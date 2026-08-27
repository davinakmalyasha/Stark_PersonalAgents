"""bench_history_modes.py — Phase 0 live measurement: per-task wipe vs continuous thread.

Real DeepSeek API calls, real cache_hit/cache_miss billing from llm_stats.
Protocol:
  PRIME  — Task A runs once (4 calls), building a realistic tool-loop history.
  MODE W — Task B starts a FRESH thread ([System][anchor][TaskB])   (per-task isolation)
  MODE C — Task B continues A's full history                        (pi-style continuity)
Same anchor bytes in both modes (topology unchanged) so the ONLY variable
is whether prior history is present. Verdict = billed cost of completing
Task B under each mode + hit-rate breakdown.
Run: venv312/Scripts/python.exe scratch/bench_history_modes.py
"""
import os
import sys
import tempfile

sys.path.insert(0, r"D:\MyProject\LangChain")
sys.stdout.reconfigure(encoding="utf-8")

from sync.state_sync import shared_state  # noqa: E402

FABRICATED_RESULTS_A = [
    # round 1: file listing after search
    "[tool result] search_code 'def ' ->\n"
    "src/utils.py:1: def format_money(value):\n"
    "src/cart.ts:2: export function total(items) {\n"
    "src/main.py:5: def main():\n(3 matches)",
    # round 2: read_file of the target
    "[tool result] read_file src/utils.py (lines 1-12 of 12)\n"
    "def format_money(value):\n"
    "    \"\"\"Format a float as USD currency string.\"\"\"\n"
    "    return f\"${value:,.2f}\"\n\n\n"
    "def format_percent(value):\n"
    "    \"\"\"Format a float as a percentage string.\"\"\"\n"
    "    return f\"{value:.1%}\"\n",
    # round 3: edit_file diff confirmation
    "[tool result] edit_file src/utils.py OK (+6 -0)\n"
    "+def format_compact(value):\n"
    "+    \"\"\"Format large numbers compactly (1.2K, 3.4M).\"\"\"\n"
    "+    for unit in ('', 'K', 'M', 'B'):\n"
    "+        if abs(value) < 1000:\n"
    "+            return f\"{value:.1f}{unit}\"\n"
    "+        value /= 1000\n",
]

FABRICATED_RESULTS_B = [
    "[tool result] read_file src/utils.py (lines 1-18 of 18)\n"
    "def format_money(value):\n"
    "    return f\"${value:,.2f}\"\n\n\n"
    "def format_percent(value):\n"
    "    return f\"{value:.1%}\"\n\n\n"
    "def format_compact(value):\n"
    "    for unit in ('', 'K', 'M', 'B'):\n"
    "        if abs(value) < 1000:\n"
    "            return f\"{value:.1f}{unit}\"\n"
    "        value /= 1000\n",
    "[tool result] run_command python -m pytest tests/ -q\n"
    "[OK] Exit 0\n1 passed in 0.03s\n",
]

TASK_A = (
    "Add a format_compact helper to src/utils.py that renders large numbers "
    "compactly (e.g. 1.2K, 3.4M). Follow the existing docstring style."
)

TASK_B = (
    "Now add slugify(value) to src/utils.py (lowercase, spaces->dashes, strip "
    "non-alphanumerics) and a pytest test tests/test_utils.py::test_slugify "
    "that asserts slugify('Hello World!') == 'hello-world'. Run the test."
)


def _snapshot():
    t = shared_state.get("token_usage", {})
    calls = t.get("calls") or []
    return {
        "calls": len(calls),
        "input": t.get("total_input_tokens", 0),
        "hit": t.get("total_cache_hit_tokens", 0),
        "miss": t.get("total_cache_miss_tokens", 0),
        "cost": t.get("total_cost", 0.0),
    }


def _diff(before):
    after = _snapshot()
    return {k: after[k] - before[k] for k in after}


def _call(messages, where):
    from llm import invoke_messages_with_fallback

    return invoke_messages_with_fallback(
        role="Developer",
        messages=messages,
        temp=0.1,
        where=where,
        reasoning_effort="disabled",
    )


def build_sandbox():
    d = tempfile.mkdtemp(prefix="hist_modes_")
    os.makedirs(os.path.join(d, "src"), exist_ok=True)
    with open(os.path.join(d, "src", "utils.py"), "w", encoding="utf-8") as f:
        f.write('def format_money(value):\n    return f"${value:,.2f}"\n')
    os.makedirs(os.path.join(d, ".agent-memory"), exist_ok=True)
    from memory.memory_manager import ensure_project_cache

    ensure_project_cache(d)
    return d


