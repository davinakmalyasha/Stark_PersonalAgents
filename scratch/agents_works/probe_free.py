"""Probe: why does the UI (free pool) turn fail instantly?"""
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

from llm.free_providers import free_native_turn, FreeProviders

print("[probe] pool providers:", flush=True)
try:
    pool = FreeProviders()
    print("[probe] pool OK:", type(pool).__name__, flush=True)
except Exception:
    traceback.print_exc()

from tools import get_native_tools

ui_tools = [d for d in get_native_tools()
            if d.get("function", {}).get("name") in
            {"browser_act", "read_file", "search_code", "run_command",
             "edit_file", "ask_user"}]
print(f"[probe] UI tool schemas: {len(ui_tools)}", flush=True)

print("[probe] single turn WITH tools…", flush=True)
t0 = time.time()
try:
    res = free_native_turn(
        "UI",
        [{"role": "user", "content": "Reply with the single word PONG."}],
        tools=ui_tools or None,
        capability="coding",
        temperature=0.2,
    )
    print(f"[probe] OK dt={time.time()-t0:.0f}s -> {str(res)[:300]!r}", flush=True)
except Exception:
    print(f"[probe] RAISED dt={time.time()-t0:.0f}s:", flush=True)
    traceback.print_exc()
