"""verify_helper.py — harness-owned verification gate + SDET-FIX package builder.

The harness (not the agents) owns verification:

  - eval: the hidden test is the authoritative gate (eval_runner.py).
  - app:  adaptive_verify() runs the project's OWN prepared scripts first
          (package.json test/check/lint, pytest, go test, Makefile targets,
          CI-committed test commands) — the "scripts they already prepared"
          pattern from enterprise CI — then falls back to a generated
          behavior smoke check for the task's domain (server boots and its
          routes answer, modules import, syntax is valid).

On failure the harness builds an SDET-FIX package (GOAL + FAILURE + test
output head+tail + prior state) and spawns SDET through the subagent
registry (tools.task) — the Developer never spawns SDET and never receives
a long diagnosis report; SDET fixes and hands back a one-line DONE.
"""
import json
import os
import re
import shutil
import subprocess
import sys
import time

_HARNESS_ROOT = os.path.dirname(os.path.abspath(__file__))
_NOISE = {"_hidden_failure.log", "_verify_out.txt", "_ops_out.txt", "bsa_telemetry.jsonl",
          "package-lock.json", "node_modules"}


# ──────────────────────────────────────────────────────────────────────────
#  Command discovery — the project's own prepared verification scripts
# ──────────────────────────────────────────────────────────────────────────

def _read_pkg_json(project_path):
    p = os.path.join(project_path, "package.json")
    if not os.path.isfile(p):
        return {}
    try:
        with open(p, encoding="utf-8", errors="replace") as f:
            return json.load(f)
    except Exception:
        return {}


def discover_verify_commands(project_path):
    """Return [(label, cmd)] of the project's own verification commands,
    most-canonical first. Only commands whose backing artifact exists are
    returned."""
    cmds = []

    pkg = _read_pkg_json(project_path)
    scripts = pkg.get("scripts") or {}
    # npm test is the canonical Node gate; check/lint/typecheck/build follow.
    for name in ("test", "check", "verify", "typecheck", "type-check", "lint", "build"):
        if name in scripts:
            cmds.append((f"npm run {name}", f"npm run {name}"))
    if "test" in scripts:
        cmds = [("npm test", "npm test")] + [c for c in cmds if c[0] != "npm run test"]

    # Python: pytest is the canonical gate.
    py_conf = any(os.path.isfile(os.path.join(project_path, c)) for c in
                  ("pytest.ini", "tox.ini", "setup.cfg", "requirements-dev.txt"))
    pyproject = os.path.join(project_path, "pyproject.toml")
    has_pytest_conf = py_conf
    if os.path.isfile(pyproject):
        try:
            has_pytest_conf = has_pytest_conf or "[tool.pytest" in open(pyproject, encoding="utf-8", errors="replace").read()
        except Exception:
            pass
    py_tests = [f for f in os.listdir(project_path)
                if f.startswith("test_") and f.endswith(".py")] if os.path.isdir(project_path) else []
    if has_pytest_conf or py_tests:
        cmds.append(("pytest", f'"{sys.executable}" -m pytest -x -q'))

    # Go.
    if os.path.isfile(os.path.join(project_path, "go.mod")):
        cmds.append(("go test", "go test ./..."))

    # Makefile targets.
    mk = os.path.join(project_path, "Makefile")
    if os.path.isfile(mk):
        try:
            mktext = open(mk, encoding="utf-8", errors="replace").read()
            for tgt in ("test", "check", "verify"):
                if re.search(rf"^{tgt}\s*:", mktext, re.MULTILINE):
                    cmds.append((f"make {tgt}", f"make {tgt}"))
                    break
        except Exception:
            pass

    # CI-committed test commands (best-effort extraction from workflow files).
    for ci in ("github", "gitlab"):
        pass
    gh = os.path.join(project_path, ".github", "workflows")
    if os.path.isdir(gh):
        for fn in os.listdir(gh):
            if not fn.endswith((".yml", ".yaml")):
                continue
            try:
                text = open(os.path.join(gh, fn), encoding="utf-8", errors="replace").read()
                for line in text.splitlines():
                    line = line.strip()
                    m = re.match(r"run\s*:\s*(.+)$", line)
                    if m and re.search(r"\b(test|pytest|go test|npm (test|run test))\b", m.group(1)):
                        cand = m.group(1).strip().strip("'\"")
                        if cand and cand not in [c for _, c in cmds]:
                            cmds.append((f"CI test ({fn})", cand))
                            break
            except Exception:
                continue

    return cmds


