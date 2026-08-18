"""Build the FINAL gen9 manifest: 18 curated real-world bug tasks with prompts.

Reads the validated candidates (swe_manifest_draft.json), selects 9 Python +
9 Node, generates the frozen artifacts the suite needs:
  - scratch/swe_gen9/tasks.json        (final task metadata + prompts)
  - scratch/swe_gen9/snapshots/<id>/   (frozen base-commit workspace copies)
  - scratch/swe_gen9/testpatches/<id>.patch (hidden-test patches)
Run BEFORE any suite run. Everything is deterministic.
"""
import json, os, shutil, subprocess, sys

sys.stdout.reconfigure(encoding="utf-8", errors="replace")
ROOT = os.path.dirname(os.path.abspath(__file__))
CACHE = os.path.join(ROOT, "swe_repo_cache")
OUT = os.path.join(ROOT, "swe_gen9")
os.makedirs(os.path.join(OUT, "snapshots"), exist_ok=True)
os.makedirs(os.path.join(OUT, "testpatches"), exist_ok=True)

m = json.load(open(os.path.join(ROOT, "swe_manifest_draft.json"), encoding="utf-8-sig"))
def find(repo, short):
    return next((e for e in m if e["repo"] == repo and e["fix_sha"].startswith(short)), None)

SEL = [
    ("sqlite-utils", "b436bdb5", "g9r-sqlite-null-columns"),
    ("sqlite-utils", "0bad2128", "g9r-sqlite-stop-before"),
    ("flask", "4995a775", "g9r-flask-subdomain"),
    ("flask", "e82db2ca", "g9r-flask-auto-options"),
    ("flask", "fb541598", "g9r-flask-key-rotation"),
    ("bottle", "e0c5ed19", "g9r-bottle-close-body"),
    ("bottle", "b73bd1db", "g9r-bottle-cache-headers"),
    ("fastapi", "d6235443", "g9r-fastapi-frontend-deps"),
    ("fastapi", "0f3e7bd6", "g9r-fastapi-sse-lines"),
    ("fastify", "b03d0783", "g9r-fastify-error-handler"),
    ("fastify", "970c5758", "g9r-fastify-diag-status"),
    ("fastify", "6d202c62", "g9r-fastify-diag-async"),
    ("fastify", "2f597a92", "g9r-fastify-prefix-join"),
    ("fastify", "1e0be129", "g9r-fastify-media-type-msg"),
    ("fastify", "c06fc06d", "g9r-fastify-ct-schema"),
    ("fastify", "6f63ce93", "g9r-fastify-ct-send"),
    ("express", "18e5985b", "g9r-express-content-length"),
    ("express", "74beeac0", "g9r-express-no-method"),
]

