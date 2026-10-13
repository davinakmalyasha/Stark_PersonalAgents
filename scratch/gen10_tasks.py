"""gen10_tasks.py — LONG-HORIZON single-agent-loop suite (3 tasks).

  g10-landing-nextjs     gen10-landing : greenfield Next.js marketing site
  g10-refine-notes       gen10-refine  : harden / productionize an existing minimal app
  g10-ecommerce-complete gen10-build   : complete e-commerce store from scratch

Policy (2026-09 rewrite):
  * Prompts state ONLY the environment/stack facts, then demand complex,
    production-grade, real-world quality. No feature checklists — the agent
    must decide what "production-grade" means for the domain.
  * Hidden gates encode the standard (big-brand landing / amazon-core store /
    production service), behaviorally and structurally. They are the judge,
    not the prompt.
  * No artificial limits: BSA_UNLIMITED + PAI_UNLIMITED + full production Dev
    tools (update_plan; ask_user stays excluded in eval-only runs) + repair
    rounds continue the SAME Dev session (BSA_KEEP_DEV_SESSION=1) so the
    failed gate is a correction inside one long run, not a cold restart.

RECOMMENDED RUN (PowerShell):

    venv312/Scripts/python.exe scratch/eval_runner.py --only g10-refine-notes --keep
    venv312/Scripts/python.exe scratch/eval_runner.py --gens 10 --keep
    venv312/Scripts/python.exe scratch/gen10_review.py --table
"""
import glob
import json as _json
import os
import random
import re
import shutil
import socket
import subprocess
import time
import urllib.error
import urllib.request

# Environment flags pushed on every task setup(). Read by eval_runner (repair
# loop), tools/_schemas (tool surface), llm/native_tools (usage limits) and
# agents/developer_agent (session continuation + completion gates).
_ENV_FLAGS = {
    "BSA_UNLIMITED": "1",          # no attempts/repairs/wall/bailout caps
    "PAI_UNLIMITED": "1",          # no pydantic-ai UsageLimits
    "BSA_FULL_DEV_TOOLS": "1",     # production Dev surface (update_plan)
    "BSA_KEEP_DEV_SESSION": "1",   # repair continues the same Dev session
    "BSA_REPAIR_VIA_HELPER": "0",  # direct Dev repair path
    "BSA_MAX_REPAIRS": "100",
    "BSA_REPAIR_UNTIL_DONE": "1",
    # Parent-side _run_one timeout (eval_runner) must also be 0 — BSA_UNLIMITED
    # only reaches the child via setup(); without this the 2400s default kills
    # healthy long-horizon runs. Defense-in-depth: main() also setdefaults it.
    "BSA_TASK_WALL_CAP": "0",
    # Context management (NOT an effort cap): compact before the provider's
    # context window forces an error. The agent keeps working on the summary.
    "COMPACTION_CHECKPOINT_TOKENS": "100000",
    "COMPACTION_HARD_LIMIT": "120000",
}


def _apply_env():
    os.environ.update(_ENV_FLAGS)


# ──────────────────────────────────────────────────────────────────────────
#  Shared helpers
# ──────────────────────────────────────────────────────────────────────────

_SKIP_DIRS = ("node_modules", ".git", ".next", "dist", ".agent-memory",
              ".deep_agents", "coverage", ".turbo")

_DEVICE_FILE_NAMES = {
    "nul", "con", "prn", "aux",
    *(f"com{i}" for i in range(1, 10)),
    *(f"lpt{i}" for i in range(1, 10)),
}


def _pkg(name, deps, scripts=None):
    return _json.dumps({
        "name": name,
        "version": "1.0.0",
        "private": True,
        "main": "server.js",
        "scripts": scripts or {"start": "node server.js", "test": "node --test tests/"},
        "dependencies": deps,
    }, indent=2) + "\n"


def _free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port


def _http(url, method="GET", body=None, timeout=15):
    """Minimal stdlib HTTP helper. Returns (status, text); status 0 on transport error."""
    data = None
    headers = {}
    if body is not None:
        data = _json.dumps(body).encode("utf-8")
        headers["Content-Type"] = "application/json"
    req = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read().decode("utf-8", "replace")
    except urllib.error.HTTPError as e:
        try:
            text = e.read().decode("utf-8", "replace")
        except Exception:
            text = ""
        return e.code, text
    except Exception as e:
        return 0, str(e)


def _read_text(path):
    try:
        with open(path, encoding="utf-8", errors="replace") as f:
            return f.read()
    except OSError:
        return ""


def _walk(cwd, exts=None, skip=_SKIP_DIRS):
    """Yield (relpath, fullpath) below cwd, skipping build/dep dirs."""
    for root, dirs, files in os.walk(cwd):
        dirs[:] = [d for d in dirs if d not in skip]
        for fn in files:
            if fn.lower().split(".")[0] in _DEVICE_FILE_NAMES:
                continue
            if exts and not fn.endswith(tuple(exts)):
                continue
            full = os.path.join(root, fn)
            try:
                rel = os.path.relpath(full, cwd).replace("\\", "/")
            except Exception:
                continue
            yield rel, full


def _src_blob(cwd, exts=(".js", ".ts", ".tsx", ".jsx")):
    out = []
    for rel, full in _walk(cwd, exts=exts):
        if rel.startswith("tests/") or "/tests/" in rel:
            continue
        out.append(_read_text(full))
    return "\n".join(out)


def _route_literals(cwd):
    """Every '/api/...' string literal found in source (routes/services/src)."""
    pat = re.compile(r"""['"`](/api/[A-Za-z0-9_\-/:.]*)['"`]""")
    found = set()
    for rel, full in _walk(cwd, exts=(".js", ".ts", ".tsx", ".jsx")):
        if rel.startswith("tests/") or "/tests/" in rel:
            continue
        for m in pat.finditer(_read_text(full)):
            found.add(m.group(1))
    return sorted(found)


def _find_literal(literals, keywords):
    for lit in literals:
        low = lit.lower()
        if any(k in low for k in keywords):
            return lit
    return None


def _count_tests(cwd):
    n = 0
    n_files = 0
    for rel, full in _walk(cwd, exts=(".js", ".mjs", ".cjs")):
        if not (rel.startswith("tests/") or "/tests/" in rel):
            continue
        n_files += 1
        n += len(re.findall(r"\btest\s*\(", _read_text(full)))
    return n, n_files


def _biggest_source_file(cwd, max_lines=400):
    """Return (rel, lines) of the biggest non-test source file over the limit, or None."""
    best = None
    for rel, full in _walk(cwd, exts=(".js", ".mjs", ".cjs")):
        if rel.startswith("tests/") or "/tests/" in rel or rel.startswith("public/"):
            continue
        lines = len(_read_text(full).splitlines())
        if lines > max_lines and (best is None or lines > best[1]):
            best = (rel, lines)
    return best


