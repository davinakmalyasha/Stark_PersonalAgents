"""Self-test for P5 ask-user tool. Run: venv312\\Scripts\\python.exe scratch/test_user_questions.py"""
import os
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from user_questions import (
    UserQuestionError, ask, pending_batches, resolve_batch, _parse_questions,
)

PASS = 0


def check(label, cond):
    global PASS
    if not cond:
        raise AssertionError(f"FAIL: {label}")
    PASS += 1
    print(f"  ok {PASS}: {label}")


def main():
    # ── parsing ──
    qs = _parse_questions([
        {
            "id": "q1", "question": "Proceed with this plan?",
            "options": ["Yes, proceed", "Revise"],
            "intent": {"kind": "plan-review", "approve": "Yes, proceed"},
            "detail": "planning.md outline",
        },
        {"id": "q2", "question": "Pick stack", "options": ["Flask", "FastAPI"], "multiSelect": True},
        {"id": "bad"},  # missing question -> dropped
    ])
    check("parses 2 valid questions", len(qs) == 2)
    check("plan-review intent", qs[0].intent is not None and qs[0].intent.approve == "Yes, proceed")
    check("multiSelect flag", qs[1].multi_select is True)

    # ── HTTP store provider round-trip (threaded) ──
    result_holder = {}

    def asker():
        try:
            result_holder["answers"] = ask(
                [{"id": "q1", "question": "Proceed?", "options": ["Yes", "No"]}],
                timeout=30,
            )
        except Exception as e:
            result_holder["error"] = e

    t = threading.Thread(target=asker, daemon=True)
    t.start()
    deadline = time.time() + 10
    batches = []
    while time.time() < deadline:
        batches = pending_batches()
        if batches:
            break
        time.sleep(0.05)
    check("batch registered", len(batches) == 1)
    ok = resolve_batch(batches[0]["batch_id"], [{"id": "q1", "selected": ["Yes"]}])
    check("batch resolved", ok)
    check("unknown batch rejected", resolve_batch("nope", []) is False)
    t.join(timeout=10)
    check("ask returned answer", "q1" in [a.id for a in result_holder.get("answers", [])]
          and result_holder["answers"][0].selected == ["Yes"])

    # ── empty questions ──
    try:
        ask([])
        check("empty raises", False)
    except UserQuestionError as e:
        check("empty raises EMPTY_QUESTIONS", e.code == "EMPTY_QUESTIONS")

    # ── timeout ──
    try:
        ask([{"id": "x", "question": "q"}], timeout=0.3)
        check("timeout raises", False)
    except UserQuestionError as e:
        check("timeout raises ASK_ABORTED", e.code == "ASK_ABORTED")

    # ── tools.ask_user end-to-end ──
    import tools
    tools.ensure_tool_registry()

    tool_result = {}

    def tool_asker():
        tool_result["res"] = tools.ask_user([
            {"id": "approve", "question": "Approve the plan?",
             "options": ["Yes", "No"],
             "intent": {"kind": "plan-review", "approve": "Yes"}},
        ], timeout=30)

    t2 = threading.Thread(target=tool_asker, daemon=True)
    t2.start()
    deadline = time.time() + 10
    batches = []
    while time.time() < deadline:
        batches = pending_batches()
        if batches:
            break
        time.sleep(0.05)
    check("tool batch registered", len(batches) == 1)
    resolve_batch(batches[0]["batch_id"], [{"id": "approve", "selected": ["Yes"]}])
    t2.join(timeout=10)
    check("tool formats answers", "approve: Yes" in tool_result.get("res", ""))

    # ── registry: ask_user registered + surfaced ──
    reg = tools.ensure_tool_registry()
    d = reg.get("ask_user")
    check("ask_user registered", d is not None and d.description.startswith("ask_user("))
    schemas = reg.schemas_for()
    check("ask_user in structural schemas", any(s["function"]["name"] == "ask_user" for s in schemas))
    from tools_pkg import get_native_tools
    native = get_native_tools()
    check("ask_user in native schemas", any(t["function"]["name"] == "ask_user" for t in native))

    # ── orchestrator surfaces include ask_user ──
    tools._load_subagents()
    gp = tools._SUBAGENTS_REGISTRY.get("general-purpose")
    check("general-purpose has ask_user", gp is not None and "ask_user" in gp.get("tools", []))

    print(f"\nALL {PASS} CHECKS PASSED")


if __name__ == "__main__":
    main()
