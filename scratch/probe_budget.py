"""probe_budget.py — verify the DeepSeek endpoint honors thinking.budget_tokens.

Two tiny calls (<<$0.01): one with a 200-token reasoning budget, one without.
If the field is honored, the capped call's reasoning_content should be much
shorter (<= ~800 chars for a 200-token cap); if rejected, the call errors.

Run:  venv312/Scripts/python.exe scratch/probe_budget.py
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import HumanMessage, SystemMessage  # noqa: E402

from llm import _get_deepseek_client, _map_model  # noqa: E402

PROMPT = ("You are a coding agent. In 5-8 sentences, explain how you would "
          "diagnose and fix a floating-point precision bug in a money display "
          "function, including the exact code you would write.")


def _call(effort, budget):
    model = _map_model("deepseek-v4-flash")
    llm = _get_deepseek_client(model, 0.2, role="Developer",
                               reasoning_effort=effort, reasoning_budget=budget,
                               messages=[SystemMessage(content="You are a coding agent.")])
    msgs = [SystemMessage(content="You are a coding agent."),
            HumanMessage(content=PROMPT)]
    r = llm.invoke(msgs)
    reasoning = (r.additional_kwargs or {}).get("reasoning_content", "") or ""
    content = (getattr(r, "content", "") or "")[:80]
    return len(reasoning), content


def main():
    print("== probe: low + budget_tokens=200 ==")
    try:
        r1_len, c1 = _call("low", 200)
        print(f"  OK  reasoning_chars={r1_len} (cap ~800) content={c1!r}")
    except Exception as e:
        print(f"  ERR {type(e).__name__}: {str(e)[:300]}")
        print("  -> endpoint REJECTED budget_tokens; ladder falls back to effort levels only")
        return 1

    print("== probe: low, no budget ==")
    try:
        r2_len, c2 = _call("low", None)
        print(f"  OK  reasoning_chars={r2_len} content={c2!r}")
    except Exception as e:
        print(f"  ERR {type(e).__name__}: {str(e)[:300]}")
        return 1

    print()
    if r1_len <= 900:
        print(f"RESULT: budget_tokens HONORED ({r1_len} chars with cap vs {r2_len} without) "
              "-> keep the per-turn budget plumbing (llm.py extra_body.thinking.budget_tokens)")
    else:
        print(f"RESULT: budget_tokens IGNORED ({r1_len} chars with cap, similar to {r2_len} without) "
              "-> budget is a no-op; effort levels alone carry the ladder")
    return 0


if __name__ == "__main__":
    sys.exit(main())