def _find_file_containing(cwd, needle, exts=(".json",), skip_dirs=_SKIP_DIRS):
    for root, dirs, files in os.walk(cwd):
        dirs[:] = [d for d in dirs if d not in skip_dirs]
        for fn in files:
            if not fn.endswith(exts):
                continue
            full = os.path.join(root, fn)
            try:
                if needle in _read_text(full):
                    return os.path.relpath(full, cwd).replace("\\", "/")
            except Exception:
                pass
    return None


def _count_files(cwd, exts, skip_dirs=_SKIP_DIRS + ("tests", "specs")):
    n = 0
    for rel, _full in _walk(cwd, exts=exts, skip=skip_dirs):
        if not os.path.basename(rel).startswith("_"):
            n += 1
    return n


def _nonce():
    return "".join(random.choice("abcdefghijklmnopqrstuvwxyz0123456789") for _ in range(8))


# ──────────────────────────────────────────────────────────────────────────
#  Task 1 — g10-landing-nextjs (greenfield Next.js marketing site)
# ──────────────────────────────────────────────────────────────────────────

_LANDING_PROMPT = (
    "Build the public marketing website for **Nimbus**, an AI meeting-notes SaaS, "
    "as a complete, production-grade Next.js application in this EMPTY project. "
    "Use **Next.js (App Router) + TypeScript**.\n\n"
    "This is a long, high-effort build. The bar is a perfect, award-level public site — "
    "the kind a top agency would ship for a well-funded startup: a distinctive brand and "
    "visual identity, a real design system, complete information architecture, real copy, "
    "and zero trace of a template or demo. Work in phases, verify each phase before moving "
    "on (run the build, run the app, exercise every page and endpoint you create, and have "
    "the UI subagent review the rendered result in a real browser), and do not stop while "
    "any phase is incomplete or any check fails.\n\n"
    "## Environment facts (only hard constraints)\n"
    "- npm is available; install dependencies yourself (next, react, react-dom, "
    "typescript, @types/react, @types/node).\n"
    "- The site must render fully offline: no external CDNs, web fonts, or runtime "
    "network calls to third parties.\n"
    "- Standard package.json scripts: dev, build, start.\n\n"
    "## Quality bar\n"
    "Design: one coherent identity and design system (type scale, spacing rhythm, color "
    "tokens, motion language) applied consistently everywhere; every section designed with "
    "intent — hero, product/features, social proof, pricing, FAQ, final CTA, footer, plus "
    "supporting pages (product, about, pricing, contact, and more); all interaction states "
    "covered (hover, focus, active, loading, empty, error); mobile-first responsive at every "
    "breakpoint; no lorem ipsum, real specific copy throughout.\n"
    "Accessibility: WCAG AA contrast, semantic landmarks and heading order, full keyboard "
    "operability, visible focus styles, prefers-reduced-motion handling, alt text.\n"
    "Performance: statically generated where possible, no layout shift, minimal client "
    "JavaScript, optimized/inline assets, fast first paint.\n"
    "Discoverability: per-route metadata, canonical URLs, Open Graph/Twitter tags, JSON-LD "
    "structured data, sitemap and robots.\n\n"
    "Judge the result the way a design lead, an SEO specialist and a staff engineer would "
    "judge a client's new site: all of the above has to hold up together. Use the UI "
    "subagent for the design direction and a browser-based design/QA pass, and fix every "
    "finding before finishing.\n\n"
    "A hidden end-to-end gate runs your production build, starts the app and "
    "exercises the public site and its APIs."
)


def _landing_routes(d):
    """Discover App Router pages -> URL paths. Returns (pages, api_routes)."""
    pages = set()
    api_routes = {}
    for base in ("app", "src/app"):
        base_dir = os.path.join(d, base.replace("/", os.sep))
        if not os.path.isdir(base_dir):
            continue
        for ext in ("tsx", "jsx", "ts", "js"):
            for p in glob.glob(os.path.join(base_dir, "**", f"page.{ext}"), recursive=True):
                rel_dir = os.path.relpath(os.path.dirname(p), base_dir).replace("\\", "/")
                segs = [s for s in rel_dir.split("/")
                        if s and s != "." and not (s.startswith("(") and s.endswith(")"))]
                pages.add("/" + "/".join(segs) if segs else "/")
        for ext in ("ts", "js"):
            for p in glob.glob(os.path.join(base_dir, "**", f"route.{ext}"), recursive=True):
                rel_dir = os.path.relpath(os.path.dirname(p), base_dir).replace("\\", "/")
                segs = [s for s in rel_dir.split("/")
                        if s and s != "." and not (s.startswith("(") and s.endswith(")"))]
                if segs:
                    api_routes["/" + "/".join(segs)] = p
    return sorted(pages), api_routes