# ──────────────────────────────────────────────────────────────────────────
#  Command execution (file-redirect pattern, no pipe hangs)
# ──────────────────────────────────────────────────────────────────────────

def run_command_capture(cmd, cwd, timeout=120):
    """Run a shell command, capture combined output to a file (grandchild-safe).
    Returns (rc, output). rc == -1 on timeout."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    outfile = os.path.join(cwd, "_verify_out.txt")
    try:
        os.remove(outfile)
    except OSError:
        pass
    rc = None
    try:
        with open(outfile, "wb") as f:
            proc = subprocess.Popen(cmd, cwd=cwd, shell=True, stdout=f,
                                    stderr=subprocess.STDOUT, creationflags=flags)
            try:
                rc = proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                rc = -1
            finally:
                try:
                    subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"],
                                   capture_output=True, timeout=10, creationflags=flags)
                except Exception:
                    pass
    except Exception as e:
        return -1, f"spawn error: {e}"
    try:
        out = open(outfile, encoding="utf-8", errors="replace").read()
    except OSError:
        out = ""
    # Strip ANSI escapes + progress-bar overwrites from CLI output before it
    # reaches any agent (colors carry no diagnostics; smaller = cheaper).
    try:
        from tools import _clean_terminal_output
        out = _clean_terminal_output(out)
    except Exception:
        pass
    return rc, out


# ──────────────────────────────────────────────────────────────────────────
#  Generated behavior smoke check (fallback when the project has no suite)
# ──────────────────────────────────────────────────────────────────────────

def _changed_files(project_path, max_n=25):
    """Best-effort snapshot of project source files (excludes noise dirs)."""
    found = []
    for root, dirs, files in os.walk(project_path):
        dirs[:] = [d for d in dirs if d not in _NOISE and not d.startswith(".")]
        for fn in files:
            if fn in _NOISE:
                continue
            rel = os.path.relpath(os.path.join(root, fn), project_path)
            if rel.endswith((".js", ".py", ".ts", ".mjs", ".cjs", ".html", ".json")):
                found.append(rel)
        if len(found) >= max_n:
            break
    return found


def _find_js_entries(project_path):
    """Candidate server/module entries in priority order."""
    pkg = _read_pkg_json(project_path)
    main = pkg.get("main")
    entries = []
    if main:
        entries.append(main)
    for cand in ("server.js", "app.js", "index.js", "src/server.js", "src/app.js", "src/index.js",
                 "server.mjs", "app.mjs", "index.mjs"):
        if os.path.isfile(os.path.join(project_path, cand)) and cand not in entries:
            entries.append(cand)
    return entries


_NODE_SMOKE_TMPL = r"""// generated harness smoke check — boots the app and probes its routes
let entry = require('./__ENTRY__');
const app =
  (entry && typeof entry === 'object' && !entry.listen && entry.app) ||
  (entry && entry.default) ||
  (entry && typeof entry === 'object' && !entry.listen && entry.createApp && entry.createApp()) ||
  entry;
