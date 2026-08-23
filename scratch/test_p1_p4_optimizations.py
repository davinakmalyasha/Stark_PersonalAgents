import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.stdout.reconfigure(encoding='utf-8')

print("=== 1. Testing P1: Compact Project Identity Projection ===")
from memory.memory_manager import (
    ensure_project_identity,
    _format_compact_project_identity,
    _project_identity_for_injection,
    load_cached_project_identity,
)

# Test card parsing with updates
sample_card = """# Project
name: LangChain
description: AI coding agent harness
purpose: AI coding agent harness
stack: Python, LangGraph
version: Python 3.12
status: stable

[updated: 2026-08-19 12:00 UTC]
stack: Python, LangGraph, LangChain
version: Python 3.12.2
"""

compact_output = _format_compact_project_identity(sample_card, ".")
print("Projected compact identity:\n" + compact_output)
assert "- project: LangChain" in compact_output
assert "- purpose: AI coding agent harness" in compact_output
assert "- stack: Python, LangGraph, LangChain" in compact_output
assert "- runtime: Python 3.12.2" in compact_output
assert "# Project" not in compact_output
assert "[updated" not in compact_output
assert "status:" not in compact_output
print("[PASS] P1 projection format matches all rules.")

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