def _next_gate(d):
    """Big-brand landing gate: build -> start -> routes -> SEO -> a11y -> APIs."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    pkg_path = os.path.join(d, "package.json")
    if not os.path.isfile(pkg_path):
        return False, "PACKAGE: package.json missing — Next.js app was not scaffolded"
    try:
        pkg = _json.load(open(pkg_path, encoding="utf-8"))
        scripts = pkg.get("scripts") or {}
    except Exception as e:
        return False, f"PACKAGE: package.json unreadable: {e}"

    npm = shutil.which("npm") or "npm"
    if not os.path.isdir(os.path.join(d, "node_modules")):
        try:
            r = subprocess.run([npm, "install", "--no-audit", "--no-fund"], cwd=d,
                               capture_output=True, text=True, timeout=1800, creationflags=flags)
        except subprocess.TimeoutExpired:
            return False, "NPM INSTALL timeout (1800s)"
        if r.returncode != 0:
            return False, "NPM INSTALL failed: " + ((r.stdout or "") + (r.stderr or ""))[-800:]

    build_cmd = [npm, "run", "build"] if scripts.get("build") else "npx next build"
    try:
        r = subprocess.run(build_cmd, cwd=d, shell=isinstance(build_cmd, str),
                           capture_output=True, text=True, timeout=1800, creationflags=flags)
    except subprocess.TimeoutExpired:
        return False, "NEXT BUILD timeout (1800s)"
    if r.returncode != 0:
        return False, "NEXT BUILD failed (rc=" + str(r.returncode) + "): " + \
            ((r.stdout or "") + (r.stderr or ""))[-1200:]

    pages, api_routes = _landing_routes(d)
    if not pages:
        return False, "ROUTES: no App Router page.tsx found"
    if "/" not in pages:
        return False, "ROUTES: no home page ('/')"
    for canonical in ("/pricing", "/contact"):
        if canonical not in pages:
            return False, f"ROUTES: required page {canonical!r} missing (found: {sorted(pages)[:12]})"
    if len(pages) < 5:
        return False, f"ROUTES: only {len(pages)} page(s) — a production marketing site is broader/richer (found: {pages})"

    port = _free_port()
    log_path = os.path.join(d, "_next_server.log")
    try:
        logf = open(log_path, "wb")
    except OSError:
        return False, "could not open server log"
    proc = subprocess.Popen("npx next start -p " + str(port), cwd=d, shell=True,
                            stdout=logf, stderr=subprocess.STDOUT, creationflags=flags)
    base = f"http://127.0.0.1:{port}"
    try:
        up = False
        for _ in range(120):
            st, _body = _http(base + "/", timeout=5)
            if st:
                up = True
                break
            if proc.poll() is not None:
                break
            time.sleep(1)
        if not up:
            tail = _read_text(log_path)[-600:]
            return False, "NEXT SERVER did not come up — " + tail

        # ── every static route must render ──
        fetched = 0
        for route in pages:
            if "[" in route:
                continue
            st, _body = _http(base + route)
            if st != 200:
                return False, f"ROUTE {route} status {st}"
            fetched += 1
        if fetched < 5:
            return False, f"ROUTES: only {fetched} static page(s) fetchable"
        st, _ = _http(base + "/definitely-not-a-page-" + _nonce())
        if st != 404:
            return False, f"404 HANDLING: unknown route returned {st} (want 404)"

        # ── home structure ──
        st, home = _http(base + "/")
        if st != 200:
            return False, f"HOME status {st}"
        hlow = home.lower()
        if "<html" not in hlow or "lang=" not in hlow:
            return False, "HOME: missing <html lang>"
        m = re.search(r"<title[^>]*>(.*?)</title>", home, re.IGNORECASE | re.DOTALL)
        if not m or not m.group(1).strip():
            return False, "HOME: missing <title>"
        if not re.search(r'<meta[^>]+name=["\']description["\'][^>]+content=["\'][^"\']+["\']', home, re.IGNORECASE):
            return False, "HOME: missing meta description"
        if "viewport" not in hlow:
            return False, "HOME: missing viewport meta"
        if ("<header" not in hlow) and ("<nav" not in hlow):
            return False, "HOME: no <header>/<nav> — no site navigation"
        if "<footer" not in hlow:
            return False, "HOME: no <footer>"
        if len(re.findall(r"<h2[ >]", hlow)) < 4:
            return False, "HOME: fewer than 4 <h2> sections — thin one-screen page"
        if "application/ld+json" not in home:
            return False, "SEO: no JSON-LD structured data on home"
        if not re.search(r'<meta[^>]+property=["\']og:', home, re.IGNORECASE):
            return False, "SEO: no Open Graph tags on home"
        if not re.search(r'<link[^>]+rel=["\']canonical["\']', home, re.IGNORECASE):
            return False, "SEO: no canonical link on home"
        if not re.search(r"<h1[ >]", hlow):
            return False, "HOME: missing <h1> — no page hierarchy"
        for _img in re.findall(r"<img\b[^>]*>", home, re.IGNORECASE):
            if "alt=" not in _img.lower():
                return False, "A11Y: <img> without alt attribute on home"

        # ── discoverability ──
        st, smap = _http(base + "/sitemap.xml")
        if st != 200 or "<url" not in smap.lower():
            return False, f"SITEMAP: /sitemap.xml status {st}"
        st, robots = _http(base + "/robots.txt")
        if st != 200 or not robots.strip():
            return False, f"ROBOTS: /robots.txt status {st}"

        # ── contact form ──
        st, contact = _http(base + "/contact")
        if st != 200:
            return False, f"CONTACT status {st}"
        clow = contact.lower()
        if "<form" not in clow:
            return False, "CONTACT page has no <form>"
        if "<label" not in clow and "aria-label" not in clow:
            return False, "A11Y: contact form has no labels"

        # ── CSS system: responsive + design tokens + theme ──
        css_blob = ""
        for p in glob.glob(os.path.join(d, ".next", "static", "**", "*.css"), recursive=True):
            css_blob += _read_text(p)
        css_blob += " ".join(re.findall(r"<style[^>]*>(.*?)</style>", home, re.DOTALL))
        if "@media" not in css_blob:
            return False, "CSS: no responsive breakpoints (@media) in built styles"
        if ":focus" not in css_blob and ":focus-visible" not in css_blob:
            return False, "A11Y: no focus styles in built CSS"
        if "prefers-reduced-motion" not in css_blob:
            return False, "A11Y: no prefers-reduced-motion handling in built CSS"
        if css_blob.count("--") < 10:
            return False, "CSS: no design-token system (CSS custom properties) in built styles"
        dark_signal = ("prefers-color-scheme" in css_blob) or ("[data-theme" in css_blob) \
            or (".dark" in css_blob)
        if not dark_signal:
            return False, "CSS: no theme handling (light/dark) found"

        # ── component architecture ──
        special = ("page", "layout", "route", "sitemap", "robots", "not-found", "error",
                   "loading", "template", "default", "global-error", "opengraph-image",
                   "icon", "apple-icon", "manifest")
        comps = 0
        for rel, _full in _walk(d, exts=(".tsx", ".jsx")):
            if rel.startswith("node_modules/"):
                continue
            base_name = os.path.splitext(os.path.basename(rel))[0]
            if base_name in special:
                continue
            comps += 1
        if comps < 15:
            return False, f"COMPONENTS: only {comps} component file(s) (< 15) — single-file site"
        big = None
        for base in ("app", "src/app"):
            for ext in ("tsx", "jsx", "ts", "js"):
                for p in glob.glob(os.path.join(d, base, "**", f"page.{ext}"), recursive=True):
                    lines = len(_read_text(p).splitlines())
                    if lines > 400 and (big is None or lines > big[1]):
                        big = (p, lines)
        if big:
            return False, f"COMPONENTS: page file {os.path.relpath(big[0], d)} is {big[1]} lines (>400)"

        # ── APIs + persistence ──
        if len(api_routes) < 2:
            return False, f"API: only {len(api_routes)} route file(s) under app/api — no backend"
        nl_route = next((u for u in api_routes if "newsletter" in u or "subscribe" in u), None)
        ct_route = next((u for u in api_routes if "contact" in u or "lead" in u), None)
        if not nl_route:
            return False, f"API: no newsletter/subscribe route (found: {sorted(api_routes)})"
        if not ct_route:
            return False, f"API: no contact/lead route (found: {sorted(api_routes)})"
        email = f"eval-{_nonce()}@example.com"
        st, body = _http(base + nl_route, "POST", {"email": email})
        if st not in (200, 201):
            return False, f"NEWSLETTER valid status {st}: {body[:200]}"
        st, body = _http(base + nl_route, "POST", {"email": "not-an-email"})
        if st != 400:
            return False, f"NEWSLETTER invalid status {st} (want 400): {body[:200]}"
        st, body = _http(base + ct_route, "POST",
                         {"name": "Eval", "email": email, "message": "hello"})
        if st not in (200, 201):
            return False, f"CONTACT api valid status {st}: {body[:200]}"
        st, body = _http(base + ct_route, "POST", {"email": email})
        if st != 400:
            return False, f"CONTACT api invalid status {st} (want 400): {body[:200]}"
        persisted = _find_file_containing(d, email)
        if not persisted:
            return False, "PERSISTENCE: no JSON file contains the submitted email"
        return True, (f"landing ok (build + {len(pages)} routes + sitemap/robots + ld+json + "
                      f"css tokens/{comps} components + api + persisted in {persisted})")
    finally:
        try:
            logf.close()
        except Exception:
            pass
        try:
            subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                           capture_output=True, timeout=15, creationflags=flags)
        except Exception:
            pass


# ──────────────────────────────────────────────────────────────────────────
#  Task 2 — g10-refine-notes (refine and upgrade an existing notes workspace)
# ──────────────────────────────────────────────────────────────────────────

# The starter project lives in its own module (18 files): a working but rough
# multi-user NoteHub workspace with deliberate debt documented in docs/TODO.md.
from gen10_refine_starter import REFINE_STARTER as _REFINE_STARTER


_REFINE_PROMPT = (
    "This project is NoteHub — a working multi-user notes workspace that grew from a "
    "prototype into something people actually use. It works, but it is rough: read the "
    "code and the docs (especially docs/TODO.md and docs/API.md) and map what exists "
    "before changing anything. Then REFINE and UPGRADE it: keep every working feature "
    "working, make the rough parts genuinely better, implement what the docs mark as "
    "missing, and restructure the codebase the way a real engineering team would "
    "maintain it.\n\n"
    "This is a long, high-effort task. Work in phases, verify each phase before moving "
    "on (run the app, exercise the API, run the tests), and do not stop while any phase "
    "is incomplete or any check fails. Judge the result the way a staff engineer "
    "reviewing a service about to get its first real users would: structure, "
    "persistence, validation, error behavior, security, features, tests, documentation "
    "and operational concerns all matter — and the product itself should end up "
    "visibly better than what you started with. Use the UI subagent for a browser pass "
    "over the frontend.\n\n"
    "A hidden gate exercises the API surface and behavior (including restarts), "
    "inspects the project structure, and runs your test suite."
)


def _refine_api_js(u1):
    """Behavior gate: auth + notes + trash/restore + history + export + stats.

    Runs under a fresh registered account so the assertions are exact and
    idempotent across repair rounds.
    """
    return (
        "    const post = (p, b, t) => fetch(base + p, { method: 'POST', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    const put = (p, b, t) => fetch(base + p, { method: 'PUT', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    const del = (p, t) => fetch(base + p, { method: 'DELETE', headers: t ? {'Authorization':'Bearer ' + t} : {} });\n"
        "    const get = (p, t) => fetch(base + p, t ? { headers: {'Authorization':'Bearer ' + t} } : undefined);\n"
        "    let r = await get('/health');\n"
        "    if (r.status !== 200) throw new Error('health ' + r.status);\n"
        "    r = await post('/api/auth/register', { name: 'Eve', email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 201) throw new Error('register ' + r.status);\n"
        "    r = await post('/api/auth/register', { name: 'Eve', email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 409) throw new Error('dup register ' + r.status);\n"
        "    r = await post('/api/auth/register', { name: '', email: 'bad', password: 'x' });\n"
        "    if (r.status !== 400) throw new Error('bad register ' + r.status);\n"
        "    r = await post('/api/auth/login', { email: '" + u1 + "', password: 'wrong-pass' });\n"
        "    if (r.status !== 401) throw new Error('bad login ' + r.status);\n"
        "    r = await post('/api/auth/login', { email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 200) throw new Error('login ' + r.status);\n"
        "    const token = (await r.json()).token;\n"
        "    if (!token) throw new Error('no token in login response');\n"
        "    r = await get('/api/me', token);\n"
        "    if (r.status !== 200) throw new Error('me ' + r.status);\n"
        "    if ((await r.json()).email !== '" + u1 + "') throw new Error('me email mismatch');\n"
        "    r = await get('/api/me');\n"
        "    if (r.status !== 401) throw new Error('me anon ' + r.status);\n"
        "    r = await get('/api/stats', token);\n"
        "    if (r.status !== 200) throw new Error('stats ' + r.status);\n"
        "    const base0 = (await r.json()).notes;\n"
        "    if (typeof base0 !== 'number') throw new Error('stats.notes not a number');\n"
        "    r = await post('/api/notes', { title: 'zebra-xyz', content: 'first note', tags: ['red', 'blue'], folder: 'work', pinned: true }, token);\n"
        "    if (r.status !== 201) throw new Error('create ' + r.status);\n"
        "    const created = await r.json();\n"
        "    if (!created.id || created.title !== 'zebra-xyz') throw new Error('create body ' + JSON.stringify(created));\n"
        "    if (!Array.isArray(created.tags) || created.tags.indexOf('red') === -1) throw new Error('tags missing: ' + JSON.stringify(created.tags));\n"
        "    r = await post('/api/notes', { title: 'second note', content: 'plain content' }, token);\n"
        "    if (r.status !== 201) throw new Error('create2 ' + r.status);\n"
        "    const second = await r.json();\n"
        "    r = await get('/api/notes/' + created.id, token);\n"
        "    if (r.status !== 200) throw new Error('get by id ' + r.status);\n"
        "    r = await put('/api/notes/' + created.id, { title: 'zebra-xyz-updated' }, token);\n"
        "    if (r.status !== 200) throw new Error('update ' + r.status);\n"
        "    const updated = await r.json();\n"
        "    if (updated.title !== 'zebra-xyz-updated') throw new Error('update body ' + JSON.stringify(updated));\n"
        "    r = await get('/api/notes/' + created.id + '/history', token);\n"
        "    if (r.status !== 200) throw new Error('history ' + r.status);\n"
        "    let hist = await r.json();\n"
        "    const revs = Array.isArray(hist) ? hist : (hist.items || hist.revisions || []);\n"
        "    if (!revs.length) throw new Error('history empty after update: ' + JSON.stringify(hist).slice(0, 160));\n"
        "    r = await get('/api/notes?q=ZEBRA-XYZ-UPDATED', token);\n"
        "    if (r.status !== 200) throw new Error('search ' + r.status);\n"
        "    let list = (await r.json()).items;\n"
        "    if (!Array.isArray(list) || !list.some(n => n.id === created.id)) throw new Error('case-insensitive search miss ' + JSON.stringify(list));\n"
        "    r = await get('/api/notes?tag=red', token);\n"
        "    list = (await r.json()).items;\n"
        "    if (!Array.isArray(list) || !list.length || list.some(n => (n.tags || []).indexOf('red') === -1)) throw new Error('tag filter ' + JSON.stringify(list));\n"
        "    r = await get('/api/notes?limit=1&page=1', token);\n"
        "    const p1 = (await r.json()).items;\n"
        "    if (!Array.isArray(p1) || p1.length !== 1) throw new Error('page 1 limit not applied ' + JSON.stringify(p1));\n"
        "    if (!r.headers.get('x-total-count')) throw new Error('missing X-Total-Count header');\n"
        "    r = await get('/api/notes?limit=1&page=2', token);\n"
        "    const p2 = (await r.json()).items;\n"
        "    if (!Array.isArray(p2) || p2.length !== 1) throw new Error('page 2 lost an item — pagination off-by-one: ' + JSON.stringify(p2));\n"
        "    if (p2[0].id === p1[0].id) throw new Error('page 2 returned the page 1 item');\n"
        "    r = await post('/api/notes', {}, token);\n"
        "    if (r.status !== 400) throw new Error('validation post {} got ' + r.status);\n"
        "    r = await post('/api/notes', { title: '   ' }, token);\n"
        "    if (r.status !== 400) throw new Error('blank title got ' + r.status);\n"
        "    r = await get('/api/notes/99999', token);\n"
        "    if (r.status !== 404) throw new Error('missing note got ' + r.status);\n"
        "    r = await fetch(base + '/api/notes', { method: 'POST', headers: Object.assign({'Content-Type':'application/json'}, {'Authorization':'Bearer ' + token}), body: '{\"title\": ' });\n"
        "    if (r.status !== 400) throw new Error('malformed json got ' + r.status);\n"
        "    r = await get('/api/stats', token);\n"
        "    const active2 = (await r.json()).notes;\n"
        "    if (active2 !== base0 + 2) throw new Error('stats active count wrong: ' + active2 + ' (want ' + (base0 + 2) + ')');\n"
        "    r = await del('/api/notes/' + second.id, token);\n"
        "    if (r.status !== 204) throw new Error('delete ' + r.status);\n"
        "    r = await get('/api/notes/' + second.id, token);\n"
        "    if (r.status !== 404) throw new Error('deleted note still present ' + r.status);\n"
        "    r = await get('/api/trash', token);\n"
        "    if (r.status !== 200) throw new Error('trash ' + r.status);\n"
        "    let trash = await r.json();\n"
        "    if (!Array.isArray(trash) || !trash.some(n => n.id === second.id)) throw new Error('trash does not list deleted note: ' + JSON.stringify(trash).slice(0, 160));\n"
        "    r = await get('/api/stats', token);\n"
        "    const active1 = (await r.json()).notes;\n"
        "    if (active1 !== base0 + 1) throw new Error('stats count trashed notes as active: ' + active1 + ' (want ' + (base0 + 1) + ')');\n"
        "    r = await post('/api/notes/' + second.id + '/restore', {}, token);\n"
        "    if (r.status !== 200) throw new Error('restore ' + r.status);\n"
        "    r = await get('/api/notes/' + second.id, token);\n"
        "    if (r.status !== 200) throw new Error('restored note not found ' + r.status);\n"
        "    r = await get('/api/export', token);\n"
        "    if (r.status !== 200) throw new Error('export ' + r.status);\n"
        "    const exp = await r.json();\n"
        "    const exported = Array.isArray(exp) ? exp : (exp.notes || exp.items || []);\n"
        "    if (!Array.isArray(exported) || !exported.some(n => n.id === created.id)) throw new Error('export missing note: ' + JSON.stringify(exp).slice(0, 160));\n"
    )


def _refine_persist_js(u1):
    """Restart probe: account + notes survive a fresh process."""
    return (
        "    const post = (p, b) => fetch(base + p, { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify(b) });\n"
        "    const get = (p, t) => fetch(base + p, t ? { headers: {'Authorization':'Bearer ' + t} } : undefined);\n"
        "    let r = await post('/api/auth/login', { email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 200) throw new Error('account lost after restart ' + r.status);\n"
        "    const token = (await r.json()).token;\n"
        "    r = await get('/api/notes?q=zebra-xyz-updated', token);\n"
        "    if (r.status !== 200) throw new Error('list ' + r.status);\n"
        "    const list = (await r.json()).items;\n"
        "    if (!Array.isArray(list) || !list.some(n => String(n.title).indexOf('zebra-xyz-updated') !== -1)) {\n"
        "      throw new Error('note lost after restart — persistence missing');\n"
        "    }\n"
    )


def _refine_ops_js(u1):
    """Production-service probe: write/auth rate limiting with observed headers."""
    return (
        "    const post = (p, b, t) => fetch(base + p, { method: 'POST', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    let sawLimit = false;\n"
        "    let r = await post('/api/auth/login', { email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status === 429) { sawLimit = true; }\n"
        "    else if (r.status !== 200) throw new Error('ops login ' + r.status);\n"
        "    let token = '';\n"
        "    if (!sawLimit) token = (await r.json()).token;\n"
        "    for (let i = 0; !sawLimit && i < 150; i++) {\n"
        "      const rr = await post('/api/notes', { title: 'rl-' + Date.now() + '-' + i }, token);\n"
        "      if (rr.status === 429) { sawLimit = true; break; }\n"
        "      for (const k of rr.headers.keys()) {\n"
        "        const lk = k.toLowerCase();\n"
        "        if (lk.indexOf('ratelimit') !== -1 || lk === 'retry-after') { sawLimit = true; }\n"
        "      }\n"
        "      if (sawLimit) break;\n"
        "    }\n"
        "    if (!sawLimit) throw new Error('no rate limiting / rate-limit headers observed');\n"
    )


def _refine_gate(d, api, file_test, check_structure):
    nonce = _nonce()
    u1 = f"eve-{nonce}@example.com"

    ok, detail = api(d, _refine_api_js(u1))
    if not ok:
        return False, "[behavior] " + detail
    ok, detail = api(d, _refine_persist_js(u1))
    if not ok:
        return False, "[persistence] " + detail

    def _has(patterns):
        for p in patterns:
            if glob.glob(os.path.join(d, p)):
                return True
        return False

    for label, patterns in (
        ("routes", ("routes/*.js", "src/routes/*.js")),
        ("services", ("services/*.js", "src/services/*.js")),
        ("middleware", ("middleware/*.js", "src/middleware/*.js")),
    ):
        if not _has(patterns):
            return False, f"STRUCTURE: {label}/ module directory missing"

    test_files = glob.glob(os.path.join(d, "tests", "**", "*.test.js"), recursive=True)
    if not test_files:
        return False, "TESTS: no tests/*.test.js files"
    try:
        r = subprocess.run(["node", "--test"], cwd=d, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return False, "TESTS: node --test timeout"
    if r.returncode != 0:
        return False, "TESTS FAIL rc=" + str(r.returncode) + " " + ((r.stdout or "") + (r.stderr or ""))[-500:]
    n_cases, n_files = _count_tests(d)
    if n_cases < 10:
        return False, f"TESTS: only {n_cases} test case(s) — production suite expected (>= 10)"

    big = _biggest_source_file(d, max_lines=400)
    if big:
        return False, f"STRUCTURE: {big[0]} is {big[1]} lines (> 400) — not maintainable"

    if not glob.glob(os.path.join(d, "specs", "*.md")):
        return False, "DOCS: specs/*.md missing"
    readme = os.path.join(d, "README.md")
    if not os.path.isfile(readme) or "run" not in _read_text(readme).lower():
        return False, "DOCS: README.md missing run instructions"

    ok, detail = api(d, _refine_ops_js(u1))
    if not ok:
        return False, "[ops] " + detail

    return True, (f"refine ok (behavior + restart persistence + structure + {n_cases} tests/{n_files} files + "
                  f"rate-limit + history/restore/export + docs)")


# ──────────────────────────────────────────────────────────────────────────
#  Task 3 — g10-ecommerce-complete (complete store from scratch)
# ──────────────────────────────────────────────────────────────────────────

_ECOM_PROMPT = (
    "Build a COMPLETE e-commerce web store from scratch in this project. This is the big "
    "one. The bar is a real online store a customer could actually shop in and a business "
    "could run — not a CRUD demo. Work in phases, verify each phase before moving on, and "
    "do not stop while any phase is incomplete or any check fails.\n\n"
    "## Environment facts (only hard constraints)\n"
    "- Node.js + Express, CommonJS (`require`/`module.exports`). Deps already installed: "
    "express, helmet, jsonwebtoken, bcryptjs.\n"
    "- `module.exports = app;` in server.js, `app.listen` only when run directly.\n\n"
    "## Quality bar\n"
    "Judge the result the way a senior product engineer and an e-commerce operator would "
    "judge a store about to take real orders: the customer journey, the operator/admin "
    "journey, data integrity under real use, security, tests, documentation and "
    "operational concerns all have to hold up together.\n\n"
    "If credentials are available, you may also push the finished project to "
    "https://github.com/davinakmalyasha/TestAgents.git — a push is nice-to-have, "
    "not required for the gate.\n\n"
    "A hidden end-to-end gate boots the app and walks it like a real customer and a real "
    "admin — including behavior across a restart — and runs YOUR test suite."
)


def _ecom_api_js(u1, u2, nonce):
    """The full customer+admin journey, idempotent across repair rounds (unique ids)."""
    return (
        "    const post = (p, b, t) => fetch(base + p, { method: 'POST', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    const put = (p, b, t) => fetch(base + p, { method: 'PUT', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    const get = (p, t) => fetch(base + p, t ? { headers: {'Authorization':'Bearer ' + t} } : undefined);\n"
        "    let r = await get('/health');\n"
        "    if (r.status !== 200) throw new Error('health ' + r.status);\n"
        "    if (r.headers.get('x-content-type-options') !== 'nosniff') throw new Error('missing security header');\n"
        "    r = await get('/');\n"
        "    if (r.status !== 200) throw new Error('storefront / ' + r.status);\n"
        "    const home = (await r.text()).toLowerCase();\n"
        "    if (home.indexOf('product') === -1 && home.indexOf('shop') === -1 && home.indexOf('store') === -1) throw new Error('storefront has no product content');\n"
        "    r = await get('/cart');\n"
        "    if (r.status !== 200) throw new Error('cart page ' + r.status);\n"
        "    if ((await r.text()).toLowerCase().indexOf('cart') === -1) throw new Error('cart page has no cart content');\n"
        "    r = await get('/admin');\n"
        "    if (r.status !== 200) throw new Error('admin page ' + r.status);\n"
        "    if ((await r.text()).toLowerCase().indexOf('admin') === -1) throw new Error('admin page has no admin content');\n"
        "    r = await post('/api/auth/register', { email: '" + u1 + "', password: 'password123', name: 'Eve' });\n"
        "    if (r.status !== 201) throw new Error('register ' + r.status);\n"
        "    r = await post('/api/auth/register', { email: '" + u1 + "', password: 'password123', name: 'Eve' });\n"
        "    if (r.status !== 409) throw new Error('dup register ' + r.status);\n"
        "    r = await post('/api/auth/register', { email: 'bad', password: 'x', name: '' });\n"
        "    if (r.status !== 400) throw new Error('bad register ' + r.status);\n"
        "    r = await post('/api/auth/login', { email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 200) throw new Error('login ' + r.status);\n"
        "    const token = (await r.json()).token;\n"
        "    r = await get('/api/me', token);\n"
        "    if (r.status !== 200) throw new Error('me ' + r.status);\n"
        "    const me = await r.json();\n"
        "    if (me.email !== '" + u1 + "' || me.role !== 'user') throw new Error('me body ' + JSON.stringify(me));\n"
        "    r = await get('/api/me');\n"
        "    if (r.status !== 401) throw new Error('me anon ' + r.status);\n"
        "    r = await post('/api/auth/login', { email: 'admin@example.com', password: 'admin123' });\n"
        "    if (r.status !== 200) throw new Error('admin login ' + r.status);\n"
        "    const atoken = (await r.json()).token;\n"
        "    r = await post('/api/products', { name: 'Mechanical Keyboard " + nonce + "', price: 89.9, category: 'peripherals', stock: 3 }, atoken);\n"
        "    if (r.status !== 201) throw new Error('create kb ' + r.status);\n"
        "    const kb = await r.json();\n"
        "    r = await post('/api/products', { name: 'Wireless Mouse " + nonce + "', price: 24.5, category: 'peripherals', stock: 5 }, atoken);\n"
        "    if (r.status !== 201) throw new Error('create mouse ' + r.status);\n"
        "    r = await post('/api/products', { name: '4K Monitor " + nonce + "', price: 199, category: 'displays', stock: 2 }, atoken);\n"
        "    if (r.status !== 201) throw new Error('create monitor ' + r.status);\n"
        "    r = await post('/api/products', { name: '', price: 5 }, atoken);\n"
        "    if (r.status !== 400) throw new Error('bad product ' + r.status);\n"
        "    r = await post('/api/products', { name: 'X', price: 5 }, token);\n"
        "    if (r.status !== 403) throw new Error('user create product ' + r.status);\n"
        "    r = await post('/api/products', { name: 'Y', price: 5 });\n"
        "    if (r.status !== 401) throw new Error('anon create product ' + r.status);\n"
        "    r = await get('/api/products');\n"
        "    let j = await r.json();\n"
        "    if (!Array.isArray(j.items) || typeof j.total !== 'number' || j.total < 3) throw new Error('catalog shape ' + JSON.stringify(j).slice(0, 200));\n"
        "    r = await get('/api/products?q=" + nonce + "');\n"
        "    j = await r.json();\n"
        "    if (!j.items.length || j.items.some(p => p.name.toLowerCase().indexOf('" + nonce + "') === -1)) throw new Error('search ' + JSON.stringify(j.items));\n"
        "    r = await get('/api/products?category=peripherals');\n"
        "    j = await r.json();\n"
        "    if (j.items.length < 2 || j.items.some(p => p.category !== 'peripherals')) throw new Error('category filter ' + JSON.stringify(j.items));\n"
        "    r = await get('/api/products?sort=price_asc');\n"
        "    j = await r.json();\n"
        "    for (let i = 1; i < j.items.length; i++) { if (j.items[i].price < j.items[i - 1].price) throw new Error('sort price_asc violated'); }\n"
        "    r = await get('/api/products?page=1&limit=2');\n"
        "    j = await r.json();\n"
        "    if (j.items.length > 2) throw new Error('limit not applied');\n"
        "    r = await get('/api/products/' + kb.id);\n"
        "    if (r.status !== 200) throw new Error('product by id ' + r.status);\n"
        "    r = await get('/api/products/99999');\n"
        "    if (r.status !== 404) throw new Error('missing product ' + r.status);\n"
        "    r = await post('/api/cart', { productId: kb.id, qty: 2 }, token);\n"
        "    if (r.status !== 200 && r.status !== 201) throw new Error('cart add ' + r.status);\n"
        "    r = await post('/api/cart', { productId: kb.id, qty: 1 }, token);\n"
        "    if (r.status !== 200 && r.status !== 201) throw new Error('cart increment ' + r.status);\n"
        "    r = await post('/api/cart', { productId: 424242, qty: 1 }, token);\n"
        "    if (r.status !== 400) throw new Error('cart unknown product ' + r.status);\n"
        "    r = await get('/api/cart', token);\n"
        "    let cart = await r.json();\n"
        "    const line = (cart.items || []).find(x => x.productId === kb.id);\n"
        "    if (!line || line.qty !== 3) throw new Error('cart state ' + JSON.stringify(cart));\n"
        "    r = await post('/api/cart', { productId: (await (await get('/api/products?q=mouse')).json()).items[0].id, qty: 99 }, token);\n"
        "    if (r.status !== 400) throw new Error('cart over-stock ' + r.status);\n"
        "    r = await post('/api/orders', {}, token);\n"
        "    if (r.status !== 201) throw new Error('checkout ' + r.status);\n"
        "    const order = await r.json();\n"
        "    if (!order.id || !Array.isArray(order.items)) throw new Error('order shape ' + JSON.stringify(order));\n"
        "    r = await get('/api/products/' + kb.id);\n"
        "    const after = await r.json();\n"
        "    if (after.stock !== 0) throw new Error('stock not decremented: ' + after.stock);\n"
        "    r = await get('/api/cart', token);\n"
        "    cart = await r.json();\n"
        "    if ((cart.items || []).length !== 0) throw new Error('cart not emptied ' + JSON.stringify(cart));\n"
        "    r = await post('/api/orders', {}, token);\n"
        "    if (r.status !== 400) throw new Error('empty checkout ' + r.status);\n"
        "    r = await post('/api/auth/register', { email: '" + u2 + "', password: 'password123', name: 'Frank' });\n"
        "    if (r.status !== 201) throw new Error('register2 ' + r.status);\n"
        "    r = await post('/api/auth/login', { email: '" + u2 + "', password: 'password123' });\n"
        "    const t2 = (await r.json()).token;\n"
        "    r = await get('/api/orders', t2);\n"
        "    j = await r.json();\n"
        "    if (!Array.isArray(j) || j.length !== 0) throw new Error('order scoping leak ' + JSON.stringify(j));\n"
        "    r = await get('/api/orders', token);\n"
        "    j = await r.json();\n"
        "    if (!Array.isArray(j) || j.length < 1) throw new Error('own orders missing');\n"
        "    r = await get('/api/admin/orders', atoken);\n"
        "    if (r.status !== 200) throw new Error('admin orders ' + r.status);\n"
        "    r = await put('/api/admin/orders/' + order.id, { status: 'shipped' }, atoken);\n"
        "    if (r.status !== 200) throw new Error('order status update ' + r.status);\n"
        "    r = await put('/api/admin/orders/' + order.id, { status: 'nonsense' }, atoken);\n"
        "    if (r.status !== 400) throw new Error('bad order status ' + r.status);\n"
        "    for (let i = 0; i < 5; i++) await post('/api/auth/login', { email: 'nobody@example.com', password: 'wrongpass' });\n"
        "    r = await post('/api/auth/login', { email: 'nobody@example.com', password: 'wrongpass' });\n"
        "    if (r.status !== 429) throw new Error('rate limit ' + r.status);\n"
    )


def _ecom_extra_js(u1, u3, nonce):
    """Amazon-core capabilities: order detail scoping, reviews, wishlist.

    Self-contained: a fresh server process re-discovers the account, product and
    order created by the journey run (so the probes also prove persistence works).
    """
    return (
        "    const post = (p, b, t) => fetch(base + p, { method: 'POST', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    const get = (p, t) => fetch(base + p, t ? { headers: {'Authorization':'Bearer ' + t} } : undefined);\n"
        "    let r = await post('/api/auth/login', { email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 200) throw new Error('login ' + r.status);\n"
        "    const token = (await r.json()).token;\n"
        "    r = await get('/api/products?q=" + nonce + "');\n"
        "    let j = await r.json();\n"
        "    if (!j.items || !j.items.length) throw new Error('product gone after journey');\n"
        "    const kbId = j.items[0].id;\n"
        "    r = await get('/api/orders', token);\n"
        "    j = await r.json();\n"
        "    if (!Array.isArray(j) || !j.length) throw new Error('orders gone after journey');\n"
        "    const orderId = j[0].id;\n"
        "    r = await get('/api/orders/' + orderId, token);\n"
        "    if (r.status !== 200) throw new Error('order detail ' + r.status);\n"
        "    let od = await r.json();\n"
        "    if (!od || !Array.isArray(od.items)) throw new Error('order detail shape ' + JSON.stringify(od).slice(0, 160));\n"
        "    r = await post('/api/auth/register', { email: '" + u3 + "', password: 'password123', name: 'Mallory' });\n"
        "    if (r.status !== 201) throw new Error('register3 ' + r.status);\n"
        "    r = await post('/api/auth/login', { email: '" + u3 + "', password: 'password123' });\n"
        "    const t3 = (await r.json()).token;\n"
        "    r = await get('/api/orders/' + orderId, t3);\n"
        "    if (r.status !== 403 && r.status !== 404) throw new Error('order leak to stranger ' + r.status);\n"
        "    r = await post('/api/products/' + kbId + '/reviews', { rating: 5, comment: 'great keyboard' }, token);\n"
        "    if (r.status !== 200 && r.status !== 201) throw new Error('review create ' + r.status);\n"
        "    r = await get('/api/products/' + kbId + '/reviews');\n"
        "    if (r.status !== 200) throw new Error('review list ' + r.status);\n"
        "    j = await r.json();\n"
        "    const revItems = Array.isArray(j) ? j : (j.items || []);\n"
        "    if (!revItems.length) throw new Error('review list empty');\n"
        "    r = await post('/api/products/' + kbId + '/reviews', { rating: 9, comment: 'bad' }, token);\n"
        "    if (r.status !== 400) throw new Error('review rating validation ' + r.status);\n"
        "    r = await post('/api/wishlist', { productId: kbId }, token);\n"
        "    if (r.status !== 200 && r.status !== 201) throw new Error('wishlist add ' + r.status);\n"
        "    r = await get('/api/wishlist', token);\n"
        "    if (r.status !== 200) throw new Error('wishlist list ' + r.status);\n"
        "    j = await r.json();\n"
        "    const wlItems = Array.isArray(j) ? j : (j.items || []);\n"
        "    if (!wlItems.length) throw new Error('wishlist empty');\n"
    )


def _ecom_persist_js(u1, nonce):
    """Restart-persistence probe: account, catalog and orders must survive a fresh process."""
    return (
        "    const post = (p, b, t) => fetch(base + p, { method: 'POST', headers: Object.assign({'Content-Type':'application/json'}, t ? {'Authorization':'Bearer ' + t} : {}), body: JSON.stringify(b) });\n"
        "    const get = (p, t) => fetch(base + p, t ? { headers: {'Authorization':'Bearer ' + t} } : undefined);\n"
        "    let r = await post('/api/auth/login', { email: '" + u1 + "', password: 'password123' });\n"
        "    if (r.status !== 200) throw new Error('account lost after restart ' + r.status);\n"
        "    const token = (await r.json()).token;\n"
        "    r = await get('/api/products?q=" + nonce + "');\n"
        "    let j = await r.json();\n"
        "    if (!j.items || !j.items.length) throw new Error('catalog lost after restart');\n"
        "    r = await get('/api/orders', token);\n"
        "    j = await r.json();\n"
        "    if (!Array.isArray(j) || !j.length) throw new Error('orders lost after restart — no persistence');\n"
    )


def _ecom_gate(d, api, file_test):
    nonce = _nonce()
    u1 = f"eve-{nonce}@example.com"
    u2 = f"frank-{nonce}@example.com"
    u3 = f"mallory-{nonce}@example.com"

    ok, detail = api(d, _ecom_api_js(u1, u2, nonce))
    if not ok:
        return False, "[journey] " + detail

    extra = _ecom_extra_js(u1, u3, nonce)
    ok, detail = api(d, extra)
    if not ok:
        return False, "[amazon-core] " + detail

    ok, detail = api(d, _ecom_persist_js(u1, nonce))
    if not ok:
        return False, "[persistence] " + detail

    if not glob.glob(os.path.join(d, "specs", "*.md")):
        return False, "NO SPECS: specs/SPEC-*.md missing"
    if not any(os.path.isfile(os.path.join(d, e)) for e in ("server.js", "app.js", "index.js", "src/server.js")):
        return False, "NO APP ENTRY: server.js/app.js/index.js missing"
    if not os.path.isfile(os.path.join(d, "README.md")):
        return False, "NO README: README.md missing"
    tests = glob.glob(os.path.join(d, "tests", "*.test.js")) + glob.glob(os.path.join(d, "tests", "*.test.mjs"))
    if not tests:
        return False, "NO TESTS: tests/ suite missing"
    n_cases, n_files = _count_tests(d)
    if n_cases < 20:
        return False, f"TESTS: only {n_cases} test case(s) — production store expected (>= 20)"
    try:
        r = subprocess.run(["node", "--test"], cwd=d, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return False, "TESTS: node --test timeout"
    if r.returncode != 0:
        return False, "TESTS FAIL rc=" + str(r.returncode) + " " + ((r.stdout or "") + (r.stderr or ""))[-500:]

    src_all = _src_blob(d)
    if "bcrypt" not in src_all:
        return False, "SECURITY: no bcrypt — passwords are not hashed"
    if "helmet" not in src_all:
        return False, "SECURITY: helmet not used — no security headers"
    if "admin@example.com" in src_all and "admin123" in src_all and "hashSync" not in src_all and "hash(" not in src_all:
        return False, "SECURITY: admin password stored in plaintext"

    low = src_all.lower()
    if "coupon" not in low and "discount" not in low and "promo" not in low:
        return False, "OPERATOR: no coupons/discounts in the codebase"
    if "analytics" not in low and "dashboard" not in low and "report" not in low and "stats" not in low:
        return False, "OPERATOR: no analytics/dashboard/reporting in the codebase"

    real = _count_files(d, (".js", ".html", ".css"))
    if real < 20:
        return False, f"COMPLETENESS: only {real} real source files (< 20) — build is incomplete"
    try:
        public_files = [f for f in os.listdir(os.path.join(d, "public"))
                        if f.endswith((".html", ".css", ".js"))]
    except OSError:
        public_files = []
    if len(public_files) < 3:
        return False, f"FRONTEND: public/ has only {len(public_files)} asset(s) — real storefront required"

    return True, (f"ecommerce ok (journey + order detail/reviews/wishlist + persistence-across-restart "
                  f"+ {n_cases} tests/{n_files} files + specs/readme + security + operator features)")


# ──────────────────────────────────────────────────────────────────────────
#  build_tasks
# ──────────────────────────────────────────────────────────────────────────

def build_tasks(H):
    write = H["write"]
    ensure_deps = H["ensure_deps"]
    api = H["api_test"]
    file_test = H["file_test"]
    check_structure = H["check_structure"]

    tasks = []

    # ── g10-landing-nextjs — greenfield Next.js marketing site ──
    tasks.append({
        "id": "g10-landing-nextjs",
        "category": "gen10-landing",
        "name": "Nimbus Next.js Site",
        "prompt": _LANDING_PROMPT,
        "setup": lambda d: _apply_env(),
        "test": lambda d: _next_gate(d),
    })

    # ── g10-refine-notes — productionize an existing minimal app ──
    def _refine_setup(d, _files=_REFINE_STARTER):
        write(d, dict(_files))
        _apply_env()
        ensure_deps(d)

    tasks.append({
        "id": "g10-refine-notes",
        "category": "gen10-refine",
        "name": "Notes Service Productionization",
        "prompt": _REFINE_PROMPT,
        "setup": _refine_setup,
        "test": lambda d: _refine_gate(d, api, file_test, check_structure),
    })

    # ── g10-ecommerce-complete — complete store from scratch ──
    _ecom_pkg = _pkg("gen10-store", {
        "express": "^4.19.0",
        "helmet": "^7.1.0",
        "jsonwebtoken": "^9.0.2",
        "bcryptjs": "^2.4.3",
    })

    def _ecom_setup(d, _pkg_text=_ecom_pkg):
        write(d, {"package.json": _pkg_text})
        _apply_env()
        ensure_deps(d)

    tasks.append({
        "id": "g10-ecommerce-complete",
        "category": "gen10-build",
        "name": "Complete E-commerce Store",
        "prompt": _ECOM_PROMPT,
        "setup": _ecom_setup,
        "test": lambda d: _ecom_gate(d, api, file_test),
    })

    return tasks
