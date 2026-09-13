"""eval_runner.py — mini SWE-bench-style evaluation for the multi-agent harness.

Runs real coding tasks through the Dev workspace agent (developer_node single-agent
loop — no orchestrator) against throwaway projects, applies a
HIDDEN test after each run (the agent never sees it — SWE-bench style), and reports
usage + quality + success per task. Instruments both sides:
  - paid calls : shared_state token_usage (tokens, cost, reasoning_chars)
  - free BSA   : BSA_TELEMETRY_FILE JSONL (provider, capability, chars)

MUST run with the harness python (needs the `deepagents` package):
    venv312/Scripts/python.exe scratch/eval_runner.py --limit 3
    venv312/Scripts/python.exe scratch/eval_runner.py --only web-server
    venv312/Scripts/python.exe scratch/eval_runner.py --category bugfix
    venv312/Scripts/python.exe scratch/eval_runner.py --all
    venv312/Scripts/python.exe scratch/eval_runner.py --all --keep   # keep tmp dirs on fail

Reports: scratch/eval_runs/<task_id>.json  + a printed summary.
"""
import argparse
import collections
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import uuid

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
_OUTDIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "eval_runs")
_EVENTS_LOG = os.path.join(_OUTDIR, "_events.jsonl")
PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
def _emit(kind: str, **fields):
    """Emit a structured progress event — printed live as `[EVENT] {json}` and appended to
    scratch/eval_runs/_events.jsonl so an external agent (e.g. OpenCode) can watch the run."""
    try:
        os.makedirs(_OUTDIR, exist_ok=True)
        rec = {"ts": round(time.time(), 3), "event": kind, **fields}
        line = json.dumps(rec, default=str)
        print(f"[EVENT] {line}", flush=True)
        with open(_EVENTS_LOG, "a", encoding="utf-8") as f:
            f.write(line + "\n")
    except Exception:
        pass


# ──────────────────────────────────────────────────────────────────────────
#  Node test helpers (hidden tests)
# ──────────────────────────────────────────────────────────────────────────

def _dump_failure(cwd, text):
    """Persist the FULL hidden-test output so a later Dev repair round
    can read it (the agent never sees it during its own attempt)."""
    try:
        with open(os.path.join(cwd, "_hidden_failure.log"), "w", encoding="utf-8") as f:
            f.write(str(text or ""))
    except Exception:
        pass


NODE_TEST_TMPL = """\
let entry = require('./__ENTRY__');
// Resolve common export shapes: module.exports = app | { app } | { default } | { createApp }
const app =
  (entry && typeof entry === 'object' && !entry.listen && entry.app) ||
  (entry && entry.default) ||
  (entry && typeof entry === 'object' && !entry.listen && entry.createApp && entry.createApp()) ||
  entry;
if (!app || typeof app.listen !== 'function') {
  console.error('FAIL: entry did not expose a listenable express app (' + JSON.stringify(Object.keys(entry || {})) + ')');
  process.exit(1);
}
const server = app.listen(0, async () => {
  const port = server.address().port;
  const base = 'http://127.0.0.1:' + port;
  try {
__BODY__
    console.log('PASS');
    process.exit(0); // no server.close(): exit during handle-close crashes node on Windows (libuv assertion)
  } catch (e) {
    console.error('FAIL: ' + (e && e.message));
    process.exit(1);
  }
});
"""