def prime_task_a(static_system, anchor):
    """Run Task A once; return the finished thread (list of messages)."""
    from langchain_core.messages import AIMessage, HumanMessage, SystemMessage

    thread = [
        SystemMessage(content=static_system),
        HumanMessage(content=anchor),
        HumanMessage(content=f"## Task\n{TASK_A}"),
    ]
    for i, result in enumerate(FABRICATED_RESULTS_A, 1):
        text = _call(thread, f"A turn {i}")
        thread.append(AIMessage(content=(text or "")[:2000]))
        thread.append(HumanMessage(content=result))
    text = _call(thread, "A final")
    thread.append(AIMessage(content=(text or "")[:2000]))
    return thread


def run_task_b(prefix, label):
    """Append Task B to `prefix`, run its loop, return billed diff."""
    from langchain_core.messages import AIMessage, HumanMessage

    thread = list(prefix)
    thread.append(HumanMessage(content=f"## Task\n{TASK_B}"))
    before = _snapshot()
    for i, result in enumerate(FABRICATED_RESULTS_B, 1):
        text = _call(thread, f"{label} turn {i}")
        thread.append(AIMessage(content=(text or "")[:2000]))
        thread.append(HumanMessage(content=result))
    _call(thread, f"{label} final")
    stats = _diff(before)
    print(f"  [{label}] calls=+{stats['calls']} input={stats['input']:>6} "
          f"hit={stats['hit']:>6} miss={stats['miss']:>6} cost=${stats['cost']:.4f}")
    return stats


def main():
    try:
        from dotenv import load_dotenv

        load_dotenv()
    except Exception:
        pass
    from sync.state_sync import reset_shared_state, safe_update_state

    try:
        reset_shared_state()
    except Exception:
        pass
    # Direct DeepSeek balance is empty (402) — route through the OpenCode Go
    # subscription instead. shared_state wins over the persisted file, so the
    # user's saved provider choice stays untouched.
    if not os.getenv("OPENCODE_API_KEY"):
        print("OPENCODE_API_KEY missing — cannot reroute.")
        return
    safe_update_state({"llm_provider": "opencode"})
    print("provider: opencode (zen go route)")

    from agents.developer_agent import _build_system_prompt
    from langchain_core.messages import HumanMessage

    d = build_sandbox()

    TASK_A_TEXT = f"## Task\n{TASK_A}"
    static_system, anchor = _build_system_prompt(TASK_A_TEXT, d)
    print(f"sandbox ready | system={len(static_system)}ch anchor={len(anchor)}ch")

    snap0 = _snapshot()
    a_thread = prime_task_a(static_system, anchor)
    a_stats = _diff(snap0)
    print(f"[PRIME A] calls={a_stats['calls']} input={a_stats['input']} "
          f"hit={a_stats['hit']} miss={a_stats['miss']} cost=${a_stats['cost']:.4f}")

    # MODE W: per-task isolation — brand-new thread, identical System+anchor bytes.
    print("\n=== MODE W: wipe (fresh thread per task) ===")
    w_thread = [a_thread[0], a_thread[1]]
    w_stats = run_task_b(w_thread, "W")

    # MODE C: continuous thread — Task B appends to A's real history.
    print("\n=== MODE C: continuous (one thread) ===")
    c_prefix = list(a_thread)  # ends with A's final AI summary
    c_stats = run_task_b(c_prefix, "C")

    total = shared_state.get("token_usage", {})
    print("\n=== VERDICT ===")
    for name, s in (("W", w_stats), ("C", c_stats)):
        hit_pct = 100.0 * s["hit"] / max(1, s["input"])
        print(f"{name}: input={s['input']}  hits={s['hit']} ({hit_pct:.1f}%)  "
              f"miss={s['miss']}  cost=${s['cost']:.4f}")
    cheaper = "C" if c_stats["cost"] <= w_stats["cost"] else "W"
    delta = abs(c_stats["cost"] - w_stats["cost"])
    print(f"\nCHEAPER MODE FOR RELATED TASK B: {cheaper} (delta ${delta:.4f})")
    print(f"experiment total cost: ${total.get('total_cost', 0):.4f}")
    print("NOTE: simulated tool results; single effort config; one provider.")


if __name__ == "__main__":
    main()
