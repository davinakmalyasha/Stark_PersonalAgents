"""Regression tests for chat continuity.

Covers the three failure modes behind "only the latest reply survives":
  * token accounting used to rewrite the whole message table
  * a partial (trace-less) chat view could be saved and delete traces
  * concurrent appends raced on a read-modify-write
"""
import threading

import pytest

from server.store import chat_store, run_tracker
from server.context.chat_history import (
    build_chat_context,
    build_chat_history_messages,
)
from tools import checkpoints


@pytest.fixture()
def store(tmp_path, monkeypatch):
    old_db = chat_store._db
    old_chats_db = chat_store.CHATS_DB
    old_sessions = chat_store.SESSIONS_ROOT
    try:
        chat_store._db = None
        chat_store.CHATS_DB = str(tmp_path / "chats.db")
        chat_store.SESSIONS_ROOT = str(tmp_path / "sessions")
        chat_store._CHAT_LOCKS.clear()
        checkpoints._INDEX.clear()
        checkpoints._ACTIVE_RUN.clear()
        checkpoints._ACTIVE_RUN_MESSAGE.clear()
        checkpoints._ACTIVE_MANIFEST.clear()
        yield tmp_path
    finally:
        try:
            if chat_store._db is not None:
                chat_store._db.close()
        except Exception:
            pass
        chat_store._db = old_db
        chat_store.CHATS_DB = old_chats_db
        chat_store.SESSIONS_ROOT = old_sessions
        chat_store._CHAT_LOCKS.clear()


def _seed(store):
    workspace = str(store / "workspace")
    chat_id = chat_store.create_chat(workspace, "Continuity")["id"]
    chat_store.add_message(workspace, chat_id, "user", "hello")
    chat_store.add_message(workspace, chat_id, "agent", "hi there", {"isTrace": True})
    chat_store.add_message(workspace, chat_id, "user", "who are you")
    chat_store.add_message(workspace, chat_id, "agent", "I am Stark", {"isTrace": True})
    return workspace, chat_id


def test_token_usage_update_never_rewrites_messages(store):
    workspace, chat_id = _seed(store)
    before = chat_store.get_chat(workspace, chat_id)["messages"]
    assert [m["content"] for m in before] == [
        "hello", "hi there", "who are you", "I am Stark",
    ]

    chat_store.update_chat_token_usage(
        workspace, chat_id,
        {"total_input_tokens": 10, "total_output_tokens": 5, "calls": []},
    )

    after = chat_store.get_chat(workspace, chat_id)
    assert [m["content"] for m in after["messages"]] == [
        "hello", "hi there", "who are you", "I am Stark",
    ]
    assert after["token_usage"]["total_input_tokens"] == 10


def test_partial_chat_save_preserves_trace_messages(store):
    workspace, chat_id = _seed(store)
    partial = chat_store.get_chat(workspace, chat_id, include_traces=False)
    assert [m["content"] for m in partial["messages"]] == ["hello", "who are you"]

    partial["title"] = "Renamed"
    chat_store.save_chat(workspace, chat_id, partial)

    full = chat_store.get_chat(workspace, chat_id)
    assert full["title"] == "Renamed"
    assert [m["content"] for m in full["messages"]] == [
        "hello", "hi there", "who are you", "I am Stark",
    ]


def test_concurrent_appends_do_not_lose_messages(store):
    workspace, chat_id = _seed(store)

    def append(i: int) -> None:
        chat_store.add_message(workspace, chat_id, "user", f"parallel {i}")

    threads = [threading.Thread(target=append, args=(i,)) for i in range(8)]
    for t in threads:
        t.start()
    for t in threads:
        t.join()

    contents = [m["content"] for m in chat_store.get_chat(workspace, chat_id)["messages"]]
    assert len([c for c in contents if c.startswith("parallel ")]) == 8


def test_context_includes_previous_assistant_turns(store):
    workspace, chat_id = _seed(store)
    current = chat_store.get_chat(workspace, chat_id)["messages"][-1]["id"]

    context = build_chat_context(workspace, chat_id, exclude_message_id=current)
    assert "[ASSISTANT]: hi there" in context
    assert "I am Stark" not in context

    turns = build_chat_history_messages(workspace, chat_id, exclude_message_id=current)
    assert [type(t).__name__ for t in turns] == ["ModelRequest", "ModelResponse", "ModelRequest"]
    assert turns[1].parts[0].content == "hi there"


def test_one_run_per_chat_is_atomic():
    run_tracker.clear_active_run()
    assert run_tracker.begin_run("ws", "chat-lock") is True
    assert run_tracker.begin_run("ws", "chat-lock") is False
    run_tracker.clear_active_run(chat_id="chat-lock", workspace_path="ws")
    assert run_tracker.begin_run("ws", "chat-lock") is True
    run_tracker.clear_active_run()


def test_rewind_restores_files_and_truncates(store, tmp_path):
    workspace = tmp_path / "workspace"
    workspace.mkdir()
    (workspace / "a.txt").write_text("before", encoding="utf-8")
    chat_id = chat_store.create_chat(str(workspace), "Rewind")["id"]

    chat_store.add_message(str(workspace), chat_id, "user", "first")
    chat_store.add_message(str(workspace), chat_id, "agent", "first reply", {"isTrace": True})
    second = chat_store.add_message(str(workspace), chat_id, "user", "second")

    checkpoints.begin_run(chat_id, str(workspace), user_message_id=second["id"])
    checkpoints.pre_write_snapshot(chat_id, str(workspace), "a.txt")
    (workspace / "b.txt").write_text("created", encoding="utf-8")
    checkpoints.record_created(chat_id, str(workspace), "b.txt")
    (workspace / "a.txt").write_text("after", encoding="utf-8")
    checkpoints.finalize_run(chat_id, str(workspace), {"files": [
        {"path": "a.txt", "status": "modified"},
        {"path": "b.txt", "status": "added"},
    ]})
    chat_store.add_message(str(workspace), chat_id, "agent", "second reply", {"isTrace": True})

    result = checkpoints.restore_checkpoint(chat_id, str(workspace), [second["id"]])
    assert result["ok"] is True
    assert (workspace / "a.txt").read_text(encoding="utf-8") == "before"
    assert not (workspace / "b.txt").exists()

    truncated = chat_store.truncate_from_message(str(workspace), chat_id, second["id"])
    assert truncated["prompt"] == "second"
    assert [(m["role"], m["content"]) for m in chat_store.get_chat(str(workspace), chat_id)["messages"]] == [
        ("user", "first"), ("agent", "first reply"),
    ]
