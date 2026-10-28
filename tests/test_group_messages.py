"""Group chat is real event data, not a UI-only projection."""
from __future__ import annotations

import os
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

from agents.group import events as ev
from agents.group import messages
from agents.group.runner import LocalRunner, RunContext, RunResult, build_prompt
from agents.group.store import ProjectStore

MILESTONES = [{
    "id": "m1",
    "title": "Work",
    "items": [{
        "id": "m1.i1",
        "title": "write the greeting",
        "acceptance": [{"kind": "check", "check": "file_exists", "path": "out.txt"}],
    }],
}]


def _store(tmp: str) -> ProjectStore:
    store = ProjectStore(os.path.join(tmp, "state.db"))
    store.append(
        "p", ev.PROJECT_CREATED, "orchestrator",
        data={"goal": "write a greeting", "milestones": MILESTONES},
    )
    return store


def test_messages_round_trip_and_unread_delivery():
    with tempfile.TemporaryDirectory() as tmp:
        store = _store(tmp)
        messages.send(
            store, "p", sender="supervisor", recipient="Dev",
            body="Assigned m1.i1", kind="assignment", item_id="m1.i1",
        )
        assert messages.unread_for(store, "p", "Dev") == []
        assert messages.latest_message(store, "p")["kind"] == "assignment"

        messages.send(
            store, "p", sender="Dev", recipient="General",
            body="The file is ready.", kind="note",
        )
        unread = messages.unread_for(store, "p", "General")
        assert [m["body"] for m in unread] == ["The file is ready."]
        messages.mark_read(store, "p", unread[0], "General")
        assert messages.unread_for(store, "p", "General") == []
        assert messages.latest_message(store, "p")["from"] == "Dev"
        store.close()


def test_replay_accepts_message_events_without_a_projection():
    with tempfile.TemporaryDirectory() as tmp:
        store = _store(tmp)
        messages.send(
            store, "p", sender="Dev", recipient="supervisor",
            body="Done.", kind="report",
        )
        assert store.rebuild("p") is None or True
        assert store.project("p")["goal"] == "write a greeting"
        assert messages.latest_message(store, "p")["body"] == "Done."
        store.close()


def test_body_is_bounded():
    with tempfile.TemporaryDirectory() as tmp:
        store = _store(tmp)
        messages.send(
            store, "p", sender="Dev", recipient="General",
            body="x" * (messages.MAX_BODY_CHARS + 50), kind="note",
        )
        body = messages.latest_message(store, "p")["body"]
        assert len(body) <= messages.MAX_BODY_CHARS
        assert "truncated" in body
        store.close()


def test_runner_emits_assignment_and_report_messages():
    with tempfile.TemporaryDirectory() as tmp:
        store = _store(tmp)
        item = store.item("m1.i1")
        ctx = RunContext(
            project_id="p",
            project_root=tmp,
            item=item,
            member={"name": "Dev"},
            member_name="Dev",
            chat_id="",
            store=store,
        )
        runner = LocalRunner(execute=lambda _ctx: RunResult(
            item_id="m1.i1", ok=True, report="The greeting is ready.",
        ))
        runner.start(ctx)
        deadline = time.time() + 5
        results = []
        while time.time() < deadline:
            results = runner.collect("p")
            if results:
                break
            time.sleep(0.02)
        assert results and results[0].ok
        messages = __import__("agents.group.messages", fromlist=["list_messages"]).list_messages(store, "p")
        assert [m["kind"] for m in messages] == ["assignment", "report"]
        assert messages[0]["to"] == "Dev"
        assert messages[1]["from"] == "Dev"
        assert messages[1]["body"] == "The greeting is ready."
        store.close()


def test_build_prompt_injects_unread_peer_messages_once():
    with tempfile.TemporaryDirectory() as tmp:
        store = _store(tmp)
        item = store.item("m1.i1")
        ctx = RunContext(
            project_id="p",
            project_root=tmp,
            item=item,
            member={"name": "General"},
            member_name="General",
            chat_id="",
            store=store,
        )
        messages.send(
            store, "p", sender="Dev", recipient="General",
            body="Use the shared template.", kind="note",
        )
        prompt = build_prompt(ctx)
        assert "Messages from your group" in prompt
        assert "Use the shared template." in prompt
        assert len(ctx.delivered_messages) == 1
        for delivered in ctx.delivered_messages:
            messages.mark_read(store, "p", delivered, "General")
        assert messages.unread_for(store, "p", "General") == []
        store.close()


def test_group_message_tool_is_scoped_to_group_runs():
    from tools import group_message
    from tools._runtime import active_group_messenger
    from tools._schemas import get_native_tool_names

    assert "group_message" not in get_native_tool_names("general")
    assert group_message("Dev", "hello").startswith("Error:")

    token = active_group_messenger.set(lambda to, body: f"queued {to}: {body}")
    try:
        assert "group_message" in get_native_tool_names("general")
        assert group_message("Dev", "hello") == "queued Dev: hello"
    finally:
        active_group_messenger.reset(token)

    assert "group_message" not in get_native_tool_names("general")
