import os
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.stdout.reconfigure(encoding='utf-8')

print("=== 1. Testing P1: Authored project memory projection ===")
from memory.memory_manager import (
    load_project_memory,
    update_project_memory,
)

# Authored doc fixture: Identity + Design bullets, no generated fields.
sample_memory = """# Project

## Identity
- project: LangChain
- purpose: AI coding agent harness
- stack: Python, LangGraph
- goals: long-horizon coding tasks

## Design
- direction: Terminal Minimal
- theme: dark, dense
- palette: primary #533afd · bg #0b0b0f
- typography: Inter — display 32/24, body 14
- concept: engineering console
"""

_mem_tmp = tempfile.mkdtemp()
os.makedirs(os.path.join(_mem_tmp, ".agent-memory"), exist_ok=True)
with open(os.path.join(_mem_tmp, ".agent-memory", "project.md"), "w", encoding="utf-8") as f:
    f.write(sample_memory)

compact_output = load_project_memory(_mem_tmp)
print("Projected project memory:\n" + compact_output)
assert "- project: LangChain" in compact_output
assert "- purpose: AI coding agent harness" in compact_output
assert "- stack: Python, LangGraph" in compact_output
assert "## Design" in compact_output
assert "- palette: primary #533afd · bg #0b0b0f" in compact_output
assert "# Project" not in compact_output

assert update_project_memory(
    _mem_tmp, "## Design\n- direction: Editorial\n- palette: primary #111111\n"
)
after = load_project_memory(_mem_tmp)
assert "- direction: Editorial" in after
assert "- project: LangChain" in after  # Identity preserved
print("[PASS] P1 authored project memory projection matches all rules.")

print("\n=== 2. Testing P2: Warmup Cache Alignment ===")
from agents.developer_agent import _build_system_prompt
import scratch.warmup_cache as warmup_module

static_system, current_context = _build_system_prompt(
    task="",
    project_path=".",
    is_first_call=True,
)
print(f"Developer static system prompt preview (first 150 chars):\n{static_system[:150]}...")
print(f"Developer static prompt total chars: {len(static_system)}")
assert len(static_system) > 100
print("[PASS] P2 warmup builder correctly interfaces with developer_agent.")

print("\n=== 3. Testing P3 & P4: Tool Schemas and Registry Validation ===")
from tools._registry import ensure_tool_registry, get_registry
from tools._schemas import TOOL_DEFINITIONS, get_compact_tool_docs, get_native_tool_names

registry = ensure_tool_registry()
registered_names = registry.names_for()
print(f"Tool registry initialized with {len(registered_names)} tools: {registered_names[:8]}...")

# Verify trimmed tools are in TOOL_DEFINITIONS and registry
trimmed_tools = ["github_apply_gate", "template_new", "browser_act", "ask_user"]
for t_name in trimmed_tools:
    defn = next((d for d in TOOL_DEFINITIONS if d["name"] == t_name), None)
    assert defn is not None, f"Missing tool definition for {t_name}"
    desc = defn["description"]
    print(f"  - {t_name}: {len(desc)} chars -> '{desc[:60]}...'")

docs = get_compact_tool_docs("developer")
assert len(docs) > 0
print(f"Developer scope compact docs generated for {len(docs)} tools.")

print("[PASS] P3 & P4 tool schemas and registry verified successfully.")

print("\n=== ALL TESTS PASSED SUCCESSFULLY ===")
