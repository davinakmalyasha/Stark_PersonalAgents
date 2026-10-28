"""HTTP tests for the task popover's schedule API."""
from __future__ import annotations

import os
import sys

from fastapi import FastAPI
from fastapi.testclient import TestClient

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
if ROOT not in sys.path:
    sys.path.insert(0, ROOT)


def _client() -> TestClient:
    from server.routers import schedules

    app = FastAPI()
    app.include_router(schedules.router)
    return TestClient(app)


def test_schedule_create_list_and_cancel(monkeypatch):
    from server.routers import schedules

    created = {}
    record = {
        "id": "sched-test",
        "name": "Daily check",
        "prompt": "Review the inbox",
        "chatId": "chat-test",
        "nextRunAt": "2030-01-01T09:00:00+07:00",
        "recurrence": "daily",
        "status": "scheduled",
    }
    monkeypatch.setattr(schedules, "find_chat_workspace_id", lambda chat_id: "_general" if chat_id == "chat-test" else None)
    monkeypatch.setattr(schedules, "get_workspace", lambda workspace_id: {"id": "_general", "path": "C:/General"})
    monkeypatch.setattr(schedules, "get_chat", lambda path, chat_id: {"id": chat_id} if (path, chat_id) == ("C:/General", "chat-test") else None)
    monkeypatch.setattr(schedules.schedule_store, "create_schedule", lambda **kwargs: (created.update(kwargs) or record))
    monkeypatch.setattr(schedules.schedule_store, "list_schedules", lambda: [record])
    monkeypatch.setattr(schedules.schedule_store, "cancel_schedule", lambda schedule_id: schedule_id == "sched-test")

    client = _client()
    response = client.post("/api/schedules", json={
        "workspace_id": "_general",
        "chat_id": "chat-test",
        "name": "Daily check",
        "prompt": "Review the inbox",
        "at": "2030-01-01T09:00:00+07:00",
        "recurrence": "daily",
    })
    assert response.status_code == 201
    assert created["chat_id"] == "chat-test"
    assert created["recurrence"] == "daily"
    assert response.json()["workspaceId"] == "_general"
    assert response.json()["chatId"] == "chat-test"
    assert response.json()["nextRunAt"] == record["nextRunAt"]

    listed = client.get("/api/schedules?workspace_id=_general")
    assert listed.status_code == 200
    assert listed.json()["schedules"][0]["status"] == "scheduled"

    cancelled = client.delete("/api/schedules/sched-test")
    assert cancelled.status_code == 200
    assert cancelled.json()["status"] == "cancelled"


def test_schedule_rejects_invalid_recurrence_blank_prompt_and_missing_chat(monkeypatch):
    from server.routers import schedules

    monkeypatch.setattr(schedules, "find_chat_workspace_id", lambda chat_id: "_general" if chat_id == "chat-test" else None)
    monkeypatch.setattr(schedules, "get_workspace", lambda workspace_id: {"id": "_general", "path": "C:/General"})
    monkeypatch.setattr(schedules, "get_chat", lambda _path, chat_id: {"id": chat_id})
    client = _client()
    base = {"chat_id": "chat-test", "prompt": "Do work", "at": "2030-01-01T09:00:00+07:00"}
    assert client.post("/api/schedules", json={**base, "recurrence": "hourly"}).status_code == 400
    assert client.post("/api/schedules", json={**base, "prompt": "   "}).status_code == 400
    assert client.post("/api/schedules", json={**base, "chat_id": "missing"}).status_code == 404
