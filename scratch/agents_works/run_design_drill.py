"""Design-loop drill (Phase 0 validation).

Runs the REAL pipeline (orchestrator -> Dev -> UI subagent) on a design
brief that points at .agents/skills/ui-ux-design/SKILL.md.

- ask_user is served by a scripted provider that logs every question and
  auto-picks the Linear/dark direction (mechanics validation, not taste).
- All artifacts must land in scratch/agents_works/design/ only.
- Logs: ask_log.jsonl, drill_snapshots.jsonl (both next to this file).

Usage (background):
  venv312\\Scripts\\python.exe scratch\\agents_works\\run_design_drill.py
"""
import json
import os
import sys
import time

# TEMP A/B: pick which system prompt to test via env. Run control (normal
# Dev prompt) with DRILL_MINIMAL_PROMPT unset/0; run experiment (minimal
# prompt) with DRILL_MINIMAL_PROMPT=1. Unwired = env-gated, default off.
if os.environ.get("DRILL_MINIMAL_PROMPT", "0") == "1":
    print("[drill] EXPERIMENT MODE: minimal system prompt enabled", flush=True)
else:
    print("[drill] CONTROL MODE: normal Dev system prompt", flush=True)

ROOT = r"D:\MyProject\LangChain"
os.chdir(ROOT)
sys.path.insert(0, ROOT)
os.environ.setdefault("HARNESS_ROOT", ROOT)

HERE = os.path.join(ROOT, "scratch", "agents_works")
ASK_LOG = os.path.join(HERE, "ask_log.jsonl")
SNAP_LOG = os.path.join(HERE, "drill_snapshots.jsonl")
TARGET = os.path.join(HERE, "design")

# Fresh logs for this run (log names include mode+chat so A/B never mixes)
_MODE_TAG = "min" if os.environ.get("DRILL_MINIMAL_PROMPT", "0") == "1" else "ctl"
ASK_LOG = os.path.join(HERE, f"ask_log_{_MODE_TAG}_{os.environ.get('DRILL_CHAT_ID','drill-design-7')}.jsonl")
SNAP_LOG = os.path.join(HERE, f"drill_snapshots_{_MODE_TAG}_{os.environ.get('DRILL_CHAT_ID','drill-design-7')}.jsonl")
for p in (ASK_LOG, SNAP_LOG):
    try:
        os.remove(p)
    except OSError:
        pass

from Desktop.user_questions import AnswerItem, register_provider
# NOTE: tools/AddOnTool/orchestrationTools.py imports the BARE
# `user_questions` module (Desktop/ on sys.path), which is a DIFFERENT
# module instance from `Desktop.user_questions`. The agent's ask_user
# reads the bare instance — register the provider on BOTH.
sys.path.insert(0, os.path.join(ROOT, "Desktop"))
import user_questions as _uq_bare


def drill_provider(questions, timeout, chat_id=None):
    rec = [
        {"id": q.id, "question": q.question,
         "options": [o.label for o in q.options]}
        for q in questions
    ]
    with open(ASK_LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec) + "\n")
    print(f"[drill-provider] asked: {json.dumps(rec)}", flush=True)
    out = []
    for q in questions:
        labels = [o.label for o in q.options]
        pick = None
        # Plan-review gates: always take the approve option.
        intent = getattr(q, "intent", None)
        if intent is not None and getattr(intent, "kind", "") == "plan-review":
            want = (getattr(intent, "approve", "") or "").strip().lower()
            pick = next((l for l in labels if l.strip().lower() == want), None)
        if pick is None:
            pick = next((l for l in labels if "linear" in l.lower()), None)
        if pick is None:
            pick = next((l for l in labels if "dark" in l.lower()), None)
        if pick is None and labels:
            pick = labels[0]
        print(f"[drill-provider] auto-pick for '{q.id}': {pick!r}", flush=True)
        out.append(AnswerItem(id=q.id, selected=[pick] if pick else []))
    return out


