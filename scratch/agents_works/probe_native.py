"""Probe: one Dev-native turn calling read_file, with full traceback."""
import os
import sys
import time
import traceback

ROOT = r"D:\MyProject\LangChain"
os.chdir(ROOT)
sys.path.insert(0, ROOT)
os.environ.setdefault("HARNESS_ROOT", ROOT)

TARGET = os.path.join(ROOT, "scratch", "agents_works", "design")

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

from llm.deps import WorkspaceDeps
from tools import get_native_tools
from llm.native_tools import run_native_agent

deps = WorkspaceDeps.from_shared_state(role="Developer", chat_id="probe-n1",
                                       project_path=TARGET)
tools = get_native_tools("developer")
print(f"[probe] {len(tools)} native tool schemas", flush=True)

msgs = [{"role": "user",
         "content": "Call read_file with file_path "
                    "'.agents/skills/ui-ux-design/SKILL.md' and then reply "
                    "with ONLY the value of its 'name' frontmatter field."}]
try:
    t0 = time.time()
    text, lc, reasoning = run_native_agent(
        role="Developer", messages=msgs, tools=tools, temp=0.2,
        deps=deps, system_prompt="You are a probe. Use tools when asked.",
    )
    print(f"[probe] OK dt={time.time()-t0:.0f}s", flush=True)
    print(f"[probe] TEXT: {text!r}", flush=True)
except Exception:
    print("[probe] RAISED:", flush=True)
    traceback.print_exc()