PROMPTS = {
 "g9r-sqlite-null-columns":
  "I'm inserting records where some fields are None. A column whose values are ALL null comes out with a "
  "garbage type, and mixing nulls into a column with real values seems to break type inference too. A column "
  "that only contains nulls should be created as TEXT, and nulls must never change the inferred type of a "
  "column that has real values.",
 "g9r-sqlite-stop-before":
  "The `migrate` command's --stop-before flag is dangerous: if I misspell a migration name it silently ignores "
  "it and runs ALL migrations anyway. It should validate that every --stop-before value matches a real "
  "migration and refuse to run if any value is unknown. It also crashes when the project uses the older "
  "sqlite-migrate style migration objects with multiple --stop-before values.",
 "g9r-flask-subdomain":
  "I set SERVER_NAME in my config and now my app refuses to serve requests that don't match that exact domain, "
  "even though I never asked for host restrictions. Also when subdomain_matching is disabled, subdomains still "
  "behave incorrectly. SERVER_NAME should not restrict which hosts can reach the app, and disabled subdomain "
  "matching must keep working as documented.",
 "g9r-flask-auto-options":
  "Setting provide_automatic_options=True on a view function has no effect when the app config has "
  "PROVIDE_AUTOMATIC_OPTIONS=False — the OPTIONS response is never registered. Per-view force-enabling must "
  "override the global config (disabling already works; enabling does not).",
 "g9r-flask-key-rotation":
  "After enabling key rotation with SECRET_KEY_FALLBACKS, session cookies signed by the CURRENT key sometimes "
  "fail to unsign — the serializer is not trying the current secret_key first. New cookies must always be "
  "signed/verified with the current key, with fallbacks only as a backstop.",
 "g9r-bottle-close-body":
  "When a route handler raises an exception mid-response, or returns an unsupported response type, the "
  "closeable response body is never closed — file handles and connections leak. The body should be closed on "
  "every error path, not just the happy path.",
 "g9r-bottle-cache-headers":
  "When a client sends BOTH If-None-Match and If-Modified-Since, my app serves 304 based on the date even "
  "when the ETag does NOT match what the client sent. Per the caching rules, If-Modified-Since must be "
  "ignored whenever If-None-Match is present.",
 "g9r-fastapi-frontend-deps":
  "Routes registered via app.frontend() (SPA static routes) silently drop background tasks and response "
  "headers injected by dependencies — they work fine on normal routes but not on the frontend route. The "
  "SPA route must honor dependency-provided background tasks and headers.",
 "g9r-fastapi-sse-lines":
  "Server-sent events get corrupted when the data contains \r or \r\n sequences: format_sse_event splits on "
  "str.splitlines(), which mangles content. SSE events must split strictly on the spec line terminators "
  "(\n, \r\n, \r) and preserve the content.",
 "g9r-fastify-error-handler":
  "My custom setErrorHandler receives errors that are not FastifyError instances — plain objects or errors "
  "from third-party plugins — and the request handling crashes while computing the status code from "
  "error.statusCode. The default error handler must tolerate arbitrary error payloads and still produce a "
  "valid 500 response.",
 "g9r-fastify-diag-status":
  "Subscribers of the fastify.request.handler diagnostics channel see errors BEFORE the status code has "
  "been applied — the store they receive has no statusCode. The error status must be set before the "
  "diagnostics event is published.",
 "g9r-fastify-diag-async":
  "When the error handler returns a promise (async error handler), diagnostics tracking is completely "
  "silent — the channel never fires. Async error handler results must publish to the diagnostics channel "
  "just like synchronous ones.",
 "g9r-fastify-prefix-join":
  "Registering a plugin with a prefix under an instance whose own prefix ends with '/' produces routes like "
  "'/firstsecond' — the slash between the two prefixes is missing in one of the joining cases. Nested "
  "prefixes must always join with exactly one slash.",
 "g9r-fastify-media-type-msg":
  "Sending an unsupported media type returns an error message containing a raw '%s' placeholder when the "
  "content type is absent — e.g. 'Unsupported Media Type: %s'. The placeholder must never leak into the "
  "client-facing error message.",
 "g9r-fastify-ct-schema":
  "Response schemas keyed by media type (e.g. 'application/json') stop matching when the response "
  "content-type carries parameters like 'application/json; charset=utf-8'. The media type extraction for "
  "schema lookup must handle parameterized content types correctly.",
 "g9r-fastify-ct-send":
  "reply.send() mis-detects JSON when the content type has a charset (e.g. 'application/json; "
  "charset=utf-8') — the JSON serialization path is skipped. Content-Type parsing must detect json/charset "
  "properly everywhere it is used.",
 "g9r-express-content-length":
  "When a response already has a Transfer-Encoding header, res.send() still adds a Content-Length header — "
  "the two together corrupt the response body on the wire. Content-Length must only be set when "
  "Transfer-Encoding is absent.",
 "g9r-express-no-method":
  "Some requests crash the router with 'method.toLowerCase is not a function' — when the request method is "
  "not a plain string (or is missing), routing throws instead of falling through. Method handling must be "
  "defensive so such requests are handled gracefully.",
}

def run(cmd, cwd=None):
    r = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=120)
    return r.returncode, r.stdout, r.stderr

tasks = []
for repo, short, tid in SEL:
    e = find(repo, short)
    assert e, f"missing {repo} {short}"
    rd = os.path.join(CACHE, repo)
    base, fix = e["base_sha"], e["fix_sha"]
    snap = os.path.join(OUT, "snapshots", tid)
    if not os.path.isdir(snap):
        wt = os.path.join(OUT, "_wt")
        shutil.rmtree(wt, ignore_errors=True)
        rc, _, err = run(["git", "-C", rd, "worktree", "add", "--detach", wt, base])
        assert rc == 0, f"worktree {tid}: {err[:200]}"
        os.makedirs(snap, exist_ok=True)
        for entry in os.listdir(wt):
            src = os.path.join(wt, entry)
            if entry == ".git":
                continue
            if os.path.isdir(src):
                shutil.copytree(src, os.path.join(snap, entry), ignore=shutil.ignore_patterns("node_modules", ".venv"))
            else:
                shutil.copy2(src, os.path.join(snap, entry))
        run(["git", "-C", rd, "worktree", "remove", "--force", wt])
    # hidden-test patch
    tpatch = os.path.join(OUT, "testpatches", f"{tid}.patch")
    rc, tp, _ = run(["git", "-C", rd, "diff", base, fix, "--"] + e["test_files"])
    assert rc == 0 and tp.strip(), f"test patch {tid}"
    with open(tpatch, "w", encoding="utf-8") as f:
        f.write(tp)
    tasks.append({
        "id": tid,
        "category": "gen9-real",
        "repo": repo,
        "lang": e["lang"],
        "base_sha": base,
        "fix_sha": fix,
        "subject": e["subject"],
        "prompt": PROMPTS[tid],
        "src_files": e["src_files"],
        "test_files": e["test_files"],
        "nm_dir": e.get("nm_dir"),
        "snapshot": os.path.relpath(snap, OUT),
        "testpatch": os.path.relpath(tpatch, OUT),
        "budget": {"max_iters": 60, "remaining_steps": 60, "max_cost_usd": 0.05},
    })

with open(os.path.join(OUT, "tasks.json"), "w", encoding="utf-8") as f:
    json.dump(tasks, f, indent=2)
print(f"gen9 manifest: {len(tasks)} tasks written to {OUT}")
for t in tasks:
    print(f"  {t['id']:<28} {t['repo']:<12} {t['lang']}")