if (!app || typeof app.listen !== 'function') {
  console.log('SMOKE-OK: module loaded without a listenable app (unit module)');
  process.exit(0);
}
const server = app.listen(0, async () => {
  const base = 'http://127.0.0.1:' + server.address().port;
  const paths = __PATHS__;
  try {
    for (const p of paths) {
      const r = await fetch(base + p, { method: 'GET', headers: { 'Accept': 'application/json' } });
      if (r.status >= 500) throw new Error(p + ' -> ' + r.status);
    }
    console.log('SMOKE-OK: routes ' + JSON.stringify(paths) + ' responded');
    process.exit(0);
  } catch (e) {
    console.error('SMOKE-FAIL: ' + (e && e.message));
    process.exit(1);
  }
});
setTimeout(() => { console.error('SMOKE-FAIL: server startup timed out'); process.exit(1); }, 20000).unref();
"""


def _node_smoke(project_path):
    """Boot the server / load the module. Catches wiring errors, missing
    modules, startup crashes — the class of bug self-tests often miss."""
    entries = _find_js_entries(project_path)
    if not entries:
        return True, "no JS entry found — nothing to smoke"
    if not shutil.which("node"):
        return False, "node not available"
    # ensure deps installed if package.json exists
    if os.path.isfile(os.path.join(project_path, "package.json")):
        nm = os.path.join(project_path, "node_modules")
        if not os.path.isdir(nm):
            run_command_capture("npm install --silent", project_path, timeout=180)
    # discover declared routes for probing
    routes = set()
    for root, dirs, files in os.walk(project_path):
        dirs[:] = [d for d in dirs if d not in _NOISE and not d.startswith(".")]
        for fn in files:
            if not fn.endswith(".js"):
                continue
            try:
                text = open(os.path.join(root, fn), encoding="utf-8", errors="replace").read()
            except Exception:
                continue
            for m in re.finditer(r"(?:app|router)\.(get|post|put|delete|patch)\(\s*['\"]([^'\"*]+)['\"]", text):
                routes.add(m.group(2))
    paths = sorted(routes)[:12] or ["/"]
    script = os.path.join(project_path, "_smoke_test.js")
    errs = []
    for entry in entries:
        src = _NODE_SMOKE_TMPL.replace("__ENTRY__", entry).replace("__PATHS__", json.dumps(paths))
        with open(script, "w", encoding="utf-8") as f:
            f.write(src)
        try:
            r = subprocess.run([shutil.which("node"), script], cwd=project_path,
                               capture_output=True, text=True, timeout=90)
        except subprocess.TimeoutExpired:
            errs.append(f"{entry}: startup timeout")
            continue
        combined = (r.stdout or "") + (r.stderr or "")
        if "SMOKE-OK" in combined:
            try:
                os.remove(script)
            except OSError:
                pass
            return True, combined[-500:]
        errs.append(f"{entry}: " + (combined.strip().splitlines()[-1][:200] if combined.strip() else "no output"))
    try:
        os.remove(script)
    except OSError:
        pass
    return False, "SMOKE-FAIL: " + " | ".join(errs)[-600:]


def _python_smoke(project_path):
    """py_compile every source file + import top-level modules (catches
    syntax errors and missing-import wiring)."""
    bad = []
    py = sys.executable
    for rel in _changed_files(project_path):
        if not rel.endswith(".py"):
            continue
        full = os.path.join(project_path, rel)
        r = subprocess.run([py, "-m", "py_compile", full], cwd=project_path,
                           capture_output=True, text=True, timeout=60)
        if r.returncode != 0:
            bad.append(f"{rel}: {r.stderr.strip().splitlines()[-1][:120] if r.stderr else 'compile error'}")
    if bad:
        return False, "SMOKE-FAIL: " + "; ".join(bad)[:600]
    return True, "SMOKE-OK: all python files compile"


def _generic_smoke(project_path):
    """Syntax check JS/TS without executing (entry-less projects)."""
    node = shutil.which("node")
    bad = []
    for rel in _changed_files(project_path):
        if rel.endswith((".js", ".mjs", ".cjs")) and node:
            r = subprocess.run([node, "--check", os.path.join(project_path, rel)],
                               capture_output=True, text=True, timeout=60)
            if r.returncode != 0:
                bad.append(f"{rel}: {r.stderr.strip().splitlines()[-1][:120] if r.stderr else 'syntax error'}")
    if bad:
        return False, "SMOKE-FAIL: " + "; ".join(bad)[:600]
    return True, "SMOKE-OK: no syntax errors detected"


def _syntax_check(project_path):
    """Zero-LLM syntax pass over the workspace's code files: node --check for
    JS/TS and python -m py_compile for PY. Catches the dumbest failures before
    any script/smoke runs. Returns (ok, detail)."""
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0)
    bad = []
    checked = 0
    try:
        for root, dirs, files in os.walk(project_path):
            dirs[:] = [d for d in dirs if d not in ("node_modules", ".git", ".agent-memory",
                                                    "venv", ".venv", "venv312", "dist", "build")]
            for fn in files:
                if not fn.endswith((".js", ".ts", ".tsx", ".jsx", ".py")):
                    continue
                p = os.path.join(root, fn)
                if fn.endswith(".py"):
                    r = subprocess.run(["python", "-m", "py_compile", p], capture_output=True,
                                       text=True, timeout=30, creationflags=flags)
                else:
                    r = subprocess.run(["node", "--check", p], capture_output=True,
                                       text=True, timeout=30, creationflags=flags)
                checked += 1
                if r.returncode != 0:
                    tail = (r.stderr or r.stdout or "")[-400:]
                    bad.append(f"{p}:\n{tail}")
                if checked >= 200:
                    break
            if checked >= 200:
                break
    except Exception as e:
        return True, f"[syntax] check error: {e}"
    if bad:
        return False, "[SYNTAX] broken files:\n" + "\n".join(bad[:5])
    return True, "[syntax] OK" if checked else "[syntax] no code files"


def adaptive_verify(project_path, task_hint=""):
    """Run the harness verification gate for a project.

    Returns (ok, output). Step 0 runs a zero-LLM syntax pass over code files.
    Step 1 runs the project's own prepared scripts (npm test / pytest / go
    test / Makefile / CI-committed commands). If the project has none,
    Step 2 generates a behavior smoke check matching the project's domain
    (server boot + route probe, module import, compile).
    """
    if not project_path or not os.path.isdir(project_path):
        return False, "verify: project path missing"

    # ── Step 0: zero-LLM syntax pass (cheap, catches dumb failures first) ──
    ok, out = _syntax_check(project_path)
    if not ok:
        return False, out

    # ── Step 1: the project's own prepared scripts (enterprise CI pattern) ──
    cmds = discover_verify_commands(project_path)
    ran_any = False
    for label, cmd in cmds:
        rc, out = run_command_capture(cmd, project_path, timeout=150)
        ran_any = True
        tail = (out or "")[-2000:]
        if rc != 0:
            return False, f"[{label}] rc={rc}\n{tail}"
        return True, f"[{label}] rc={rc}\n{tail}"

    # ── Step 2: no prepared scripts → generated domain-aware smoke check ──
    hint = (task_hint or "").lower()
    is_node = os.path.isfile(os.path.join(project_path, "package.json")) or any(
        f.endswith(".js") for f in os.listdir(project_path)) if os.path.isdir(project_path) else False
    is_py = any(f.endswith(".py") for f in os.listdir(project_path)) if os.path.isdir(project_path) else False

    if is_node:
        ok, out = _node_smoke(project_path)
    elif is_py:
        ok, out = _python_smoke(project_path)
    else:
        ok, out = _generic_smoke(project_path)
    if ok:
        return True, out
    # If the node smoke failed to even load, also try generic syntax check for
    # more signal before declaring failure.
    return False, out


# ──────────────────────────────────────────────────────────────────────────
#  SDET-FIX package — the context handoff (GOAL + FAILURE + output head+tail)
# ──────────────────────────────────────────────────────────────────────────

def _head_tail(text, head=1500, tail=1500):
    text = str(text or "")
    if len(text) <= head + tail:
        return text
    return text[:head] + f"\n... [{len(text) - head - tail} chars truncated] ...\n" + text[-tail:]


def _extract_contract(text):
    """Best-effort extraction of the expected contract from a failure output:
    lines that state expectations (expected / should be / must return / assert
    conditions). Gives SDET the exact contract so its first fix lands."""
    if not text:
        return ""
    picks = []
    for ln in str(text).splitlines():
        low = ln.lower()
        if any(k in low for k in ("expected", "should be", "should return", "should equal",
                                  "must be", "must return", "must equal", "expect", "!== ",
                                  "!= ", "to be null", "should be null", "got ")):
            if len(ln) < 300 and ln.strip():
                picks.append(ln.strip())
        if len(picks) >= 8:
            break
    return "\n".join(picks)


def build_sdet_fix_package(goal, failure_detail, test_output, files=None,
                           prior_state=None, prior_diff=None, max_goal=3000):
    """The complete context SDET-FIX receives: the goal (what must work), the
    authoritative failure, the exact CONTRACT demanded by the verification, the
    test output head+tail, what exists on disk, and — when available — the
    diff of the previous attempt (the 'lift from latest test' context)."""
    contract = _extract_contract(test_output) or _extract_contract(failure_detail)
    parts = [
        "## GOAL",
        (goal or "").strip()[:max_goal],
        "",
        "## FAILURE",
        (failure_detail or "").strip()[:1200] or "(no detail — reproduce it)",
        "",
        "## TEST OUTPUT (head + tail)",
        _head_tail(test_output, 1500, 1500) or "(none captured)",
    ]
    if prior_diff:
        parts += [
            "",
            "## PRIOR ATTEMPT DIFF (what the previous attempt changed — the diff IS the bug surface)",
            str(prior_diff)[:2000],
            "",
            "This diff shows exactly what the previous attempt modified. The failing check is "
            "almost certainly caused by (or missing from) these changes — inspect and correct "
            "them rather than re-exploring the whole project.",
        ]
    if contract:
        parts += [
            "",
            "## CONTRACT (what the verification demands — match this LITERALLY)",
            contract,
            "",
            "The assertion lines above ARE the specification. Reproduce the exact "
            "expected values, shapes, and status codes they demand, even where they "
            "differ from your judgment of 'correct' behavior. Do NOT 'improve' the "
            "semantics and do NOT theorize about other edge cases.",
        ]
    if files:
        listing = ", ".join(str(f) for f in files[:20])
        parts += ["", "## FILES", listing]
    if prior_state:
        parts += ["", "## PREVIOUS ATTEMPT STATE", str(prior_state)[:800]]
    parts += [
        "",
        "## YOUR JOB",
        "Reproduce the failure yourself (run the failing command / the project's own test "
        "script — npm test, pytest, or whatever exists; do not guess the syntax, run it). "
        "If the project has no runnable test command, write your OWN temporary repro script "
        "that asserts the CONTRACT above, and iterate against it. Read the relevant files, "
        "find the ROOT CAUSE (wiring errors, wrong values, wrong contracts — not just "
        "typos), fix it with minimal edits, then re-run the verification until it PASSES. "
        "One careful pass is the goal: reproduce → read the ONE failing path → minimal fix "
        "→ verify. If your first fix fails, STOP and re-read the CONTRACT before touching "
        "anything else. Never give up until your own verification passes.",
        "",
        "Reply with EXACTLY this format and nothing else:",
        "DONE | FILES: <comma-separated changed paths> | NOTE: <one line>",
    ]
    return "\n".join(parts)


def run_sdet_fix(goal, failure_detail, test_output, files=None, prior_state=None,
                 prior_diff=None, project_path="", chat_id="", extra_instruction=""):
    """Spawn SDET-FIX via the subagent registry (tools.task → generic tool loop).

    Returns the handoff string (DONE | FILES: ... | NOTE: ...) or an error note.
    The caller is responsible for re-running its own verification afterwards.
    """
    pkg = build_sdet_fix_package(goal, failure_detail, test_output, files, prior_state, prior_diff)
    if extra_instruction:
        pkg += "\n\n" + extra_instruction

    # Verified past-failure patterns (harness-global, matched by error
    # signature). Injected ONLY here — the repair package is volatile, so
    # the matched subset rides along with zero cache impact. BSA_ERROR_PATTERNS=1.
    try:
        if os.environ.get("BSA_ERROR_PATTERNS", "1") == "1":
            from error_patterns import match_patterns
            matches = match_patterns((failure_detail or "") + "\n" + (test_output or ""))
            if matches:
                lines = [
                    "",
                    "## KNOWN PATTERNS (verified from past failures)",
                    "Each pattern below is a REAL failure that a past repair round fixed and",
                    "the verification then PASSED. ADAPT it to THIS codebase — do not copy blindly.",
                ]
                for i, m in enumerate(matches, 1):
                    lines.append(f"\nPattern {i} — error: {m['sig'][:140]}")
                    if m.get("snippet"):
                        lines.append(f"  failing output: {m['snippet'][:200]}")
                    if m.get("fix"):
                        lines.append(f"  fix that passed: {m['fix'][:600]}")
                pkg += "\n".join(lines)
    except Exception as e:
        print(f"[patterns] injection failed: {e}")

    try:
        from tools import execute_tool
        from state_sync import safe_update_state, shared_state
        try:
            safe_update_state({"project_path": project_path or shared_state.get("project_path", ""),
                               "chat_id": chat_id or shared_state.get("chat_id", "")})
        except Exception:
            pass
        res = execute_tool("task", {"name": "SDET", "task": pkg})
        return str(res)
    except Exception as e:
        return f"SDET-FIX SPAWN ERROR: {e}"


def _snapshot_files(project_path: str) -> dict:
    """Capture {rel_path: content} for code files — used to diff a repair round."""
    snap = {}
    try:
        for root, dirs, files in os.walk(project_path):
            dirs[:] = [d for d in dirs if d not in ("node_modules", "venv", ".venv", "venv312",
                                                    ".git", "__pycache__", ".agent-memory",
                                                    ".pytest_cache", ".deep_agents", "dist", "build")]
            for fn in files:
                if not fn.lower().endswith((".py", ".js", ".ts", ".tsx", ".jsx", ".php", ".dart", ".json")):
                    continue
                rel = os.path.relpath(os.path.join(root, fn), project_path)
                try:
                    if os.path.getsize(os.path.join(root, fn)) > 200000:
                        continue  # skip big lock/manifest files
                    with open(os.path.join(root, fn), "r", encoding="utf-8", errors="replace") as f:
                        snap[rel] = f.read()
                except Exception:
                    pass
    except Exception:
        pass
    return snap


def _learn_pattern(failure_detail: str, project_path: str, pre_snap: dict) -> None:
    """App-side verified-lesson learning: a repair round made verification pass.
    Records the failure signature + the exact diff of what changed."""
    try:
        if os.environ.get("BSA_ERROR_PATTERNS", "1") != "1" or not failure_detail:
            return
        import difflib as _dl
        from error_patterns import learn
        fix_parts = []
        for rel, old in pre_snap.items():
            cur = os.path.join(project_path, rel)
            if not os.path.isfile(cur):
                continue
            with open(cur, "r", encoding="utf-8", errors="replace") as f:
                new = f.read()
            if new != old:
                d = list(_dl.unified_diff(old.splitlines(), new.splitlines(), lineterm="",
                                          fromfile=f"a/{rel}", tofile=f"b/{rel}", n=1))
                fix_parts.append("\n".join(d))
        fix_diff = "\n".join(fix_parts)[:600]
        if fix_diff:
            learn(failure_detail, fix_diff, workspace=project_path)
    except Exception:
        pass


def verify_and_fix(project_path, task_hint, chat_id="", max_fix_rounds=2,
                   goal_override=None, files_override=None):
    """App-mode gate: verify → on failure spawn SDET-FIX → re-verify (bounded).

    Returns (final_ok, rounds, last_output, handoffs).
    """
    goal = goal_override or task_hint
    ok, output = adaptive_verify(project_path, task_hint)
    rounds = 0
    handoffs = []
    while not ok and rounds < max_fix_rounds:
        rounds += 1
        fail_output = output  # the failing evidence of THIS round
        pre_snap = _snapshot_files(project_path)
        handoff = run_sdet_fix(goal=goal, failure_detail="Harness verification failed.",
                               test_output=fail_output, files=files_override,
                               project_path=project_path, chat_id=chat_id)
        handoffs.append(handoff)
        ok, output = adaptive_verify(project_path, task_hint)
        if ok:
            _learn_pattern(fail_output or "Harness verification failed.", project_path, pre_snap)
    return ok, rounds, output, handoffs


def run_graph_with_verify(prompt, workspace_path, chat_id, cancel_event=None,
                          max_fix_rounds=None):
    """Drop-in wrapper around run_and_sync_graph for the desktop app / bot.

    Yields every graph state snapshot as before, then — when the graph
    completes — runs the HARNESS verification gate on the workspace. On
    failure it spawns SDET-FIX (bounded rounds, no Dev, no orchestrator) and
    re-verifies. The final yielded snapshot carries:

      verify_status : "PASS" | "FAIL" | "ERROR" | None (gate disabled)
      verify_output : tail of the last verification output
      sdet_handoffs : list of SDET DONE/FILES/NOTE lines (only if fixed)
    """
    from sync_manager import run_and_sync_graph
    for snap in run_and_sync_graph(prompt, workspace_path=workspace_path,
                                   chat_id=chat_id, cancel_event=cancel_event):
        yield snap

    if os.environ.get("BSA_APP_VERIFY_GATE", "1") != "1":
        return
    if not workspace_path or not os.path.isdir(workspace_path):
        return

    try:
        from state_sync import safe_get_state
        if max_fix_rounds is None:
            max_fix_rounds = int(os.environ.get("BSA_APP_VERIFY_ROUNDS", "4"))
        ok, output = adaptive_verify(workspace_path, prompt)
        rounds = 0
        handoffs = []
        while not ok and rounds < max_fix_rounds:
            rounds += 1
            handoff = run_sdet_fix(
                goal=prompt,
                failure_detail="Harness verification failed after the agent completed.",
                test_output=output,
                project_path=workspace_path,
                chat_id=chat_id,
            )
            handoffs.append(str(handoff)[:400])
            ok, output = adaptive_verify(workspace_path, prompt)
        try:
            from state_sync import safe_update_state, shared_state
            safe_update_state({"outputs": {
                **shared_state.get("outputs", {}),
                "verify_status": "PASS" if ok else "FAIL",
                "verify_output": str(output)[-1500:],
            }})
        except Exception:
            pass
        try:
            snap = safe_get_state()
        except Exception:
            snap = {}
        snap["verify_status"] = "PASS" if ok else "FAIL"
        snap["verify_output"] = str(output)[-1500:]
        if handoffs:
            snap["sdet_handoffs"] = handoffs
        yield snap
    except Exception as e:
        try:
            from state_sync import safe_get_state
            snap = safe_get_state()
            snap["verify_status"] = "ERROR"
            snap["verify_error"] = str(e)[:300]
            yield snap
        except Exception:
            pass
