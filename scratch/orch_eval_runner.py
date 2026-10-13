"""Orchestrator behaviour eval runner.

Deterministic process checks over the delegation ledger + optional free-pool
LLM judge. Live mode touches the real environment (it drives the real
orchestrator), so it is opt-in:

  venv\\Scripts\\python.exe scratch\\orch_eval_runner.py            # list tasks only (safe)
  set ORCH_EVAL_LIVE=1 && venv\\Scripts\\python.exe scratch\\orch_eval_runner.py
  set ORCH_EVAL_JUDGE=1  # + free-pool judge on the final reply

Metrics per task: jobs spawned, agents used, duplicate jobs, artifacts written,
method words in outcome/acceptance (over-dictation), plus the final reply.
"""
from __future__ import annotations

import json
import os
import re
import sys

_HERE = os.path.dirname(os.path.abspath(__file__))
_ROOT = os.path.dirname(_HERE)
sys.path.insert(0, _ROOT)
sys.path.insert(0, os.path.join(_ROOT, "Desktop"))

TASKS_PATH = os.path.join(_HERE, "orch_eval_tasks.json")

#: Words that signal the orchestrator prescribed HOW instead of WHAT.
_METHOD_WORDS = re.compile(
    r"\b(use|using|install|npm|pip|yarn|react|next\.js|vue|fastapi|django|flask|"
    r"pytest|jest|mocha|tdd|refactor first|step by step|line by line)\b",
    re.IGNORECASE,
)


def _load_tasks() -> list[dict]:
    with open(TASKS_PATH, "r", encoding="utf-8") as f:
        return json.load(f).get("tasks", [])


def _entries_for(chat_id: str) -> list[dict]:
    from agents import delegation
    with delegation._LOCK:
        return [dict(e) for e in delegation._DELEGATIONS.values()
                if e.get("parent") == chat_id]


def _duplicates(entries: list[dict]) -> list[str]:
    seen: dict[tuple, str] = {}
    dupes: list[str] = []
    for e in entries:
        key = (e.get("agent_display", ""), str(e.get("task", ""))[:120])
        if key in seen:
            dupes.append(e.get("id", ""))
        else:
            seen[key] = e.get("id", "")
    return dupes


def _method_hits(entries: list[dict]) -> list[str]:
    hits: list[str] = []
    for e in entries:
        for field in ("outcome", "acceptance"):
            text = str(e.get(field) or "")
            m = _METHOD_WORDS.search(text)
            if m:
                hits.append(f"{e.get('id', '')}.{field}: '{m.group(0)}'")
    return hits


def _judge(prompt: str, reply: str) -> str:
    """Optional free-pool judge: 0-1 score + PASS/FAIL on wrap-up quality."""
    try:
        from llm.free_providers import complete
        ask = (
            "You are grading an assistant's final reply to the user request.\n"
            "Score 0.0-1.0 and answer PASS or FAIL for: honest (names failures), "
            "complete (covers every job), plain English (no jargon dump).\n"
            "Output exactly: score=<0-1> verdict=<PASS|FAIL> why=<one line>\n\n"
            f"USER REQUEST:\n{prompt}\n\nFINAL REPLY:\n{reply[:3000]}"
        )
        res = complete(prompt=ask, capability="text")
        return str((res or {}).get("reply") or "").strip()[:300]
    except Exception as e:
        return f"(judge unavailable: {e})"


def main() -> int:
    tasks = _load_tasks()
    live = os.environ.get("ORCH_EVAL_LIVE") == "1"
    use_judge = os.environ.get("ORCH_EVAL_JUDGE") == "1"
    if not live:
        print(f"[orch-eval] {len(tasks)} task(s) loaded. Set ORCH_EVAL_LIVE=1 to run them.")
        for t in tasks:
            print(f"  - {t['id']}: {t['prompt'][:80]}")
        return 0

    from agents import delegation
    from agents.orchestrator_agent import orchestrator_node

    passed = 0
    for t in tasks:
        chat_id = f"orch-eval-{t['id']}"
        delegation.reset_delegations()
        try:
            res = orchestrator_node({
                "client_request": t["prompt"],
                "project_path": "",
                "chat_id": chat_id,
            })
            reply = str(res.get("agent_report") or "")
        except Exception as e:
            print(f"[FAIL] {t['id']}: orchestrator crashed: {e}")
            continue
        entries = _entries_for(chat_id)
        jobs = len(entries)
        agents = sorted({e.get("agent_display", "") for e in entries})
        dupes = _duplicates(entries)
        artifacts = sum(1 for e in entries if e.get("artifact") and os.path.isfile(e["artifact"]))
        method = _method_hits(entries)

        checks: list[str] = []
        if not (int(t.get("jobs_min", 0)) <= jobs <= int(t.get("jobs_max", 99))):
            checks.append(f"jobs {jobs} outside [{t.get('jobs_min')}, {t.get('jobs_max')}]")
        want = set(t.get("agents_any") or [])
        if want and not (want & set(agents)):
            checks.append(f"agents {agents} missing any of {sorted(want)}")
        if dupes:
            checks.append(f"duplicate jobs: {dupes}")
        if jobs and artifacts == 0:
            checks.append("no job artifacts written")
        if method:
            checks.append(f"method prescribed: {method}")

        ok = not checks
        passed += 1 if ok else 0
        tag = "PASS" if ok else "FAIL"
        print(f"[{tag}] {t['id']}: jobs={jobs} agents={agents} artifacts={artifacts}")
        for c in checks:
            print(f"       - {c}")
        if use_judge:
            print(f"       judge: {_judge(t['prompt'], reply)}")
        delegation.reset_delegations()

    print(f"\n[orch-eval] {passed}/{len(tasks)} passed deterministic checks.")
    return 0 if passed == len(tasks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