def _run_node(cwd, script_file, timeout=90):
    node = shutil.which("node") or "node"
    try:
        r = subprocess.run([node, script_file], cwd=cwd, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return -1, "", "timeout"


def _ensure_deps(cwd):
    """Ensure node_modules matches package.json. Reinstalls only when package.json's
    CONTENT changed since the last install (a marker file in node_modules records the
    hash — mtime-based checks would reinstall every time setup rewrites the file).
    Retries once; silent on failure (a missing dep then surfaces as a test failure,
    which is itself a valid signal)."""
    pkg = os.path.join(cwd, "package.json")
    nm = os.path.join(cwd, "node_modules")
    if not os.path.isfile(pkg):
        return
    marker = os.path.join(nm, ".installed-pkg-hash")
    try:
        with open(pkg, "rb") as f:
            h = hashlib.md5(f.read()).hexdigest()
        if os.path.isfile(marker) and open(marker, encoding="utf-8").read().strip() == h:
            return
    except Exception:
        h = ""
    npm = shutil.which("npm") or "npm"  # full path: bare "npm" isn't resolved as npm.CMD by subprocess
    for _attempt in range(2):
        try:
            r = subprocess.run([npm, "install", "--silent"], cwd=cwd, capture_output=True, text=True, timeout=180)
            if r.returncode == 0 and os.path.isdir(nm):
                try:
                    with open(marker, "w", encoding="utf-8") as f:
                        f.write(h)
                except Exception:
                    pass
                return
        except subprocess.TimeoutExpired:
            pass


def _api_test(cwd, js_body, entries=("server", "app", "index", "src/server")):
    """Start the app via require(entry), run JS fetch assertions. Returns (ok, detail).

    If an entry LOADS but assertions fail, that's a real failure — report it immediately
    instead of masking it with the next entry's "module not found" error.
    """
    _ensure_deps(cwd)
    script_path = os.path.join(cwd, "_hidden_test.js")
    load_error = None
    for entry in entries:
        src = NODE_TEST_TMPL.replace("__ENTRY__", entry).replace("__BODY__", js_body)
        with open(script_path, "w", encoding="utf-8") as f:
            f.write(src)
        rc, out, err = _run_node(cwd, "_hidden_test.js")
        combined = (out or "") + "\n" + (err or "")
        _dump_failure(cwd, combined)
        # "PASS" is printed (stdout) only after every assertion in the body succeeded, so it is
        # the authoritative success signal (rc may be non-zero on a Windows exit race).
        if "PASS" in (out or "") and "FAIL:" not in combined:
            try:
                os.remove(script_path)
            except OSError:
                pass
            return True, f"via {entry} (rc={rc})"
        if "FAIL:" in combined:
            # app loaded but an assertion failed — real failure, don't fall through to other entries
            try:
                os.remove(script_path)
            except OSError:
                pass
            last_line = [l for l in combined.strip().splitlines() if l.startswith("FAIL:")][-1]
            return False, f"({entry}) {last_line[:200]}"
        # entry failed to load (e.g. wrong filename) — remember and try the next candidate
        snippet = [l for l in combined.strip().splitlines() if l.strip() and 'at ' not in l[:4]]
        # capture the meaningful tail (skip node version banner + require-stack noise)
        meaningful = [l for l in snippet if not l.startswith('Node.js v') and 'require stack' not in l.lower()][-1:] or snippet[-1:]
        load_error = (entry, meaningful[-1][:200] if meaningful else "no output")
    try:
        os.remove(script_path)
    except OSError:
        pass
    return False, f"no entry loaded — tried {[e for e in entries]} — last error: {load_error}"


def _file_test(cwd, checks):
    """checks: list of (relpath, substring_or_None). All must hold. Returns (ok, detail)."""
    missing = []
    for path, substr in checks:
        full = os.path.join(cwd, path)
        if not os.path.isfile(full):
            missing.append(f"{path} MISSING")
        elif substr and substr not in open(full, encoding="utf-8", errors="replace").read():
            missing.append(f"{path} lacks {substr!r}")
    return (not missing), ("ok" if not missing else "; ".join(missing))


def _require_test(cwd, entry, js_asserts):
    script = "const m = require('./" + entry + "');\n" + js_asserts + "\nconsole.log('PASS');\n"
    p = os.path.join(cwd, "_hidden_test.js")
    with open(p, "w", encoding="utf-8") as f:
        f.write(script)
    rc, out, err = _run_node(cwd, "_hidden_test.js")
    try:
        os.remove(p)
    except OSError:
        pass
    combined = (out or "") + "\n" + (err or "")
    _dump_failure(cwd, combined)
    return (rc == 0 and "PASS" in out), f"rc={rc} {out[:200]}{err[:200]}"


def _require_async_test(cwd, entry, js_body):
    """Like _require_test but wraps the body in an async IIFE (supports await)."""
    script = (
        "const m = require('./" + entry + "');\n"
        "(async () => {\n"
        "  try {\n"
        + js_body + "\n"
        "    console.log('PASS');\n"
        "    process.exit(0);\n"
        "  } catch (e) {\n"
        "    console.error('FAIL: ' + (e && e.message));\n"
        "    process.exit(1);\n"
        "  }\n"
        "})();\n"
    )
    p = os.path.join(cwd, "_hidden_test.js")
    with open(p, "w", encoding="utf-8") as f:
        f.write(script)
    rc, out, err = _run_node(cwd, "_hidden_test.js")
    try:
        os.remove(p)
    except OSError:
        pass
    combined = (out or "") + "\n" + (err or "")
    _dump_failure(cwd, combined)
    return ("PASS" in (out or "") and "FAIL:" not in combined), f"rc={rc} {combined[:200]}"


def _run_python(cwd, script_file, timeout=90):
    """Run a python script with the harness python (venv312). Returns (rc, stdout, stderr)."""
    py = sys.executable
    try:
        r = subprocess.run([py, script_file], cwd=cwd, capture_output=True, text=True, timeout=timeout)
        return r.returncode, r.stdout, r.stderr
    except subprocess.TimeoutExpired:
        return -1, "", "timeout"


def _py_test(cwd, module, py_body, setup=""):
    """Python version of _require_test: import `module` (e.g. 'utils'), run py_body with `m` bound."""
    script = (
        "import sys, importlib\n"
        "sys.path.insert(0, '.')\n"
        "m = importlib.import_module('" + module + "')\n"
        + (setup + "\n" if setup else "") +
        "try:\n"
        "    " + py_body.replace("\n", "\n    ") + "\n"
        "    print('PASS')\n"
        "    sys.exit(0)\n"
        "except Exception as e:\n"
        "    print('FAIL:', e)\n"
        "    sys.exit(1)\n"
    )
    p = os.path.join(cwd, "_hidden_test.py")
    with open(p, "w", encoding="utf-8") as f:
        f.write(script)
    rc, out, err = _run_python(cwd, "_hidden_test.py")
    try:
        os.remove(p)
    except OSError:
        pass
    combined = (out or "") + "\n" + (err or "")
    _dump_failure(cwd, combined)
    return ("PASS" in (out or "") and "FAIL:" not in combined), f"rc={rc} {combined[:200]}"


def _check_structure(cwd, checks):
    """structure checks: list of (kind, *args).
    kinds: ('file_exists', rel), ('file_absent', rel), ('has_text', rel, substr),
           ('lacks_text', rel, substr), ('has_export', rel, name), ('lacks_export', rel, name).
    Returns (ok, detail)."""
    for kind, *args in checks:
        if kind == "file_exists":
            if not os.path.isfile(os.path.join(cwd, args[0])):
                return False, f"STRUCTURE: file {args[0]} MISSING"
        elif kind == "file_absent":
            if os.path.isfile(os.path.join(cwd, args[0])):
                return False, f"STRUCTURE: file {args[0]} should not exist"
        elif kind == "has_text":
            full = os.path.join(cwd, args[0])
            if not os.path.isfile(full):
                return False, f"STRUCTURE: file {args[0]} MISSING"
            if args[1] not in open(full, encoding="utf-8", errors="replace").read():
                return False, f"STRUCTURE: {args[0]} lacks {args[1]!r}"
        elif kind == "lacks_text":
            full = os.path.join(cwd, args[0])
            if os.path.isfile(full) and args[1] in open(full, encoding="utf-8", errors="replace").read():
                return False, f"STRUCTURE: {args[0]} still contains {args[1]!r}"
        elif kind in ("has_export", "lacks_export"):
            full = os.path.join(cwd, args[0])
            name = args[1]
            exported = False
            if os.path.isfile(full):
                src = open(full, encoding="utf-8", errors="replace").read()
                # 1) explicit: exports.name = ...  or  module.exports.name = ...
                if re.search(rf"(?:exports|module\.exports)\.{re.escape(name)}\b", src):
                    exported = True
                # 2) object literal: module.exports = { name, ... } / { name: ... }
                m = re.search(r"module\.exports\s*=\s*\{(.*?)\}", src, re.DOTALL)
                if m:
                    body = m.group(1)
                    if re.search(rf"(?:^|,|\s)\s*{re.escape(name)}\s*(?::|,|}}|$)", body):
                        exported = True
            if kind == "has_export" and not exported:
                return False, f"STRUCTURE: {args[0]} no longer exports {name}"
            if kind == "lacks_export" and exported:
                return False, f"STRUCTURE: {args[0]} still exports {name}"
    return True, "structure ok"


def _refactor_test(cwd, entry, js_body, structure_checks):
    """Behavior-preserving refactor test: JS body must still PASS **and** the structure
    checks (file moves / export renames / deletions) must hold. Returns (ok, detail)."""
    ok, detail = _require_async_test(cwd, entry, js_body)
    if not ok:
        return ok, detail
    sok, sdetail = _check_structure(cwd, structure_checks)
    return sok, sdetail


def _ops_test(cwd, cmd, must_contain=None, expect_rc=None, timeout=60):
    """Terminal-style hidden test: run a real shell command in the project dir and check
    the outcome. Terminal-Bench style — the agent must have made the project actually
    operable. Returns (ok, detail).

    - must_contain: substring required in combined stdout+stderr.
    - expect_rc: required exit code.
    - A timeout with must_contain satisfied counts as OK (server-start tasks).

    Output goes to a FILE (not a pipe) so a long-running server grandchild cannot keep
    the pipe open and hang the timeout; on timeout the whole process tree is killed."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    _ensure_deps(cwd)  # install deps the agent may have added to package.json
    outfile = os.path.join(cwd, "_ops_out.txt")
    try:
        os.remove(outfile)
    except OSError:
        pass
    rc = None
    with open(outfile, "wb") as f:
        proc = subprocess.Popen(cmd, cwd=cwd, shell=True, stdout=f, stderr=subprocess.STDOUT,
                                creationflags=flags)
        try:
            rc = proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            rc = -1  # timed out — acceptable only if the expected marker was produced
        finally:
            try:
                subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                               capture_output=True, timeout=10, creationflags=flags)
            except Exception:
                pass
    try:
        out = open(outfile, encoding="utf-8", errors="replace").read()
    except OSError:
        out = ""
    _dump_failure(cwd, out)
    if expect_rc is not None and rc != expect_rc:
        return False, f"rc={rc} expected {expect_rc} — {out[-300:]}"
    if must_contain and must_contain not in out:
        return False, f"missing {must_contain!r} in output — {out[-300:]}"
    return True, f"rc={rc} {out.strip()[-200:]}"


# ──────────────────────────────────────────────────────────────────────────
#  Helpers
# ──────────────────────────────────────────────────────────────────────────

# When `--style user` is active, setup files are sanitized: `// BUG:` / `# BUG:`
# marker comments (which point straight at the defect) are stripped so the agent
# must find the bug on its own, SWE-bench style.
_USER_STYLE = False


def _strip_markers(text):
    """Remove trailing '// BUG' / '# BUG' marker comments line-by-line."""
    out = []
    for line in text.split("\n"):
        stripped = re.sub(r"(//|#)\s*BUG\b.*$", "", line).rstrip()
        out.append(stripped)
    return "\n".join(out)


def _write(cwd, files):
    for rel, content in files.items():
        full = os.path.join(cwd, rel)
        os.makedirs(os.path.dirname(full), exist_ok=True)
        if _USER_STYLE and rel.endswith((".js", ".py", ".json", ".html", ".css", ".md", ".log")):
            content = _strip_markers(content)
        with open(full, "w", encoding="utf-8") as f:
            f.write(content)


_DEVICE_FILE_NAMES = {
    "nul", "con", "prn", "aux",
    *(f"com{i}" for i in range(1, 10)),
    *(f"lpt{i}" for i in range(1, 10)),
}


def _snapshot(cwd):
    snap = {}
    for root, dirs, files in os.walk(cwd):
        dirs[:] = [d for d in dirs if d not in ("node_modules", ".git", ".agent-memory", ".deep_agents", ".next", "dist")]
        for fn in files:
            # Windows reserved device names (nul, con, ...) can be created by
            # shell redirects; abspath maps them to \\.\<name> and relpath then
            # raises (different mount). They are never real project files.
            if fn.lower().split(".")[0] in _DEVICE_FILE_NAMES:
                continue
            full = os.path.join(root, fn)
            try:
                rel = os.path.relpath(full, cwd)
                snap[rel] = (os.path.getsize(full), hash(open(full, "rb").read()))
            except Exception:
                # Unreadable/racy entry: skip — the snapshot only feeds
                # change detection, never correctness.
                continue
    return snap


def _files_changed(before, after):
    return sorted(set(after) - set(before))


def _files_diff(before, after):
    """Per-file change list between two snapshots, for hollow-run detection.
    Returns (added, modified, deleted) — excluding harness noise files
    (bsa_telemetry.jsonl is written by the BSA harness, package-lock.json by npm)."""
    noise = {"bsa_telemetry.jsonl", "package-lock.json", "_hidden_failure.log", "_verify_out.txt"}
    before_keys = {k for k in before if k not in noise}
    after_keys = {k for k in after if k not in noise}
    added = sorted(after_keys - before_keys)
    deleted = sorted(before_keys - after_keys)
    modified = sorted(k for k in before_keys & after_keys if before[k] != after[k])
    return added, modified, deleted


_REPORT_NOISE = {"_ops_out.txt", "_hidden_failure.log", "_verify_out.txt", "_next_server.log",
                 "_ops_test.js", "package-lock.json", "bsa_telemetry.jsonl", "server_port.json"}


def _is_noise_path(rel):
    """True for harness/OS artifacts that must never count as agent file changes."""
    low = str(rel or "").replace("/", "\\").lower()
    base = os.path.basename(low)
    if base.split(".")[0] in _DEVICE_FILE_NAMES:
        return True
    if base in _REPORT_NOISE:
        return True
    return (".deep_agents\\" in low) or (".agent-memory\\" in low)


def _express_server(extra_routes=""):
    return (
        "const express = require('express');\n"
        "const app = express();\n"
        "app.use(express.json());\n"
        + extra_routes +
        "\nmodule.exports = app;\n"
    )


def _express_pkg():
    return '{"name": "eval-app", "main": "server.js", "dependencies": { "express": "^4.19.0" } }\n'


# ──────────────────────────────────────────────────────────────────────────
#  The 10 tasks  (3 web / 3 feature / 4 bugfix)
# ──────────────────────────────────────────────────────────────────────────

TASKS = [
    # ── WEB ──
    {
        "id": "web-static-site",
        "category": "web",
        "prompt": "Build a static landing page in this empty project: index.html with a hero section "
                  "(id='hero'), a features section (id='features'), and a footer; plus a linked style.css "
                  "that styles the page. Keep it simple and self-contained.",
        "setup": lambda d: _write(d, {"index.html": "", "style.css": ""}),  # placeholder; dev replaces
        "test": lambda d: _file_test(d, [
            ("index.html", 'id="hero"'), ("index.html", 'id="features"'),
            ("index.html", "<footer"), ("index.html", 'href="style.css"'),
            ("style.css", None),
        ]),
    },
    {
        "id": "web-todo",
        "category": "web",
        "prompt": "Build a client-side todo app in this empty project with exactly two files: index.html and "
                  "app.js. A user types a task in an input, clicks Add, and it appears in a list (id='task-list') "
                  "with a delete button on each item. Persist tasks in localStorage.",
        "setup": lambda d: _write(d, {"index.html": "", "app.js": ""}),
        "test": lambda d: _file_test(d, [
            ("index.html", 'id="task-list"'), ("index.html", "app.js"),
            ("app.js", "addEventListener"), ("app.js", "localStorage"),
        ]),
    },
    {
        "id": "web-server",
        "category": "web",
        "prompt": "Create a small Express server in this empty project: server.js exporting the app "
                  "(module.exports = app) with a GET /health route returning JSON {status:'ok'} and a "
                  "GET / route serving a simple HTML page. Add package.json with express as a dependency.",
        "setup": lambda d: (_write(d, {}), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/health');\n"
            "    if (r.status !== 200) throw new Error('health status ' + r.status);\n"
            "    let j = await r.json();\n"
            "    if (j.status !== 'ok') throw new Error('health body ' + JSON.stringify(j));\n"
            "    r = await fetch(base + '/');\n"
            "    let html = await r.text();\n"
            "    if (!/<html/i.test(html)) throw new Error('root not html');\n"
        )),
    },

    # ── FEATURE ──
    {
        "id": "feature-crud",
        "category": "feature",
        "prompt": "The existing Express app in server.js has GET /api/items returning an in-memory list. "
                  "Add POST /api/items (create, body {name}, return 201, 400 if name missing) and "
                  "DELETE /api/items/:id (remove, return 200, 404 if not found). Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const items = [{ id: 1, name: 'apple' }, { id: 2, name: 'banana' }];\n"
                "app.get('/api/items', (req, res) => res.json(items));\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({name:'cherry'}) });\n"
            "    if (r.status !== 201) throw new Error('POST status ' + r.status);\n"
            "    let items = await (await fetch(base + '/api/items')).json();\n"
            "    let created = items.find(i => i.name === 'cherry');\n"
            "    if (!created) throw new Error('created item not in list');\n"
            "    r = await fetch(base + '/api/items/' + created.id, { method: 'DELETE' });\n"
            "    if (r.status !== 200) throw new Error('DELETE status ' + r.status);\n"
            "    items = await (await fetch(base + '/api/items')).json();\n"
            "    if (items.some(i => i.name === 'cherry')) throw new Error('deleted item still present');\n"
            "    r = await fetch(base + '/api/items/999', { method: 'DELETE' });\n"
            "    if (r.status !== 404) throw new Error('DELETE unknown should be 404, got ' + r.status);\n"
        )),
    },
    {
        "id": "feature-profile",
        "category": "feature",
        "prompt": "The existing Express app has GET /api/users returning users [{id,name,email}]. Add "
                  "PUT /api/users/:id to update name and email: return 200 with the updated user, 404 if "
                  "the id doesn't exist, and 400 if name is empty or email is invalid. Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const users = [{ id: 1, name: 'alice', email: 'alice@example.com' }];\n"
                "app.get('/api/users', (req, res) => res.json(users));\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/users/1', { method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify({name:'alicia', email:'alicia@example.com'}) });\n"
            "    if (r.status !== 200) throw new Error('PUT status ' + r.status);\n"
            "    let users = await (await fetch(base + '/api/users')).json();\n"
            "    let u = users.find(x => x.id === 1);\n"
            "    if (!u || u.name !== 'alicia' || u.email !== 'alicia@example.com') throw new Error('not updated: ' + JSON.stringify(u));\n"
            "    r = await fetch(base + '/api/users/99', { method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify({name:'x', email:'x@y.com'}) });\n"
            "    if (r.status !== 404) throw new Error('unknown id should be 404, got ' + r.status);\n"
            "    r = await fetch(base + '/api/users/1', { method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify({name:'', email:'x@y.com'}) });\n"
            "    if (r.status !== 400) throw new Error('empty name should be 400, got ' + r.status);\n"
        )),
    },
    {
        "id": "feature-search",
        "category": "feature",
        "prompt": "The existing Express app has GET /api/users returning all users [{id,name,email}]. Add a "
                  "?q= query parameter to filter by name or email, case-insensitive (substring match). "
                  "No q => all users. Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const users = [{ id: 1, name: 'alice', email: 'alice@example.com' }, { id: 2, name: 'bob', email: 'bob@example.com' }];\n"
                "app.get('/api/users', (req, res) => res.json(users));\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/users?q=ALICE');\n"
            "    let list = await r.json();\n"
            "    if (list.length !== 1 || list[0].name !== 'alice') throw new Error('case-insensitive q failed: ' + JSON.stringify(list));\n"
            "    r = await fetch(base + '/api/users?q=bob@example.com');\n"
            "    list = await r.json();\n"
            "    if (list.length !== 1 || list[0].name !== 'bob') throw new Error('email q failed: ' + JSON.stringify(list));\n"
            "    r = await fetch(base + '/api/users?q=zzz');\n"
            "    list = await r.json();\n"
            "    if (list.length !== 0) throw new Error('no-match should be empty: ' + JSON.stringify(list));\n"
        )),
    },

    # ── BUGFIX ──
    {
        "id": "bugfix-500",
        "category": "bugfix",
        "prompt": "GET /api/items currently returns HTTP 500 instead of the items list. Find and fix the "
                  "bug in server.js so it returns the items as JSON. Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const items = [{ id: 1, name: 'apple' }];\n"
                "app.get('/api/items', (req, res) => res.json(itsm));  // BUG: typo\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/items');\n"
            "    if (r.status !== 200) throw new Error('expected 200, got ' + r.status);\n"
            "    let list = await r.json();\n"
            "    if (!Array.isArray(list) || list.length === 0) throw new Error('not an item list: ' + JSON.stringify(list));\n"
        )),
    },
    {
        "id": "bugfix-cors",
        "category": "bugfix",
        "prompt": "Cross-origin browser requests to this Express app are blocked because responses lack "
                  "CORS headers. Add CORS support so every response includes Access-Control-Allow-Origin "
                  "(value *) and handle OPTIONS preflight. Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const users = [{ id: 1, name: 'alice' }];\n"
                "app.get('/api/users', (req, res) => res.json(users));\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/users');\n"
            "    let h = r.headers.get('access-control-allow-origin');\n"
            "    if (h !== '*') throw new Error('missing ACAO header, got ' + h);\n"
            "    r = await fetch(base + '/api/users', { method: 'OPTIONS', headers: {'Origin':'http://x.com', 'Access-Control-Request-Method':'GET'} });\n"
            "    if (r.status >= 400) throw new Error('preflight failed: ' + r.status);\n"
        )),
    },
    {
        "id": "bugfix-offbyone",
        "category": "bugfix",
        "prompt": "utils.js exports nth(items, n) which should return the n-th element using 1-based "
                  "indexing (nth(['a','b','c'], 1) === 'a'). It's currently off by one. Fix it.",
        "setup": lambda d: _write(d, {
            "utils.js": "function nth(items, n) { return items[n]; }  // BUG: off-by-one\nmodule.exports = { nth };\n",
        }),
        "test": lambda d: _require_test(d, "utils", (
            "if (m.nth([10, 20, 30], 1) !== 10) throw new Error('nth 1 got ' + m.nth([10,20,30],1));\n"
            "if (m.nth([10, 20, 30], 3) !== 30) throw new Error('nth 3 got ' + m.nth([10,20,30],3));\n"
        )),
    },
    {
        "id": "bugfix-empty",
        "category": "bugfix",
        "prompt": "POST /api/items with an empty or invalid body currently crashes with HTTP 500. Add "
                  "validation so a missing or empty name returns 400 with a clear error, and valid "
                  "requests still create the item (201). Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const items = [{ id: 1, name: 'apple' }];\n"
                "app.get('/api/items', (req, res) => res.json(items));\n"
                "app.post('/api/items', (req, res) => { items.push({ id: items.length + 1, name: req.body.name }); res.status(201).json(items); });  // BUG: no validation\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: '{}' });\n"
            "    if (r.status !== 400) throw new Error('empty body should be 400, got ' + r.status);\n"
            "    r = await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({name:'cherry'}) });\n"
            "    if (r.status !== 201) throw new Error('valid POST should be 201, got ' + r.status);\n"
            "    let items = await (await fetch(base + '/api/items')).json();\n"
            "    if (!items.some(i => i.name === 'cherry')) throw new Error('cherry not created');\n"
        )),
    },

    # ── BUGFIX-HARD: tricky bugs agents commonly miss or get wrong ──
    # These hidden tests assert CORRECT VALUES, not just "doesn't crash" — the buggy
    # code produces wrong-but-non-crashing output, so the agent must actually reason
    # about correctness (its own self-tests often miss exactly this class of bug).
    {
        "id": "bug-falsy-zero",
        "category": "bugfix-hard",
        "prompt": "utils.js exports at(items, i) which should return items[i]. It currently returns "
                  "null for index 0 — a classic falsy-zero bug (if (!i) treats 0 as missing). Fix it so "
                  "at(items, 0) returns the first element.",
        "setup": lambda d: _write(d, {"utils.js": "function at(items, i) { if (!i) return null; return items[i]; }  // BUG: falsy 0\nmodule.exports = { at };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "if (m.at(['a','b','c'], 0) !== 'a') throw new Error('at 0 got ' + m.at(['a','b','c'],0));\n"
            "if (m.at(['a','b','c'], 2) !== 'c') throw new Error('at 2 got ' + m.at(['a','b','c'],2));\n"
            "if (m.at([], 0) !== undefined) throw new Error('at empty 0 got ' + m.at([],0));\n"
        )),
    },
    {
        "id": "bug-race-state",
        "category": "bugfix-hard",
        "prompt": "utils.js exports nextId() which should return 1, then 2, then 3 on successive calls "
                  "(a counter). It currently always returns 1 — the counter is being reset every call. "
                  "Fix the state so the counter persists across calls.",
        "setup": lambda d: _write(d, {"utils.js": "function nextId() { let count = 0; count += 1; return count; }  // BUG: count reset each call\nmodule.exports = { nextId };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "if (m.nextId() !== 1) throw new Error('1st call got ' + m.nextId());\n"
            "if (m.nextId() !== 2) throw new Error('2nd call got ' + m.nextId());\n"
            "if (m.nextId() !== 3) throw new Error('3rd call got ' + m.nextId());\n"
        )),
    },
    {
        "id": "bug-case-dup",
        "category": "bugfix-hard",
        "prompt": "utils.js exports uniq(items) which should de-duplicate case-insensitively: "
                  "uniq(['A','a','B']) must be ['A','B']. It currently keeps both 'A' and 'a'. Fix it "
                  "to treat values as equal ignoring case, keeping the FIRST occurrence.",
        "setup": lambda d: _write(d, {"utils.js": "function uniq(items) { return items.filter((v,i,a) => a.indexOf(v) === i); }  // BUG: case-sensitive\nmodule.exports = { uniq };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const r = m.uniq(['A','a','B','b']);\n"
            "if (r.length !== 2 || r[0] !== 'A' || r[1] !== 'B') throw new Error('uniq got ' + JSON.stringify(r));\n"
        )),
    },
    {
        "id": "bug-slice-off",
        "category": "bugfix-hard",
        "prompt": "utils.js exports take(items, n) which should return the first n elements "
                  "(take([1,2,3,4], 2) === [1,2]). It's off by one — it returns n-1 elements. Fix it.",
        "setup": lambda d: _write(d, {"utils.js": "function take(items, n) { return items.slice(0, n - 1); }  // BUG: off-by-one\nmodule.exports = { take };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const a = m.take([1,2,3,4], 2);\n"
            "if (a.length !== 2 || a[0] !== 1 || a[1] !== 2) throw new Error('take 2 got ' + JSON.stringify(a));\n"
            "const b = m.take([1,2,3], 1);\n"
            "if (b.length !== 1 || b[0] !== 1) throw new Error('take 1 got ' + JSON.stringify(b));\n"
        )),
    },
    {
        "id": "bug-sort-inverted",
        "category": "bugfix-hard",
        "prompt": "utils.js exports sortAsc(items) which should sort ascending. It currently sorts "
                  "descending (the comparator is inverted). Fix it to return [1,2,3] for [3,1,2].",
        "setup": lambda d: _write(d, {"utils.js": "function sortAsc(items) { return [...items].sort((a, b) => b - a); }  // BUG: inverted comparator\nmodule.exports = { sortAsc };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const r = m.sortAsc([3,1,2]);\n"
            "if (r.join(',') !== '1,2,3') throw new Error('sortAsc got ' + JSON.stringify(r));\n"
        )),
    },
    {
        "id": "bug-null-guard",
        "category": "bugfix-hard",
        "prompt": "utils.js exports getName(user) which should return user.name, and must NOT throw "
                  "when user is null/undefined (return null in that case). It currently crashes on null. "
                  "Add a guard.",
        "setup": lambda d: _write(d, {"utils.js": "function getName(user) { return user.name; }  // BUG: crashes on null\nmodule.exports = { getName };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "let threw = false;\n"
            "try { m.getName(null); } catch (e) { threw = true; }\n"
            "if (threw) throw new Error('getName(null) threw');\n"
            "if (m.getName({name:'alice'}) !== 'alice') throw new Error('getName got ' + m.getName({name:'alice'}));\n"
        )),
    },
    {
        "id": "bug-double-add",
        "category": "bugfix-hard",
        "prompt": "utils.js exports add(a, b) which should return a + b. It currently adds b twice "
                  "(add(2,3) returns 8 instead of 5). Fix the arithmetic.",
        "setup": lambda d: _write(d, {"utils.js": "function add(a, b) { return a + b + b; }  // BUG: adds b twice\nmodule.exports = { add };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "if (m.add(2,3) !== 5) throw new Error('add(2,3) got ' + m.add(2,3));\n"
            "if (m.add(-1,1) !== 0) throw new Error('add(-1,1) got ' + m.add(-1,1));\n"
        )),
    },
    {
        "id": "bug-empty-avg",
        "category": "bugfix-hard",
        "prompt": "utils.js exports avg(nums) which should return the mean of a numeric array, and "
                  "return 0 for an empty array. It currently returns NaN for avg([]) because it "
                  "divides by zero. Fix it.",
        "setup": lambda d: _write(d, {"utils.js": "function avg(nums) { return nums.reduce((a,b) => a + b, 0) / nums.length; }  // BUG: NaN on empty\nmodule.exports = { avg };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "if (m.avg([]) !== 0) throw new Error('avg([]) got ' + m.avg([]));\n"
            "if (m.avg([2,4]) !== 3) throw new Error('avg([2,4]) got ' + m.avg([2,4]));\n"
        )),
    },
    {
        "id": "bug-regression",
        "category": "bugfix-hard",
        "prompt": "server.js has two endpoints: GET /api/items (currently returns HTTP 500 due to a "
                  "typo) and GET /api/users (works correctly). Fix GET /api/items so it returns the "
                  "items list as JSON. CRITICAL: do NOT break GET /api/users — both endpoints must "
                  "keep working after your fix. Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": _express_server(
                "const items = [{ id: 1, name: 'apple' }];\n"
                "const users = [{ id: 1, name: 'alice' }, { id: 2, name: 'bob' }];\n"
                "app.get('/api/items', (req, res) => res.json(itsm));  // BUG: typo -> 500\n"
                "app.get('/api/users', (req, res) => res.json(users));\n"),
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/items');\n"
            "    if (r.status !== 200) throw new Error('items expected 200, got ' + r.status);\n"
            "    let items = await r.json();\n"
            "    if (!Array.isArray(items) || items.length === 0) throw new Error('items not a list: ' + JSON.stringify(items));\n"
            "    r = await fetch(base + '/api/users');  // regression check\n"
            "    if (r.status !== 200) throw new Error('users REGRESSED! expected 200, got ' + r.status);\n"
            "    let users = await r.json();\n"
            "    if (!Array.isArray(users) || users.length !== 2) throw new Error('users list wrong: ' + JSON.stringify(users));\n"
        )),
    },
    {
        "id": "bug-swap-args",
        "category": "bugfix-hard",
        "prompt": "utils.js exports divide(a, b) which should return a / b. It currently has the "
                  "arguments swapped — divide(10, 2) returns 0.2 instead of 5. Fix it.",
        "setup": lambda d: _write(d, {"utils.js": "function divide(a, b) { return b / a; }  // BUG: swapped args\nmodule.exports = { divide };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "if (m.divide(10, 2) !== 5) throw new Error('divide(10,2) got ' + m.divide(10,2));\n"
            "if (m.divide(9, 3) !== 3) throw new Error('divide(9,3) got ' + m.divide(9,3));\n"
        )),
    },

    # ── BUGFIX-DEV: realistic bugs software developers actually hit ──
    # Async ordering, loop closures, escaping/parsing, mutation aliasing, swallowed
    # errors, pagination, money parsing, deep copy, Express middleware order.
    {
        "id": "bug2-async-order",
        "category": "bugfix-dev",
        "prompt": "utils.js exports async mapOrdered(items, fn) which must return the results in the "
                  "same order as items (using Promise.all). It currently returns an array of Promises "
                  "because it's missing Promise.all. Fix it.",
        "setup": lambda d: _write(d, {"utils.js": "async function mapOrdered(items, fn) { return items.map(item => fn(item)); }  // BUG: missing Promise.all\nmodule.exports = { mapOrdered };\n"}),
        "test": lambda d: _require_async_test(d, "utils", (
            "    const r = await m.mapOrdered([3,1,2], async (x) => x * 2);\n"
            "    if (r.join(',') !== '6,2,4') throw new Error('order wrong: ' + JSON.stringify(r));\n"
        )),
    },
    {
        "id": "bug2-closure-loop",
        "category": "bugfix-dev",
        "prompt": "utils.js exports createTasks(n) which should return n functions, where the k-th "
                  "function returns k. It currently returns n functions that ALL return n — a classic "
                  "loop-closure bug (var in a for loop captures the final value). Fix it.",
        "setup": lambda d: _write(d, {"utils.js": "function createTasks(n) { var tasks = []; for (var i = 0; i < n; i++) { tasks.push(function () { return i; }); } return tasks; }  // BUG: var closure\nmodule.exports = { createTasks };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const t = m.createTasks(3);\n"
            "if (t[0]() !== 0) throw new Error('t0 got ' + t[0]());\n"
            "if (t[1]() !== 1) throw new Error('t1 got ' + t[1]());\n"
            "if (t[2]() !== 2) throw new Error('t2 got ' + t[2]());\n"
        )),
    },
    {
        "id": "bug2-csv-parse",
        "category": "bugfix-dev",
        "prompt": "utils.js exports parseCsv(line) which should split a CSV line into fields, "
                  "handling quoted fields that contain commas (parseCsv('a,\"b,c\",d') must be "
                  "['a','b,c','d']). It currently naively splits on every comma and breaks quoted "
                  "fields. Fix it to respect double quotes.",
        "setup": lambda d: _write(d, {"utils.js": "function parseCsv(line) { return line.split(','); }  // BUG: breaks quoted fields\nmodule.exports = { parseCsv };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const r = m.parseCsv('a,\"b,c\",d');\n"
            "if (r.length !== 3 || r[0] !== 'a' || r[1] !== 'b,c' || r[2] !== 'd') throw new Error('csv got ' + JSON.stringify(r));\n"
            "const r2 = m.parseCsv('x,y');\n"
            "if (r2.length !== 2) throw new Error('simple csv got ' + JSON.stringify(r2));\n"
        )),
    },
    {
        "id": "bug2-html-escape",
        "category": "bugfix-dev",
        "prompt": "utils.js exports escapeHtml(s) which must HTML-escape &, <, >, \" and '. It "
                  "currently only escapes < and >, leaving & and quotes — a security (XSS) hole. "
                  "Fix it to escape all five.",
        "setup": lambda d: _write(d, {"utils.js": "function escapeHtml(s) { return s.replace(/</g, '&lt;').replace(/>/g, '&gt;'); }  // BUG: misses & and quotes\nmodule.exports = { escapeHtml };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const r = m.escapeHtml('<a href=\"x\">&');\n"
            "if (r !== '&lt;a href=&quot;x&quot;&gt;&amp;') throw new Error('escaped got ' + r);\n"
            "if (r.indexOf('&') !== -1 && r.indexOf('&amp;') !== 0) { /* r already checked */ }\n"
        )),
    },
    {
        "id": "bug2-sort-mutates",
        "category": "bugfix-dev",
        "prompt": "utils.js exports getSorted(arr) which should return a sorted COPY, leaving the "
                  "original array unchanged. It currently sorts in place and mutates its input. Fix it "
                  "so the original array is not modified.",
        "setup": lambda d: _write(d, {"utils.js": "function getSorted(arr) { return arr.sort((a, b) => a - b); }  // BUG: mutates input\nmodule.exports = { getSorted };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const orig = [3,1,2];\n"
            "const sorted = m.getSorted(orig);\n"
            "if (sorted.join(',') !== '1,2,3') throw new Error('sorted got ' + JSON.stringify(sorted));\n"
            "if (orig.join(',') !== '3,1,2') throw new Error('ORIGINAL MUTATED: ' + JSON.stringify(orig));\n"
        )),
    },
    {
        "id": "bug2-swallowed-error",
        "category": "bugfix-dev",
        "prompt": "utils.js exports fetchWithRetry(fetcher) which must return the fetcher's result, "
                  "and must REJECT (propagate) if fetcher throws — it should NOT swallow the error. "
                  "It currently catches errors and returns null, silently hiding failures. Fix it to "
                  "rethrow.",
        "setup": lambda d: _write(d, {"utils.js": "async function fetchWithRetry(fetcher) { try { return await fetcher(); } catch (e) { return null; } }  // BUG: swallows error\nmodule.exports = { fetchWithRetry };\n"}),
        "test": lambda d: _require_async_test(d, "utils", (
            "    if ((await m.fetchWithRetry(async () => 42)) !== 42) throw new Error('success path failed');\n"
            "    let rejected = false;\n"
            "    try { await m.fetchWithRetry(async () => { throw new Error('boom'); }); } catch (e) { rejected = true; }\n"
            "    if (!rejected) throw new Error('error was swallowed instead of rejecting');\n"
        )),
    },
    {
        "id": "bug2-pagination",
        "category": "bugfix-dev",
        "prompt": "utils.js exports paginate(items, page, size) which should return the items for "
                  "that 1-based page (paginate(['a','b','c','d'], 1, 2) === ['a','b']). It's off by "
                  "one — page 1 skips the first item. Fix it.",
        "setup": lambda d: _write(d, {"utils.js": "function paginate(items, page, size) { const start = (page - 1) * size; return items.slice(start + 1, start + size + 1); }  // BUG: off-by-one\nmodule.exports = { paginate };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const items = ['a','b','c','d','e'];\n"
            "const p1 = m.paginate(items, 1, 2);\n"
            "if (p1.join(',') !== 'a,b') throw new Error('page1 got ' + JSON.stringify(p1));\n"
            "const p2 = m.paginate(items, 2, 2);\n"
            "if (p2.join(',') !== 'c,d') throw new Error('page2 got ' + JSON.stringify(p2));\n"
        )),
    },
    {
        "id": "bug2-money-parse",
        "category": "bugfix-dev",
        "prompt": "utils.js exports sumPrices(prices) which should sum currency strings like "
                  "'$3.50' (sumPrices(['$3.50','$1.25']) === 4.75). It currently uses parseFloat "
                  "directly on the string, which returns NaN for '$3.50'. Fix it to strip non-numeric "
                  "characters first.",
        "setup": lambda d: _write(d, {"utils.js": "function sumPrices(prices) { return prices.reduce((a, p) => a + parseFloat(p), 0); }  // BUG: parseFloat('$3.50') = NaN\nmodule.exports = { sumPrices };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const r = m.sumPrices(['$3.50', '$1.25']);\n"
            "if (!isFinite(r) || Math.abs(r - 4.75) > 0.001) throw new Error('sum got ' + r);\n"
        )),
    },
    {
        "id": "bug2-deep-copy",
        "category": "bugfix-dev",
        "prompt": "utils.js exports clone(obj) which must return a DEEP copy — mutating the clone's "
                  "nested objects must not affect the original. It currently does a shallow copy "
                  "({...obj}) so nested arrays/objects are shared. Fix it to deep-copy.",
        "setup": lambda d: _write(d, {"utils.js": "function clone(obj) { return { ...obj }; }  // BUG: shallow copy, nested refs shared\nmodule.exports = { clone };\n"}),
        "test": lambda d: _require_test(d, "utils", (
            "const a = { list: [1, 2], name: 'x' };\n"
            "const b = m.clone(a);\n"
            "b.list.push(3);\n"
            "if (a.list.length !== 2) throw new Error('original mutated by clone: ' + JSON.stringify(a.list));\n"
            "if (b.name !== 'x') throw new Error('clone lost fields');\n"
        )),
    },
    {
        "id": "bug2-middleware-order",
        "category": "bugfix-dev",
        "prompt": "server.js is an Express app where POST /api/items reads req.body, but the JSON body "
                  "parser (express.json()) is registered AFTER the route — so req.body is undefined "
                  "and the request 500s. Move the middleware BEFORE the routes so POST /api/items "
                  "returns the parsed body. Keep module.exports = app.",
        "setup": lambda d: (_write(d, {
            "server.js": "const express = require('express');\n"
                         "const app = express();\n"
                         "app.post('/api/items', (req, res) => { res.json({ received: req.body.name }); });  // BUG: body undefined\n"
                         "app.use(express.json());  // TOO LATE — after the route\n"
                         "module.exports = app;\n",
            "package.json": _express_pkg(),
        }), _ensure_deps(d)),
        "test": lambda d: _api_test(d, (
            "    let r = await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({name:'cherry'}) });\n"
            "    if (r.status !== 200) throw new Error('POST status ' + r.status);\n"
            "    let j = await r.json();\n"
            "    if (j.received !== 'cherry') throw new Error('body not parsed: ' + JSON.stringify(j));\n"
        )),
    },
]
TASKS_BY_ID = {t["id"]: t for t in TASKS}

# ── gen2 suite: 60 tasks across 4 skill axes (bugfix/refactor/python/multifile) ──
try:
    from gen2_tasks import build_tasks as _build_gen2_tasks
    GEN2_TASKS = _build_gen2_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
        "py_test": _py_test,
        "refactor_test": _refactor_test,
    })
    TASKS = TASKS + GEN2_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen2 suite registered: {len(GEN2_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN2_TASKS))})", flush=True)
except Exception as _gen2_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen2 suite NOT loaded ({_gen2_err})", flush=True)

# ── gen3 suite: traps / chains / log-debug / long-horizon ──
try:
    from gen3_tasks import build_tasks as _build_gen3_tasks
    GEN3_TASKS = _build_gen3_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
        "refactor_test": _refactor_test,
    })
    TASKS = TASKS + GEN3_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen3 suite registered: {len(GEN3_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN3_TASKS))})", flush=True)
except Exception as _gen3_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen3 suite NOT loaded ({_gen3_err})", flush=True)

# ── gen4 suite: harder traps / chains / logs / generated long-horizon ──
try:
    from gen4_tasks import build_tasks as _build_gen4_tasks
    GEN4_TASKS = _build_gen4_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
        "refactor_test": _refactor_test,
    })
    TASKS = TASKS + GEN4_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen4 suite registered: {len(GEN4_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN4_TASKS))})", flush=True)
except Exception as _gen4_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen4 suite NOT loaded ({_gen4_err})", flush=True)

# ── gen5: THE CAPSTONE — one big enterprise build, all agents required ──
try:
    from gen5_tasks import build_tasks as _build_gen5_tasks
    GEN5_TASKS = _build_gen5_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
    })
    TASKS = TASKS + GEN5_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen5 suite registered: {len(GEN5_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN5_TASKS))})", flush=True)
except Exception as _gen5_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen5 suite NOT loaded ({_gen5_err})", flush=True)

# ── gen6: THE E-COMMERCE CAPSTONE — complete store, all agents required ──
try:
    from gen6_tasks import build_tasks as _build_gen6_tasks
    GEN6_TASKS = _build_gen6_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
    })
    TASKS = TASKS + GEN6_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen6 suite registered: {len(GEN6_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN6_TASKS))})", flush=True)
except Exception as _gen6_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen6 suite NOT loaded ({_gen6_err})", flush=True)

# ── gen7: local terminal-style ops tasks (Terminal-Bench flavor, no Docker) ──
try:
    from gen7_ops_tasks import build_tasks as _build_gen7_tasks
    GEN7_TASKS = _build_gen7_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
        "py_test": _py_test,
        "refactor_test": _refactor_test,
        "ops_test": _ops_test,
        "file_test": _file_test,
    })
    TASKS = TASKS + GEN7_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen7 suite registered: {len(GEN7_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN7_TASKS))})", flush=True)
except Exception as _gen7_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen7 suite NOT loaded ({_gen7_err})", flush=True)

# ── SWE-bench-style USER prompts (vague human issue reports) — override with --style user ──
try:
    from hard_prompts import USER_PROMPTS as _USER_PROMPTS
except ImportError:
    _USER_PROMPTS = {}

# ── gen9-real: 18 real-world bug tasks (SWE-bench methodology) ──
# Frozen repo snapshots at real base commits; hidden tests = the upstream fix
# commit's own tests. setup(d) handles snapshot copy + venv/node_modules
# junctions + per-task budget override, so the existing runner flow is reused.
try:
    from gen9_real_tasks import build_tasks as _build_gen9_tasks
    GEN9_TASKS = _build_gen9_tasks({})
    TASKS = TASKS + GEN9_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen9-real suite registered: {len(GEN9_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN9_TASKS))})", flush=True)
except Exception as _gen9_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen9-real suite NOT loaded ({_gen9_err})", flush=True)

# ── gen10: LONG-HORIZON single-agent-loop suite (landing / refine / build) ──
# One long multi-phase prompt per task -> long Dev session (context growth,
# compaction, many tool iterations). Budgets are applied in setup(). The hidden
# test is a lenient functional gate; quality is judged after the run via
# scratch/gen10_review.py (design / structure / perf / scale / maintain / security).
try:
    from gen10_tasks import build_tasks as _build_gen10_tasks
    GEN10_TASKS = _build_gen10_tasks({
        "write": _write,
        "ensure_deps": _ensure_deps,
        "require_test": _require_test,
        "require_async_test": _require_async_test,
        "api_test": _api_test,
        "py_test": _py_test,
        "refactor_test": _refactor_test,
        "check_structure": _check_structure,
        "ops_test": _ops_test,
        "file_test": _file_test,
    })
    TASKS = TASKS + GEN10_TASKS
    TASKS_BY_ID = {t["id"]: t for t in TASKS}
    print(f"[eval] gen10 suite registered: {len(GEN10_TASKS)} tasks "
          f"({sorted(set(t['category'] for t in GEN10_TASKS))})", flush=True)
except Exception as _gen10_err:
    import traceback
    traceback.print_exc()
    print(f"[eval] WARNING: gen10 suite NOT loaded ({_gen10_err})", flush=True)


# ──────────────────────────────────────────────────────────────────────────
#  Single-task runner
# ──────────────────────────────────────────────────────────────────────────

def _read_bsa_telemetry(path):
    if not os.path.isfile(path):
        return []
    rows = []
    for line in open(path, encoding="utf-8"):
        try:
            rows.append(json.loads(line))
        except Exception:
            pass
    return rows


def _tools_from_calls(calls):
    """Aggregate executed-tool counts from each call's structured tool_counts.

    The old implementation parsed the human-readable ``where`` tag and almost
    always returned {} — the real counts live in ``extra.tool_counts`` written
    by llm/native_tools.py.
    """
    c = collections.Counter()
    for call in calls:
        tc = call.get("tool_counts")
        if isinstance(tc, dict):
            for k, v in tc.items():
                if isinstance(v, int) and v > 0:
                    c[str(k)] += v
    return dict(c)


def _tasks_dir() -> str:
    """Central task-tracking dir used by sync_helpers (keyed by chat_id, NOT per project)."""
    return os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), ".deep_agents", "tasks")


def _build_repair_prompt(base_prompt: str, prev_attempt: dict, detail: str, round_no: int) -> str:
    """Build the repair-round prompt: base task + the hidden-test failure + a handoff
    of what the PREVIOUS attempt already produced.

    `prev_attempt` = {"files": [relpaths], "spec": bool, "report": str} or None.
    The handoff tells the orchestrator the project already has a spec + code, so it
    should NOT re-delegate BSA or rebuild from scratch — it should fix the failure.
    """
    lines = [base_prompt]
    lines.append(
        f"\n\n[VERIFICATION FAILED — REPAIR {round_no}] A hidden test suite ran against your previous "
        f"attempt and it FAILED with:\n{detail}\n"
    )
    lines.append(
        "[GROUND TRUTH] The hidden-test assertion above IS the specification — it shows the EXACT "
        "values the app must produce. Match them literally, even where they differ from your "
        "judgment of 'correct' behavior. If the assertion expects `done: 1` after a retry, the "
        "implementation must produce `done: 1` — do not 'improve' the semantics to what you think "
        "is right, and do not theorize about other edge cases. REPRODUCE the failure first (run "
        "the server / the test command and read the real traceback), then make the failing check "
        "pass with the minimal change. If your previous change is already in place, read your own "
        "diff for a subtle mismatch (wrong status code, wrong field, extra/missing side effect)."
    )
    if prev_attempt:
        lines.append("[PREVIOUS ATTEMPT STATE]")
        if prev_attempt.get("spec"):
            lines.append("- A spec already exists at specs/SPEC-*.md — do NOT re-delegate BSA; reuse it.")
        files = prev_attempt.get("files") or []
        if files:
            # Compact inventory: top-level dirs + root files (keep the prompt small)
            dirs = sorted({f.split("/")[0] + "/" for f in files if "/" in f})
            roots = sorted(f for f in files if "/" not in f)
            bits = dirs[:12] + roots[:12]
            more = len(dirs) + len(roots) - len(bits)
            listing = ", ".join(bits)
            if more > 0:
                listing += f" (+{more} more)"
            lines.append(f"- Existing files ({len(files)} total): {listing}")
        report = (prev_attempt.get("report") or "").strip()
        if report:
            lines.append(f"- Last agent report: {report[:600]}")
        tools = prev_attempt.get("tools") or {}
        if tools:
            tool_bits = ", ".join(f"{k} x{v}" for k, v in sorted(tools.items())[:12])
            lines.append(f"- Prior activity (tools used): {tool_bits}")
        changed = prev_attempt.get("files_changed") or []
        if changed:
            lines.append(f"- Files changed last attempt: {', '.join(changed[:15])}")
        lines.append(
            "[REPAIR MODE] The previous attempt's code is still on disk and mostly correct. "
            "Do NOT rebuild from scratch and do NOT re-delegate spec work (BSA). Read the "
            "existing files, find the root cause of the failing check above, and apply the "
            "MINIMAL fix. Do not redo work that already passes — only fix the reported failure."
        )
    else:
        lines.append(
            "Reuse the existing project state, find the root cause, and fix it. Do not redo "
            "work that already passes — only fix the reported failure."
        )
    return "\n".join(lines)


def _kill_stray_servers(cwd):
    """Kill node/python server processes left over from PRIOR attempts that
    reference this workspace (command line contains the tmp path). A leftover
    server holds the task's port → the next attempt's server hits EADDRINUSE
    and the hidden test never sees 'listening on <port>'. Process cleanup
    only — never touches files. Called at the start of every attempt and
    again right before the hidden test runs (the attempt's OWN background
    server is the usual straggler — node.exe's command line does not contain
    the workspace path, so a port-based kill is required)."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    try:
        ps_cmd = (
            "Get-CimInstance Win32_Process | Where-Object { "
            "$_.CommandLine -like '*" + cwd + "*' -and "
            "($_.Name -eq 'node.exe' -or $_.Name -eq 'python.exe' -or "
            "$_.Name -eq 'python3.exe' -or $_.Name -eq 'pythonw.exe') } | "
            "ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }"
        )
        subprocess.run(["powershell", "-NoProfile", "-Command", ps_cmd],
                       capture_output=True, text=True, timeout=20, creationflags=flags)
    except Exception:
        pass
    # Port-based kill: node/python listeners on the workspace's declared ports.
    # node.exe spawned by `npm start` keeps the port but its command line has no
    # path match — only a port kill reaches it. Restricted to node/python.
    ports = set()
    spj = os.path.join(cwd, "server_port.json")
    if os.path.isfile(spj):
        try:
            import json as _json
            _p = _json.load(open(spj, encoding="utf-8"))
            if isinstance(_p, dict) and _p.get("port"):
                ports.add(int(_p["port"]))
        except Exception:
            pass
    ports.add(3000)
    for port in ports:
        try:
            ps_cmd = (
                f"Get-NetTCPConnection -LocalPort {port} -State Listen -ErrorAction SilentlyContinue | "
                f"ForEach-Object {{ $pid_ = $_.OwningProcess; "
                f"$proc = Get-Process -Id $pid_ -ErrorAction SilentlyContinue; "
                f"if ($proc -and ($proc.ProcessName -match 'node|python')) {{ "
                f"Stop-Process -Id $pid_ -Force -ErrorAction SilentlyContinue }} }}"
            )
            subprocess.run(["powershell", "-NoProfile", "-Command", ps_cmd],
                           capture_output=True, text=True, timeout=20, creationflags=flags)
        except Exception:
            pass


def _cleanup_temp_scripts(cwd, baseline_snap):
    """Harness-side cleanup of agent temp verify scripts — the agent's
    write→run→del cycle becomes write→run with ZERO LLM cost for the delete.
    Only files CREATED during the attempt matching temp-script patterns are
    removed; setup files and deliverables are untouched."""
    import fnmatch as _fnmatch
    patterns = ("verify*.js", "verify*.py", "probe*.js", "probe*.py", "selfcheck*.js",
                "check*.js", "check*.py", "repro*.js", "repro*.py", "smoke*.js", "smoke*.py",
                "test*.js", "test*.mjs", "test*.py", "*_test.js", "*_test.py", "*_test.mjs")
    try:
        for root, dirs, files in os.walk(cwd):
            dirs[:] = [d for d in dirs if d not in ("node_modules", ".git", ".agent-memory", ".deep_agents", ".next", "dist")]
            for fn in files:
                rel = os.path.relpath(os.path.join(root, fn), cwd)
                if rel in baseline_snap:
                    continue  # not created this attempt
                if any(_fnmatch.fnmatch(fn, p) for p in patterns):
                    try:
                        os.remove(os.path.join(root, fn))
                    except OSError:
                        pass
    except Exception:
        pass


def _git_init(cwd):
    """Init a git repo with an initial commit so the harness can diff the
    agent's changes: Dev's session-end diff message activates, and the Dev
    repair package receives the prior attempt's diff (the 'lift from latest
    test' context). Noise files are gitignored so diffs stay clean."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    try:
        with open(os.path.join(cwd, ".gitignore"), "w", encoding="utf-8") as f:
            f.write("node_modules/\n"
                    ".next/\ndist/\n"
                    ".agent-memory/\n.deep_agents/\n"
                    "_hidden_test.js\n_hidden_test.py\n_hidden_failure.log\n"
                    "_verify_out.txt\n_ops_out.txt\n_ops_test.js\n"
                    "bsa_telemetry.jsonl\npackage-lock.json\n")
        subprocess.run(["git", "init", "-q"], cwd=cwd, capture_output=True, timeout=30, creationflags=flags)
        subprocess.run(["git", "add", "-A"], cwd=cwd, capture_output=True, timeout=30, creationflags=flags)
        subprocess.run(["git", "-c", "user.email=harness@local", "-c", "user.name=harness",
                        "commit", "-q", "-m", "init"], cwd=cwd, capture_output=True, timeout=60,
                       creationflags=flags)
    except Exception:
        pass


def _git_diff(cwd, max_chars=1200):
    """Return the working-tree diff vs HEAD (the agent's uncommitted changes),
    with -U2 compact context so diffs stay token-lean. READ-ONLY."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    try:
        r = subprocess.run(["git", "diff", "-U2", "HEAD", "--", "."], cwd=cwd,
                           capture_output=True, text=True, timeout=30, creationflags=flags)
        if r.returncode == 0 and r.stdout.strip():
            return r.stdout.strip()[:max_chars]
    except Exception:
        pass
    return ""


def _git_status_block(cwd, max_lines=8):
    """Compact workspace-state block from `git status --porcelain=v2` — the
    agent sees what changed without running prose git status. READ-ONLY."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    try:
        r = subprocess.run(["git", "status", "--porcelain=v2"], cwd=cwd,
                           capture_output=True, text=True, timeout=20, creationflags=flags)
        if r.returncode != 0:
            return ""
        out = []
        for l in r.stdout.splitlines():
            l = l.strip()
            if not l:
                continue
            if l.startswith("?"):
                out.append("untracked: " + l[2:])
            elif l.startswith(("1 ", "2 ")):
                parts = l.split(" ", 8)
                if len(parts) >= 9:
                    out.append(f"{parts[1]} {parts[8]}")
            elif l.startswith("u "):
                out.append("conflict: " + l[2:])
            if len(out) >= max_lines:
                out.append("...")
                break
        return "\n".join(out)
    except Exception:
        return ""


def _purge_stale_eval_tasks(task_id: str = None) -> int:
    """Delete stale eval task-tracking files from the central .deep_agents/tasks/ dir.

    Task tracking is keyed globally by chat_id (sync_helpers.load_task_tracking), so an
    old eval run can leave a task_eval-<id>-r<n>.json with status "completed" behind.
    The next run reuses the same chat_id → sync_manager injects a stale
    "=== TASK PROGRESS (1/1 steps) === [x] Step 1: ... done" block into the orchestrator
    prompt → the orchestrator trusts it and never delegates → instant hidden-test fail.
    """
    tasks_dir = _tasks_dir()
    if not os.path.isdir(tasks_dir):
        return 0
    pattern = f"task_eval-{task_id}-*.json" if task_id else "task_eval-*.json"
    removed = 0
    for f in glob.glob(os.path.join(tasks_dir, pattern)):
        try:
            os.remove(f)
            removed += 1
        except OSError:
            pass
    return removed


def _prompt_for(task):
    """Resolve the prompt for this task: SWE-bench-style USER report (--style user)
    if one exists for the task, otherwise the original spec-style prompt."""
    if os.environ.get("BSA_EVAL_STYLE", "spec") == "user":
        p = _USER_PROMPTS.get(task["id"])
        if p:
            return p
    return task["prompt"]


def run_single(task_id, keep=False):
    task = TASKS_BY_ID[task_id]
    # Task workspaces must NEVER land on the OS temp drive (C:) — big repos and
    # long runs would eat the C partition. BSA_EVAL_TMP defaults to D:.
    _eval_tmp_root = os.environ.get("BSA_EVAL_TMP", os.path.join(PROJECT_ROOT, "scratch", "eval_tmp"))
    try:
        os.makedirs(_eval_tmp_root, exist_ok=True)
    except Exception:
        _eval_tmp_root = tempfile.gettempdir()
    tmp = tempfile.mkdtemp(prefix=f"eval_{task_id}_", dir=_eval_tmp_root)
    # Unique chat_id per run → stale task-tracking state can never leak into this run.
    run_seed = uuid.uuid4().hex[:6]
    _purge_stale_eval_tasks(task_id)
    # Per-task call log starts clean so the review bundle contains exactly this run.
    try:
        _clog = os.environ.get("BSA_CALLS_LOG")
        if _clog and os.path.isfile(_clog):
            os.remove(_clog)
    except OSError:
        pass
    report = {"id": task_id, "category": task["category"], "status": "error"}
    try:
        global _USER_STYLE
        _USER_STYLE = os.environ.get("BSA_EVAL_STYLE", "spec") == "user"
        task["setup"](tmp)
        _git_init(tmp)  # baseline commit so agent changes are diffable
        before = _snapshot(tmp)

        os.environ["BSA_MAX_SECTIONS"] = os.environ.get("BSA_MAX_SECTIONS", "3")
        tel = os.path.join(tmp, "bsa_telemetry.jsonl")
        os.environ["BSA_TELEMETRY_FILE"] = tel

        _emit("task_start", id=task_id, category=task["category"])

        # ── Self-repair loop: if the hidden test fails, feed the failure back to the
        #    harness and let it fix (up to max_repairs). Mirrors a user reporting a bug.
        #    Hollow-run resilience: an attempt that produced NO work (no files changed,
        #    near-zero LLM cost, <=1 call — e.g. a provider 500 swallowing the delegation)
        #    is an infrastructure failure, not an agent failure. It retries with a FRESH
        #    chat context WITHOUT consuming a repair round. If every attempt is hollow,
        #    the task is reported as status="error" (infra), not "fail" (agent).
        max_repairs = int(os.environ.get("BSA_MAX_REPAIRS", "2"))
        max_hollow = int(os.environ.get("BSA_MAX_HOLLOW_RETRIES", "2"))
        hollow_cost_max = float(os.environ.get("BSA_HOLLOW_COST_MAX", "0.003"))
        # Long-horizon mode (gen10): no attempts/repairs/bailout/wall caps — the
        # hidden gate + the agent decide when the work is done.
        unlimited = os.environ.get("BSA_UNLIMITED", "0") == "1"
        ok, detail, attempts, repairs, dur = False, "", 0, 0, 0.0
        helper_repair_rounds = 0
        hollow_retries = 0
        hard_errors = 0  # consecutive developer-crash attempts (infra spin guard)
        any_work = False
        first_real_ok = None  # did the FIRST non-hollow attempt pass? (pass@1, SWE-bench style)
        attempt_log = []  # per-attempt cost/duration evidence (repair rounds are visible)
        prev_attempt = None  # {"files": [...], "spec": bool, "report": str} from last attempt
        prev_fail_detail = ""  # the hidden-test output of the last FAILING attempt (learning evidence)
        _sig_counts = {}  # normalized failure signature -> count (no-progress bail-out)
        pre_repair_content = None  # file contents at the start of the current repair attempt

        def _usage_now():
            try:
                from sync.state_sync import safe_get_state
                u = safe_get_state().get("token_usage", {}) or {}
                return float(u.get("total_cost", 0) or 0), len(u.get("calls", []) or [])
            except Exception:
                return 0.0, 0

        task_t0 = time.time()  # for the "until done" wall-cap bound
        while True:
            attempts += 1
            # Kill leftover server processes from prior attempts (EADDRINUSE
            # port conflicts break the next attempt's hidden test).
            try:
                _kill_stray_servers(tmp)
            except Exception:
                pass
            # Keep the memory code map in sync with the current on-disk code so
            # Dev's PROJECT MEMORY symbol index never goes stale between attempts.
            try:
                from memory.memory_manager import refresh_code_map
                refresh_code_map(tmp)
            except Exception:
                pass
            run_task = _prompt_for(task)
            if repairs > 0:
                run_task = _build_repair_prompt(_prompt_for(task), prev_attempt, detail, repairs)
            t0 = time.time()
            snap_before = _snapshot(tmp)
            cost_before, calls_before = _usage_now()
            # One stable session across attempts: a repair round must CONTINUE
            # the same Dev transcript (cache-warm, no cold re-derivation), not
            # restart from an empty context.
            chat_id = f"eval-{task_id}-{run_seed}"
            # Capture the file contents at the start of a REPAIR attempt so a
            # verified lesson can later diff exactly what the repair changed.
            pre_repair_content = None
            if repairs > 0 and prev_attempt and prev_attempt.get("files"):
                try:
                    pre_repair_content = {}
                    for rel in prev_attempt["files"][:20]:
                        p = os.path.join(tmp, rel)
                        if os.path.isfile(p):
                            with open(p, "r", encoding="utf-8", errors="replace") as f:
                                pre_repair_content[rel] = f.read()
                except Exception:
                    pre_repair_content = None
            last_state = None
            hard_error = False
            repair_via_helper = (
                repairs > 0
                and os.environ.get("BSA_REPAIR_VIA_HELPER", "1") != "0"
            )
            run_dev_repair = None
            if repair_via_helper:
                try:
                    from verify_helper import run_dev_repair as _rdr
                    run_dev_repair = _rdr
                except ImportError:
                    run_dev_repair = None
                if run_dev_repair is None:
                    # verify_helper may be a stub in this tree — fall through to
                    # the direct Dev repair path instead of silently breaking.
                    print("[eval] verify_helper.run_dev_repair unavailable — using direct Dev repair path", flush=True)
                    repair_via_helper = False
            if repair_via_helper:
                # DEV repair path: the harness hands the hidden-test failure
                # to the Dev subagent (full toolset), never via the
                # orchestrator. Dev reproduces, fixes and verifies internally;
                # the hidden test re-runs below and remains the only gate.
                from sync.state_sync import safe_update_state, shared_state
                try:
                    safe_update_state({"project_path": tmp, "chat_id": chat_id,
                                       "client_request": run_task})
                except Exception:
                    pass
                goal = _prompt_for(task)
                if goal != task["prompt"]:
                    goal = f"{goal}\n\n## TASK SPEC (exact requirements)\n{task['prompt']}"
                fail_log = ""
                try:
                    fl = os.path.join(tmp, "_hidden_failure.log")
                    if os.path.isfile(fl):
                        with open(fl, encoding="utf-8", errors="replace") as f:
                            fail_log = f.read()
                except Exception:
                    pass
                prev_files = sorted((prev_attempt or {}).get("files") or [])[:20]
                prior = ""
                if prev_attempt:
                    prior = ("Existing files: " + ", ".join(prev_files) + "\n"
                             + "Last agent report: " + str((prev_attempt.get("report") or ""))[:600])
                prior_diff = _git_diff(tmp)  # the 'lift from latest test' context (-U2 compact)
                prior_status = _git_status_block(tmp)  # porcelain workspace state (read-only)
                try:
                    if prior_status and prior_diff:
                        prior_diff = ("## WORKSPACE STATE (git status --porcelain=v2)\n"
                                      + prior_status + "\n\n" + prior_diff)
                    handoff = run_dev_repair(
                        goal=goal,
                        failure_detail=detail,
                        test_output=fail_log or detail,
                        files=prev_files,
                        prior_state=prior or None,
                        prior_diff=prior_diff or None,
                        project_path=tmp,
                        chat_id=chat_id,
                    )
                    helper_repair_rounds += 1
                    last_state = {"outputs": {"agent_report": str(handoff)}}
                    safe_update_state({"outputs": {
                        **shared_state.get("outputs", {}),
                        "test_report": str(detail),
                        "agent_report": str(handoff),
                    }})
                except Exception as e:
                    _emit("task_error", id=task_id, error=f"helper repair failed: {str(e)[:200]}")
                    last_state = None
                    hard_error = True
            else:
                # Fallback (BSA_REPAIR_VIA_HELPER=0): the repair prompt IS the
                # plan (failure detail + prior attempt state). Bypass the
                # orchestrator's replan/ceremony turns and feed it to Dev.
                from agents.developer_agent import developer_node
                from sync.state_sync import safe_update_state, shared_state
                try:
                    safe_update_state({"project_path": tmp, "chat_id": chat_id,
                                       "client_request": run_task})
                except Exception:
                    pass
                dev_state = {
                    "client_request": run_task,
                    "tech_spec": "",
                    "requirements": "",
                    "project_path": tmp,
                    "chat_id": chat_id,
                    "error_count": repairs,
                    "test_report": detail if repairs > 0 else "",
                }
                try:
                    res = developer_node(dev_state)
                    last_state = {"outputs": {"agent_report": str(res.get("agent_report") or "")}}
                    safe_update_state({"outputs": {
                        **shared_state.get("outputs", {}),
                        "code": res.get("code", ""),
                        "test_report": res.get("test_report", ""),
                    }})
                except Exception as e:
                    _emit("task_error", id=task_id, error=f"direct-repair dev failed: {str(e)[:200]}")
                    last_state = None
                    hard_error = True
                # NOTE (v23 benchmark policy): Dev-only single-agent loop.
                # The full orchestrator graph is intentionally NOT run here:
                # inside a task workspace the request goes straight to the
                # Dev workspace agent (same as production project_dev
                # routing); the hidden test below is the only gate. The
                # main-workspace orchestrator path is untouched.
            dur += time.time() - t0
            snap_after = _snapshot(tmp)
            cost_after, calls_after = _usage_now()
            # The attempt's OWN background server (started to verify the work) is
            # the usual EADDRINUSE straggler — kill it BEFORE the hidden test binds.
            _kill_stray_servers(tmp)
            ok, detail = task["test"](tmp)

            # ── Hollow-run detection: did the agents actually DO anything? ──
            added, modified, deleted = _files_diff(snap_before, snap_after)
            work_count = len(added) + len(modified) + len(deleted)
            cost_delta = max(0.0, cost_after - cost_before)
            calls_delta = max(0, calls_after - calls_before)
            hollow = work_count == 0 and cost_delta < hollow_cost_max and calls_delta <= 1
            if work_count > 0 or cost_delta >= hollow_cost_max:
                any_work = True
            if not hollow and first_real_ok is None:
                first_real_ok = ok

            # Capture what this attempt produced so the NEXT repair round knows the
            # existing state (files, spec, last report) instead of rebuilding blindly.
            try:
                spec_exists = os.path.isdir(os.path.join(tmp, "specs")) and any(
                    f.endswith(".md") for f in os.listdir(os.path.join(tmp, "specs")))
                last_report = ""
                if last_state:
                    last_report = (last_state.get("outputs") or {}).get("agent_report") or ""
                prev_attempt = {"files": sorted(snap_after.keys()), "spec": bool(spec_exists), "report": str(last_report),
                                "tools": report.get("tools") or {}, "files_changed": report.get("files_changed") or []}
            except Exception:
                prev_attempt = None

            _emit("attempt", id=task_id, attempt=attempts, ok=ok, detail=(detail or "")[:300],
                  hollow=hollow, work=work_count, cost_delta=round(cost_delta, 5), calls_delta=calls_delta)
            attempt_log.append({
                "attempt": attempts,
                "ok": bool(ok),
                "duration_s": round(time.time() - t0, 1),
                "cost_delta": round(cost_delta, 6),
                "calls_delta": calls_delta,
                "work": work_count,
                "detail": (detail or "")[:200],
            })
            if hard_error:
                hard_errors += 1
                if hard_errors >= 3:
                    print(f"[eval] {task_id}: developer crashed {hard_errors} attempts in a row — "
                          f"aborting task (infrastructure failure, not an agent failure)", flush=True)
                    _emit("bailout", id=task_id, signature="developer-crash-spin", count=hard_errors)
                    break
            else:
                hard_errors = 0
            _cleanup_temp_scripts(tmp, snap_before)  # zero-LLM-cost temp script cleanup
            if ok:
                # Verified-lesson learning: a repair round made the hidden test
                # pass. Evidence = the last failing output + the exact diff the
                # repair round applied. BSA_ERROR_PATTERNS=1 gates it.
                try:
                    if (os.environ.get("BSA_ERROR_PATTERNS", "0") == "1"
                            and repairs > 0 and pre_repair_content and prev_fail_detail):
                        import difflib as _difflib
                        from error_patterns import learn
                        fix_parts = []
                        for rel, old in pre_repair_content.items():
                            cur_path = os.path.join(tmp, rel)
                            if not os.path.isfile(cur_path):
                                continue
                            with open(cur_path, "r", encoding="utf-8", errors="replace") as f:
                                new = f.read()
                            if new != old:
                                dl = list(_difflib.unified_diff(
                                    old.splitlines(), new.splitlines(), lineterm="",
                                    fromfile=f"a/{rel}", tofile=f"b/{rel}", n=1))
                                fix_parts.append("\n".join(dl))
                        fix_diff = "\n".join(fix_parts)[:600]
                        if fix_diff:
                            try:
                                from memory.memory_manager import _detect_stack_summary
                                _stack = _detect_stack_summary(tmp)
                            except Exception:
                                _stack = ""
                            learn(prev_fail_detail, fix_diff, workspace=task_id, stack=_stack)
                            print(f"[eval] {task_id}: learned verified pattern "
                                  f"(repair {repairs}, {len(fix_diff)} chars of fix)", flush=True)
                except Exception as e:
                    print(f"[eval] pattern learn failed: {e}", flush=True)
                break
            prev_fail_detail = detail or prev_fail_detail
            # ── No-progress bail-out: the SAME failure signature ≥3 times means the
            # repair loop is not converging (environmental collision, unfixable task)
            # — stop repairing instead of burning identical repair sessions.
            try:
                from error_patterns import _normalize as _norm
            except Exception:
                _norm = lambda t: " ".join((t or "").lower().split())[:200]  # noqa: E731
            sig = _norm(detail or "")[:150]
            _sig_counts[sig] = _sig_counts.get(sig, 0) + 1
            max_same = int(os.environ.get("BSA_MAX_SAME_SIG", "3"))
            if (not unlimited) and _sig_counts.get(sig, 0) >= max_same:
                print(f"[eval] {task_id}: same failure signature x{_sig_counts[sig]} — "
                      f"no repair progress, stopping", flush=True)
                _emit("bailout", id=task_id, signature=sig[:120], count=_sig_counts[sig])
                break
            if hollow and hollow_retries < max_hollow:
                hollow_retries += 1
                # Provider hiccup — clear transient throttle/cooldown state so the fresh
                # attempt isn't throttled or poisoned by the previous failed call.
                try:
                    from llm import _throttle
                    _throttle.cooldown_until = 0.0
                except Exception:
                    pass
                try:
                    from sync.state_sync import shared_state, state_lock
                    with state_lock:
                        shared_state["failed_models"] = []
                except Exception:
                    pass
                _emit("hollow", id=task_id, retry=hollow_retries, work=work_count,
                      cost_delta=round(cost_delta, 5), calls_delta=calls_delta,
                      detail=(detail or "")[:200])
                print(f"[eval] {task_id} attempt {attempts} was HOLLOW (0 work, ${cost_delta:.5f}, "
                      f"{calls_delta} call(s)) — fresh-context retry {hollow_retries}/{max_hollow}", flush=True)
                time.sleep(2.0)  # brief backoff for the transient provider failure
                continue
            # ── Until-done policy (helper repair path) ──
            # The helper repair path loops until the hidden test passes, bounded
            # only by the eval wall cap (leave a margin so the report writes
            # before the parent kills the child). The direct Dev fallback keeps
            # max_repairs.
            helper_mode = os.environ.get("BSA_REPAIR_VIA_HELPER", "1") != "0"
            until_done = helper_mode and os.environ.get("BSA_REPAIR_UNTIL_DONE", "1") == "1"
            wall_cap_s = int(os.environ.get("BSA_TASK_WALL_CAP", "2400") or 0)
            if not unlimited and attempts >= 15:
                break
            if (not unlimited and until_done and wall_cap_s > 0 and repairs > 0
                    and time.time() - task_t0 > wall_cap_s - 60):
                break
            if not unlimited and not until_done and repairs >= max_repairs:
                break
            repairs += 1
            _emit("repair", id=task_id, round=repairs, reason=(detail or "")[:200])
            print(f"[eval] {task_id} attempt {attempts} FAILED — repairing ({repairs}/{max_repairs})", flush=True)

        from sync.state_sync import safe_get_state
        usage = safe_get_state().get("token_usage", {})
        calls = usage.get("calls", [])
        after = _snapshot(tmp)

        bsa_rows = _read_bsa_telemetry(tel)
        # Honest status: if the agent NEVER produced work (every attempt was hollow —
        # provider/LLM failures), report status="error" (infrastructure), not "fail" (agent).
        if ok:
            status = "pass"
        elif hard_errors >= 3:
            status = "error"
            detail = (detail or "") + " [CRASH-SPIN: developer raised on consecutive attempts — infrastructure failure]"
        elif hollow_retries > 0 and not any_work:
            status = "error"
            detail = (detail or "") + ("" if "HOLLOW" in str(detail) else
                                       " [HOLLOW: all attempts produced no work — LLM/provider failure, not an agent failure]")
        else:
            status = "fail"
        # ── Natural self-repair metrics (single-run, no harness repair) ──
        # tool_seq (from the native run telemetry) is the ordered executed-tool
        # log with per-call failure flags. A NATURAL fix cycle = the agent
        # observed a failing check and then edited a file — entirely inside
        # its own loop. self_checks = verification executions it ran itself.
        _tool_seq = []
        for _c in calls:
            _cs = _c.get("tool_seq") or []
            if isinstance(_cs, list):
                _tool_seq.extend(_cs)
        _saw_fail = False
        _fix_cycles = 0
        for _s in _tool_seq:
            if _s.get("f"):
                _saw_fail = True
            elif _s.get("t") == "edit_file" and _saw_fail:
                _fix_cycles += 1
                _saw_fail = False
        _self_checks = sum(1 for _s in _tool_seq if _s.get("t") == "run_command")
        report.update({
            "status": status,
            "attempts": attempts,
            "repairs": repairs,
            "hollow_retries": hollow_retries,
            "any_work": any_work,
            "attempt_log": attempt_log,
            "pass_at": attempts if ok else None,
            "pass_at_1": (1 if first_real_ok else 0) if first_real_ok is not None else None,
            "prompt_style": os.environ.get("BSA_EVAL_STYLE", "spec"),
            "test_detail": detail,
            "duration_s": round(dur, 1),
            "cost": round(usage.get("total_cost", 0), 6),
            "output_tokens": usage.get("total_output_tokens", 0),
            "input_tokens": usage.get("total_input_tokens", 0),
            "cache_hit": usage.get("total_cache_hit_tokens", 0),
            "cache_miss": usage.get("total_cache_miss_tokens", 0),
            "reasoning_tokens": sum((c.get("reasoning_chars") or 0) // 4 for c in calls),
            "paid_calls": len(calls),
            "model_requests": sum((c.get("requests") or 0) for c in calls if isinstance(c.get("requests"), int)),
            "req_log": [rl for c in calls if isinstance(c.get("req_log"), list) for rl in c.get("req_log")][:60],
            "map_live_deltas": sum((c.get("map_live_deltas") or 0) for c in calls if isinstance(c.get("map_live_deltas"), int)),
            "self_checks": _self_checks,
            "natural_fix_cycles": _fix_cycles,
            "agents": sorted({c.get("agent") for c in calls}),
            "helper_repair_rounds": helper_repair_rounds,
            "tools": _tools_from_calls(calls),
            "bsa": {
                "calls": len(bsa_rows),
                "providers": sorted({r.get("provider", "") for r in bsa_rows if r.get("provider")}),
                "chars": sum(r.get("chars", 0) for r in bsa_rows),
                "by_kind": dict(collections.Counter(r.get("kind", "?") for r in bsa_rows)),
            },
            "files_changed": [f for f in _files_changed(before, after)
                              if not _is_noise_path(f)][:60],
            "spec_produced": os.path.isdir(os.path.join(tmp, "specs")) and any(
                f.endswith(".md") for f in os.listdir(os.path.join(tmp, "specs"))),
        })
        _emit("task_result", id=task_id, status=report["status"], pass_at=report.get("pass_at"),
              pass_at_1=report.get("pass_at_1"), prompt_style=report.get("prompt_style"),
              attempts=attempts, repairs=repairs, hollow_retries=hollow_retries,
              cost=report.get("cost"), duration_s=report.get("duration_s"),
              agents=report.get("agents"), spec_produced=report.get("spec_produced"))
        if (not ok or report["status"] == "error") and keep:
            report["tmp_dir"] = tmp
        elif keep:
            # Audit mode: keep the dir on passes too so the delivered code is inspectable
            report["tmp_dir"] = tmp
            report["kept_on_pass"] = True
        else:
            shutil.rmtree(tmp, ignore_errors=True)
    except Exception as e:
        import traceback
        report["status"] = "error"
        report["error"] = f"{e}\n{traceback.format_exc()[-400:]}"
        _emit("task_error", id=task_id, error=(str(e) or "")[:300])
        if keep:
            report["tmp_dir"] = tmp
        else:
            shutil.rmtree(tmp, ignore_errors=True)

    os.makedirs(_OUTDIR, exist_ok=True)
    with open(os.path.join(_OUTDIR, f"{task_id}.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=1, default=str)
    return report


# ──────────────────────────────────────────────────────────────────────────
#  Summary
# ──────────────────────────────────────────────────────────────────────────

def summarize(task_ids=None):
    files = sorted(glob.glob(os.path.join(_OUTDIR, "*.json")))
    if task_ids is not None:
        wanted = set(task_ids)
        files = [f for f in files if os.path.splitext(os.path.basename(f))[0] in wanted]
    if not files:
        print("no reports yet — run some tasks first")
        return
    print("\n=== EVAL SUMMARY ===")
    print(f"{'task':<18}{'cat':<9}{'res':<8}{'cost$':<9}{'out':<8}{'reason':<8}{'dur s':<7}{'chg':<5}")
    agg = collections.Counter()
    pass1 = 0
    n_real = 0
    total_cost = 0.0
    for f in files:
        r = json.load(open(f, encoding="utf-8"))
        st = r.get("status", "?")
        agg[st] += 1
        total_cost += r.get("cost", 0) or 0
        if r.get("pass_at_1") is not None:
            n_real += 1
            pass1 += r.get("pass_at_1") == 1
        pa = r.get("pass_at")
        pa_s = f"pass@{pa}" if pa and pa > 1 else st
        print(f"{r.get('id','?'):<18}{r.get('category','?'):<9}{pa_s:<8}"
              f"${r.get('cost',0):<9.4f}{r.get('output_tokens',0):<8}{r.get('reasoning_tokens',0):<8}"
              f"{r.get('duration_s',0):<7}{len(r.get('files_changed',[])):<5}")
    print(f"\npass={agg.get('pass',0)}  fail={agg.get('fail',0)}  error={agg.get('error',0)}  "
          f"| total cost ${total_cost:.4f}  | reports in {_OUTDIR}")
    if n_real:
        print(f"pass@1: {pass1}/{n_real} ({100.0 * pass1 / n_real:.1f}%)   "
              f"pass@final: {agg.get('pass',0)}/{len(files)} ({100.0 * agg.get('pass',0) / len(files):.1f}%)")

    # ── Failure classification (evidence for pass@1 work) ──
    # Groups failed tasks by normalized hidden-test signature so the dominant
    # failure classes (interface mismatch / edge case / file naming / runtime)
    # are visible per run instead of being deleted with the tmp workspaces.
    try:
        from error_patterns import _normalize
    except Exception:
        _normalize = lambda t: " ".join((t or "").lower().split())[:200]  # noqa: E731
    fails = []
    for f in files:
        r = json.load(open(f, encoding="utf-8"))
        if r.get("status") == "fail":
            fails.append(r)
    if fails:
        classes = collections.Counter()
        for r in fails:
            sig = _normalize(r.get("test_detail") or "")[:150][:80]
            classes[(sig,)] += 1
        print("\n=== FAILURE CLASSES (normalized hidden-test signatures) ===")
        for (sig,), cnt in classes.most_common(10):
            ids = [r.get("id") for r in fails if _normalize(r.get("test_detail") or "")[:150][:80] == sig]
            print(f"  x{cnt:<3} {sig[:110]}")
            print(f"       tasks: {ids[:12]}")
        try:
            fail_path = os.path.join(os.path.dirname(os.path.abspath(__file__)), "eval_runs",
                                     "failure_classes.json")
            payload = []
            for (sig,), cnt in classes.most_common(20):
                members = [r for r in fails if _normalize(r.get("test_detail") or "")[:150][:80] == sig]
                payload.append({"count": cnt, "signature": sig,
                                "tasks": [r.get("id") for r in members],
                                "detail_head": (members[0].get("test_detail") or "")[:500]})
            with open(fail_path, "w", encoding="utf-8") as f:
                json.dump(payload, f, indent=1, default=str)
            print(f"  -> {fail_path}")
        except Exception as e:
            print(f"  (failed to write failure_classes.json: {e})")
    _emit("summary", tasks=len(files), pass_count=agg.get('pass', 0),
          fail_count=agg.get('fail', 0), error_count=agg.get('error', 0),
          pass_at_1=f"{pass1}/{n_real}" if n_real else None,
          total_cost=round(total_cost, 6))


# ──────────────────────────────────────────────────────────────────────────
#  CLI
# ──────────────────────────────────────────────────────────────────────────

def main():
    try:
        import deepagents  # noqa: F401  (guard: must run under venv312)
    except ImportError:
        print("ERROR: run this with the harness python that has `deepagents`:\n"
              "    venv312/Scripts/python.exe scratch/eval_runner.py ...")
        return 1

    ap = argparse.ArgumentParser(description="Mini SWE-bench eval for the harness")
    ap.add_argument("--only", help="run a single task id")
    ap.add_argument("--category", help="web | feature | bugfix | bugfix-hard | bugfix-dev | gen2-*")
    ap.add_argument("--gens", help="comma-separated gen prefixes, e.g. '2,3,4' runs gen2-*/gen3-*/gen4-* only")
    ap.add_argument("--limit", type=int, default=None, help="run first N tasks (default 3)")
    ap.add_argument("--all", action="store_true", help="run all tasks")
    ap.add_argument("--keep", action="store_true", help="keep tmp project dirs on failure")
    ap.add_argument("--style", default="spec", choices=["spec", "user"],
                    help="prompt style: spec (original prompts) or user (SWE-bench-style vague human reports)")
    ap.add_argument("--repairs", type=int, default=None, help="max self-repair rounds on hidden-test failure (default 2, 0=pass@1)")
    ap.add_argument("--max-hollow", type=int, default=None, help="max fresh-context retries for hollow (no-work) attempts (default 2)")
    ap.add_argument("--hollow-cost", type=float, default=None, help="cost ceiling for an attempt to count as hollow (default 0.003)")
    ap.add_argument("--parallel", type=int, default=1, help="run N tasks concurrently in separate subprocesses (default 1)")
    args = ap.parse_args()
    # Per-call LLM usage logging (agents, output tokens, reasoning chars) so every
    # benchmark run produces analyzable token/cost data. Reset the log per run.
    os.environ["DEEP_AGENTS_EVAL_RUN"] = "1"
    try:
        calls_log = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                                 ".deep_agents", "gen5_eval_calls.jsonl")
        if os.path.exists(calls_log) and not args.only:
            os.remove(calls_log)
    except OSError:
        pass
    if args.repairs is not None:
        os.environ["BSA_MAX_REPAIRS"] = str(args.repairs)
    if args.max_hollow is not None:
        os.environ["BSA_MAX_HOLLOW_RETRIES"] = str(args.max_hollow)
    if args.hollow_cost is not None:
        os.environ["BSA_HOLLOW_COST_MAX"] = str(args.hollow_cost)
    os.environ["BSA_EVAL_STYLE"] = args.style

    if args.only:
        if args.only not in TASKS_BY_ID:
            print(f"unknown task {args.only!r}; available: {sorted(TASKS_BY_ID)}")
            return 1
        # Per-task call telemetry file: no cross-run mixing in the shared log.
        os.environ["BSA_CALLS_LOG"] = os.path.join(
            PROJECT_ROOT, ".deep_agents", f"gen5_eval_calls_{args.only}.jsonl")
        r = run_single(args.only, keep=args.keep)
        print(json.dumps(r, indent=1, default=str))
        return 0 if r["status"] == "pass" else 1

    tasks = TASKS
    if args.gens:
        gens = [g.strip() for g in args.gens.split(",") if g.strip()]
        tasks = [t for t in tasks if any(t["category"].startswith(f"gen{g}-") for g in gens)]
        print(f"[eval] filtered to {len(tasks)} tasks for gens {','.join(gens)}", flush=True)
    if args.category:
        tasks = [t for t in tasks if t["category"] == args.category]
    if not args.all:
        tasks = tasks[: (args.limit or 3)]

    # Each task runs in a fresh subprocess so shared_state is clean per task.
    script = os.path.abspath(__file__)
    purged = _purge_stale_eval_tasks()
    if purged:
        print(f"[eval] purged {purged} stale eval task-tracking file(s) from .deep_agents/tasks/", flush=True)

    def _report_done(tid, wall_s, note=""):
        """Live per-task completion line (status + wall time), read from the JSON report."""
        try:
            with open(os.path.join(_OUTDIR, f"{tid}.json"), encoding="utf-8") as f:
                rep = json.load(f)
            _st = str(rep.get("status", "?"))
            _pa = rep.get("pass_at")
            line = (f"    [done] {tid:<26} {_st:<6} dur={wall_s:7.1f}s "
                    f"cost=${float(rep.get('cost', 0) or 0):.4f} pass@"
                    f"{str(_pa) if _pa is not None else '-'}  "
                    f"attempts={rep.get('attempts', 0)}  "
                    f"out={rep.get('output_tokens', 0):>6,} re={rep.get('reasoning_tokens', 0):>6,} "
                    f"miss={rep.get('cache_miss', 0):>6,}  {time.strftime('%H:%M:%S')}{(' ' + note) if note else ''}")
            print(line, flush=True)
        except Exception:
            print(f"    [done] {tid:<26} (no report yet) {time.strftime('%H:%M:%S')}", flush=True)

    def _run_one(t):
        print(f"\n===== {t['id']} ({t['category']})  started {time.strftime('%H:%M:%S')} =====", flush=True)
        cmd = [sys.executable, script, "--only", t["id"]]
        if args.keep:
            cmd.append("--keep")
        cmd += ["--style", args.style]
        if args.repairs is not None:
            cmd += ["--repairs", str(args.repairs)]
        env = {**os.environ, "PYTHONIOENCODING": "utf-8"}
        env["BSA_CALLS_LOG"] = os.path.join(
            PROJECT_ROOT, ".deep_agents", f"gen5_eval_calls_{t['id']}.jsonl")
        wall_cap = int(os.environ.get("BSA_TASK_WALL_CAP", "2400") or 0)
        flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
        _t_start = time.time()
        try:
            proc = subprocess.Popen(
                cmd,
                cwd=os.path.dirname(script),
                env=env,
                creationflags=flags,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                encoding="utf-8",
                errors="replace",
            )
            try:
                child_output, _ = proc.communicate(timeout=wall_cap if wall_cap > 0 else None)
                rc = proc.returncode
            except subprocess.TimeoutExpired:
                # Wall-cap hit (a stuck repair loop should not eat hours): kill the
                # whole process tree and record an honest TIMEOUT report.
                subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                               capture_output=True, timeout=15, creationflags=flags)
                try:
                    child_output, _ = proc.communicate(timeout=15)
                except Exception:
                    child_output = ""
                os.makedirs(_OUTDIR, exist_ok=True)
                with open(os.path.join(_OUTDIR, f"{t['id']}.json"), "w", encoding="utf-8") as f:
                    json.dump({
                        "id": t["id"], "category": t["category"], "status": "fail",
                        "attempts": 1, "repairs": 0, "pass_at": None, "pass_at_1": None,
                        "prompt_style": args.style, "cost": 0.0, "duration_s": wall_cap,
                        "output_tokens": 0, "input_tokens": 0, "cache_hit": 0,
                        "cache_miss": 0, "reasoning_tokens": 0, "paid_calls": 0,
                        "agents": [], "tools": [],
                        "test_detail": f"[WALL-CAP TIMEOUT after {wall_cap}s]",
                    }, f, indent=1, default=str)
                _report_done(t["id"], wall_cap, note="WALL-CAP-TIMEOUT")
                return
            if rc != 0:
                tail = (child_output or "").strip()[-2400:]
                print(f"    [child exit {rc}]" + (f"\n{tail}" if tail else ""), flush=True)
            _report_done(t["id"], time.time() - _t_start)
        except Exception as e:
            print(f"    [spawn error: {e}]", flush=True)

    if args.parallel and args.parallel > 1:
        from concurrent.futures import ThreadPoolExecutor, as_completed
        workers = min(args.parallel, len(tasks))
        print(f"[eval] running {len(tasks)} tasks with {workers} parallel workers", flush=True)
        with ThreadPoolExecutor(max_workers=workers) as ex:
            futs = {ex.submit(_run_one, t): t["id"] for t in tasks}
            for fut in as_completed(futs):
                fut.result()
    else:
        for t in tasks:
            _run_one(t)

    summarize([t["id"] for t in tasks])
    return 0


if __name__ == "__main__":
    sys.exit(main())
