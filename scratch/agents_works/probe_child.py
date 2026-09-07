"""Probe: reproduce the instant-empty UI child turn with tracing."""
import os
import sys
import time
import traceback

ROOT = r"D:\MyProject\LangChain"
os.chdir(ROOT)
sys.path.insert(0, ROOT)
os.environ.setdefault("HARNESS_ROOT", ROOT)

from sync.state_sync import reset_shared_state, safe_update_state

reset_shared_state()
safe_update_state({
    "selected_model": "Automatic Fallback",
    "selected_temp": 0.2,
    "is_general": False,
    "failed_models": [],
    "token_usage": {"total_input_tokens": 0, "total_output_tokens": 0,
                    "total_cache_hit_tokens": 0, "total_cache_miss_tokens": 0,
                    "total_cost": 0.0, "calls": []},
})

# Trace the backend call inside _run_turn (it swallows exceptions).
import tools.AddOnTool.subagentTools as st
_orig = st._specialist_call_backend


def _traced(name, messages, native_tools=None, schema=None, model=None,
            temp=0.2, reasoning_effort=None, where=""):
    print(f"[trace] backend enter name={name} n_msgs={len(messages or [])} "
          f"n_tools={len(native_tools or [])} temp={temp}", flush=True)
    t0 = time.time()
    try:
        res = _orig(name, messages, native_tools=native_tools, schema=schema,
                    model=model, temp=temp, reasoning_effort=reasoning_effort,
                    where=where)
        print(f"[trace] backend OK dt={time.time()-t0:.1f}s "
              f"type={type(res).__name__} len={len(res) if isinstance(res, tuple) else '?'}",
              flush=True)
        return res
    except Exception:
        print(f"[trace] backend RAISED dt={time.time()-t0:.1f}s:", flush=True)
        traceback.print_exc()
        raise


st._specialist_call_backend = _traced
# _run_turn imported the symbol? check: it does `from ... import _specialist_call_backend`
# INSIDE _run_turn (line 234, function-local import) -> picks up patched attr. Good.
import agents.subagent_manager as sm

ws = os.path.join(ROOT, "scratch", "agents_works", "design-clean")
print("[probe] start_continuable…", flush=True)
t0 = time.time()
try:
    out = sm.start_continuable("probe-child-1", ws, "UI",
                               "Reply with the single word PONG. Do nothing else.",
                               label="probe")
    print(f"[probe] DONE dt={time.time()-t0:.1f}s -> {str(out)[:400]!r}", flush=True)
except Exception:
    print("[probe] RAISED:", flush=True)
    traceback.print_exc()