register_provider("drill-auto", drill_provider)
_uq_bare.register_provider("drill-auto", drill_provider)

BRIEF = (
    "Design drill — follow instructions exactly, do NOT create any project, "
    "do NOT touch anything outside the folders named below.\n\n"
    "1. Read the file D:\\MyProject\\LangChain\\.agents\\skills\\ui-ux-design\\SKILL.md "
    "(absolute path — repo-relative paths resolve against the project "
    "folder, not the repo root) and follow its loop end-to-end "
    "(direction-lock via ask_user, build, self-critique, visual verify, "
    "finalize). The shared catalog is at "
    "D:\\MyProject\\LangChain\\design-references\\INDEX.md (absolute path).\n"
    "2. The design brief: a landing page for a fictional coffee subscription "
    "called 'North Roast' (hero, 3 pricing tiers, footer).\n"
    "3. Work ONLY inside scratch/agents_works/design/ (repo-relative). You "
    "may READ design-references/INDEX.md and design-references/*.md. Write "
    "the artifact HTML, any screenshot evidence notes, and the final "
    "DESIGN.md into scratch/agents_works/design/ only.\n"
    "4. Keep it to ONE iterate pass max after the first render, then "
    "finalize. When done, report: files written, winning direction, "
    "screenshot path.\n"
    "5. TURN DISCIPLINE: never end a turn with text-only until the "
    "artifact HTML, the screenshot verification, and DESIGN.md all "
    "exist. After each ask_user answer, continue with tools in the "
    "same turn. Narrate only in the final report."
)

import faulthandler
faulthandler.dump_traceback_later(480, exit=True)  # hang watchdog: dump + exit

CHAT_ID = os.environ.get("DRILL_CHAT_ID", "drill-design-7")  # A/B: fresh per run
print(f"[drill] target={TARGET} chat={CHAT_ID}", flush=True)
print(f"[drill] start={time.strftime('%H:%M:%S')}", flush=True)

# NOTE: orchestrator graph path is broken by in-progress work on this
# branch (agents/graph/ untracked refactor; run_llm_turn(state) vs
# def _run_native_orchestrator_turn(state, deps)). Drill drives Dev
# directly — same Dev->UI chain, minus the broken orchestrator node.
from sync.state_sync import reset_shared_state, safe_update_state

# Mirror server/routers/runs.py seeding (direct node calls skip it).
reset_shared_state()
safe_update_state({
    "selected_model": "Automatic Fallback",
    "selected_temp": 0.7,
    "is_general": False,
    "failed_models": [],
    "token_usage": {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cache_hit_tokens": 0,
        "total_cache_miss_tokens": 0,
        "total_cost": 0.0,
        "calls": [],
    },
})

from agents.developer_agent import developer_node

state = {"client_request": BRIEF, "project_path": TARGET,
         "chat_id": CHAT_ID}
try:
    result = developer_node(state)
    print(f"[drill] DONE end={time.strftime('%H:%M:%S')}", flush=True)
    try:
        keys = sorted(result.keys()) if isinstance(result, dict) else type(result).__name__
    except Exception:
        keys = "?"
    print(f"[drill] result keys: {keys}", flush=True)
    with open(SNAP_LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps({"result_keys": keys,
                            "t": time.strftime("%H:%M:%S")}) + "\n")
    if isinstance(result, dict):
        for k in ("final", "reply", "answer", "text", "output", "response"):
            v = result.get(k)
            if isinstance(v, str) and v.strip():
                print("[drill] RESULT_TEXT_BEGIN", flush=True)
                print(v[-3000:], flush=True)
                print("[drill] RESULT_TEXT_END", flush=True)
                break
except Exception as e:
    print(f"[drill] EXCEPTION: {type(e).__name__}: {e}", flush=True)
    import traceback as _tb
    _tb.print_exc()
    raise
