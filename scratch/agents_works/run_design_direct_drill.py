"""Phase-1 validation drill: drive the NEW run_ui_design_direct runner
(same entry the server uses for workspace_kind='design') on a FRESH folder.

- Fresh target: scratch/agents_works/design-clean/ (no leftovers).
- ask_user served by scripted provider on BOTH user_questions instances.
- Logs next to this file: drill_direct_stdout.log (via redirect),
  ask_log_direct_<chat>.jsonl, direct_snapshots_<chat>.jsonl.
"""
import faulthandler
import json
import os
import sys
import time

faulthandler.dump_traceback_later(600, exit=True)  # hang watchdog

if os.environ.get("DRILL_MINIMAL_PROMPT", "0") == "1":
    print("[drill] EXPERIMENT MODE (unexpected for Phase 1)", flush=True)
else:
    print("[drill] CONTROL MODE: normal prompts/specs", flush=True)

ROOT = r"D:\MyProject\LangChain"
os.chdir(ROOT)
sys.path.insert(0, ROOT)
sys.path.insert(0, os.path.join(ROOT, "Desktop"))
os.environ.setdefault("HARNESS_ROOT", ROOT)
# Drill-only backend pin (documented SUBAGENT_MODEL_BACKEND precedence):
# command-mode paid/light endpoint hangs from here, so validate the runner
# on the free pool. Production default (env unset) is untouched.
os.environ.setdefault("SUBAGENT_MODEL_BACKEND",
                      os.environ.get("DRILL_SUBAGENT_BACKEND", "free"))
print(f"[drill] subagent backend pin: {os.environ.get('SUBAGENT_MODEL_BACKEND')}",
      flush=True)

HERE = os.path.join(ROOT, "scratch", "agents_works")
CHAT_ID = os.environ.get("DRILL_CHAT_ID", "design-direct-4")
ASK_LOG = os.path.join(HERE, f"ask_log_direct_{CHAT_ID}.jsonl")
SNAP_LOG = os.path.join(HERE, f"direct_snapshots_{CHAT_ID}.jsonl")
TARGET = os.path.join(HERE, "design-clean")
os.makedirs(TARGET, exist_ok=True)

for p in (ASK_LOG, SNAP_LOG):
    try:
        os.remove(p)
    except OSError:
        pass

from Desktop.user_questions import AnswerItem, register_provider
import user_questions as _uq_bare


def drill_provider(questions, timeout, chat_id=None):
    rec = [{"id": q.id, "question": q.question,
            "options": [o.label for o in q.options]} for q in questions]
    with open(ASK_LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec) + "\n")
    print(f"[drill-provider] asked: {json.dumps(rec)[:400]}", flush=True)
    out = []
    for q in questions:
        labels = [o.label for o in q.options]
        pick = None
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
        print(f"[drill-provider] auto-pick for '{q.id}': {pick!r}"[:200], flush=True)
        out.append(AnswerItem(id=q.id, selected=[pick] if pick else []))
    return out


register_provider("drill-auto", drill_provider)
_uq_bare.register_provider("drill-auto", drill_provider)

BRIEF = (
    "Design a landing page for a fictional coffee subscription called "
    "'North Roast' (hero, 3 pricing tiers, footer)."
)

print(f"[drill] target={TARGET} chat={CHAT_ID}", flush=True)
print(f"[drill] start={time.strftime('%H:%M:%S')}", flush=True)

from sync.state_sync import reset_shared_state, safe_update_state

reset_shared_state()
safe_update_state({
    "selected_model": "Automatic Fallback",
    "selected_temp": 0.7,
    "is_general": False,
    "failed_models": [],
    "token_usage": {"total_input_tokens": 0, "total_output_tokens": 0,
                    "total_cache_hit_tokens": 0, "total_cache_miss_tokens": 0,
                    "total_cost": 0.0, "calls": []},
})

from agents.ui_design_direct import run_ui_design_direct

n = 0
try:
    for snap in run_ui_design_direct(BRIEF, workspace_path=TARGET,
                                     chat_id=CHAT_ID, max_rounds=4):
        n += 1
        try:
            log = str((snap.get("live_terminal_log", "") or ""))[-300:]
            outs = sorted((snap.get("outputs", {}) or {}).keys())
        except Exception:
            log, outs = "?", []
        with open(SNAP_LOG, "a", encoding="utf-8") as f:
            f.write(json.dumps({"n": n, "outputs": outs, "log_tail": log,
                                "t": time.strftime("%H:%M:%S")}) + "\n")
        if n % 5 == 0:
            print(f"[drill] {n} snapshots... last: {log[-120:]}", flush=True)
    print(f"[drill] DONE snapshots={n} end={time.strftime('%H:%M:%S')}", flush=True)
except Exception:
    print("[drill] EXCEPTION:", flush=True)
    import traceback as _tb
    _tb.print_exc()
    raise
