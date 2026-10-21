"""Canvas mode: watch agent chats and wake the orchestrator on completion."""
from __future__ import annotations


from server.store import agent_wakes
from server.store.chat_store import get_chat, get_chat_workspace_path, list_chats
from server.store.run_tracker import snapshot as run_snapshot

def _norm_workspace(path: str) -> str:
    return "" if path == "_general" else (path or "")


def _agent_chats() -> list[dict]:
    from server.store.workspace_registry import list_workspaces

    found: list[dict] = []
    seen: set[str] = set()
    for workspace in list_workspaces():
        path = workspace.get("path") or ""
        for chat in list_chats(path):
            chat_id = str(chat.get("id") or "")
            if not chat_id or chat_id in seen:
                continue
            if not (chat.get("parentChatId") or chat_id.startswith("gp-")):
                continue
            seen.add(chat_id)
            found.append({"id": chat_id, "workspacePath": _norm_workspace(path)})
    return found


def _chat_snapshot(chat_id: str, workspace_path: str = "") -> dict | None:
    path = _norm_workspace(workspace_path or get_chat_workspace_path(chat_id))
    chat = get_chat(path, chat_id, include_traces=False, include_usage=False)
    if not chat:
        return None
    run = run_snapshot(chat_id=chat_id)
    messages = chat.get("messages") or []
    last = messages[-1] if messages else {}
    return {
        "kind": "chat",
        "id": chat_id,
        "workspacePath": path,
        "title": chat.get("title") or chat_id,
        "parentChatId": chat.get("parentChatId") or "",
        "running": bool(run.get("running")),
        "lastRole": last.get("role") or "",
        "lastMessage": str(last.get("content") or "")[:1200],
        "updatedAt": chat.get("updatedAt") or "",
    }


def _group_snapshot(group_id: str) -> dict | None:
    try:
        from server.store import group_registry
        store = group_registry.open_store(group_id)
        project = store.project(group_id) or {}
        items = store.items(group_id) or []
        milestones = store.milestones(group_id) or []
    except Exception:
        return None
    return {
        "kind": "group",
        "id": group_id,
        "title": project.get("goal") or group_id,
        "status": project.get("status") or "unknown",
        "itemsDone": sum(1 for item in items if item.get("status") == "done"),
        "itemsTotal": len(items),
        "milestonesDone": sum(1 for item in milestones if item.get("status") == "done"),
        "milestonesTotal": len(milestones),
        "awaitingApproval": sum(
            1 for item in items if item.get("blocked_reason") == "awaiting_approval"
        ),
        "spentUsd": project.get("spent_usd") or 0,
        "budgetUsd": project.get("budget_usd") or 0,
    }


def snapshot(orchestrator_chat_id: str) -> dict:
    watch = agent_wakes.active_watch(orchestrator_chat_id)
    if not watch:
        return {"watch": None, "chats": [], "groups": [], "events": []}
    chats = []
    groups = []
    if watch.get("mode") == "all":
        for row in _agent_chats():
            item = _chat_snapshot(row["id"], row.get("workspacePath", ""))
            if item:
                chats.append(item)
    else:
        for target in watch.get("targets") or []:
            target = str(target)
            if target.startswith("group:"):
                group = _group_snapshot(target.split(":", 1)[1])
                if group:
                    groups.append(group)
            else:
                item = _chat_snapshot(target)
                if item:
                    chats.append(item)
    return {
        "watch": watch,
        "chats": chats,
        "groups": groups,
        "events": agent_wakes.list_events(watch["id"], limit=20),
    }


def enter(orchestrator_chat_id: str, workspace_path: str, mode: str = "all",
          targets: list[str] | None = None, wake_on: list[str] | None = None,
          max_actions: int = 20, until: str | None = None) -> dict:
    mode = (mode or "all").strip().lower()
    if mode not in ("single", "multiple", "all"):
        mode = "all"
    clean_targets = [str(t).strip() for t in (targets or []) if str(t).strip()]
    if mode != "all" and not clean_targets:
        raise ValueError("select at least one chat or group when mode is single/multiple")
    for target in clean_targets:
        if target.startswith("group:"):
            try:
                from server.store import group_registry
                if not group_registry.exists(target.split(":", 1)[1]):
                    raise ValueError(f"unknown group {target}")
            except ValueError:
                raise
            except Exception as exc:
                raise ValueError(f"unknown group {target}: {exc}") from exc
        elif not get_chat("", target, include_traces=False, include_usage=False):
            raise ValueError(f"unknown chat {target}")
    watch = agent_wakes.enter_watch(
        orchestrator_chat_id, workspace_path, mode, clean_targets,
        wake_on=wake_on or ["completed", "failed"], max_actions=max_actions,
        until=until,
    )
    return snapshot(orchestrator_chat_id)


def exit(chat_id: str) -> int:
    return agent_wakes.close_watch_for_chat(chat_id)


def read(chat_id: str) -> dict:
    return snapshot(chat_id)


def message(chat_id: str, target_chat_id: str, text: str) -> dict:
    watch = agent_wakes.active_watch(chat_id)
    if not watch:
        raise ValueError("canvas mode is not active")
    if int(watch.get("actionsUsed") or 0) >= int(watch.get("maxActions") or 0):
        raise ValueError("canvas action budget reached; exit or raise max_actions")
    if not agent_wakes.matches(watch, target_chat_id):
        raise ValueError("target is not selected in this canvas watch")
    if str(target_chat_id).startswith("group:"):
        raise ValueError("use group_update for group projects")
    workspace_path = _norm_workspace(get_chat_workspace_path(target_chat_id))
    wake = agent_wakes.enqueue_wake(
        workspace_path,
        target_chat_id,
        text,
        {"fromCanvas": True, "watchId": watch["id"], "orchestratorChatId": chat_id},
        mode="auto",
    )
    used = agent_wakes.bump_watch(watch["id"])
    if used >= int(watch.get("maxActions") or 0):
        agent_wakes.close_watch(watch["id"])
    return {"wake": wake, "actionsUsed": used, "maxActions": watch.get("maxActions")}


def notify_run_finished(source_chat_id: str, report: str, status: str = "completed") -> int:
    if not source_chat_id or not report:
        return 0
    watches = agent_wakes.active_watches()
    if not watches:
        return 0
    source = get_chat("", source_chat_id, include_traces=False, include_usage=False)
    if not source:
        if source_chat_id.startswith("gp-"):
            source = {"title": source_chat_id, "parentChatId": ""}
        else:
            return 0
    parent = source.get("parentChatId") or ""
    sent = 0
    for watch in watches:
        if not agent_wakes.matches(watch, source_chat_id):
            continue
        if status not in (watch.get("wakeOn") or []):
            continue
        if watch.get("mode") == "all" and parent and not source_chat_id.startswith("gp-"):
            continue
        used = int(watch.get("actionsUsed") or 0)
        if used >= int(watch.get("maxActions") or 0):
            agent_wakes.close_watch(watch["id"])
            continue
        title = source.get("title") or source_chat_id
        event_text = (
            f"[CANVAS EVENT] {title} ({source_chat_id}) {status}.\n"
            f"{str(report)[:4000]}\n\n"
            "Review this result and decide whether to message the chat, send follow-up work, "
            "or take no action."
        )
        try:
            agent_wakes.enqueue_wake(
                _norm_workspace(get_chat_workspace_path(watch["orchestratorChatId"])),
                watch["orchestratorChatId"],
                event_text,
                {
                    "canvasEvent": True,
                    "watchId": watch["id"],
                    "sourceChatId": source_chat_id,
                    "status": status,
                },
                mode="orchestrator",
            )
            agent_wakes.record_event(watch["id"], source_chat_id, status, {
                "title": title,
                "report": str(report)[:4000],
            })
            used = agent_wakes.bump_watch(watch["id"])
            if used >= int(watch.get("maxActions") or 0):
                agent_wakes.close_watch(watch["id"])
            sent += 1
        except Exception as exc:
            print(f"[CANVAS] wake failed: {exc}", flush=True)
    return sent


def available() -> dict:
    chats = []
    for row in _agent_chats():
        item = _chat_snapshot(row["id"], row.get("workspacePath", ""))
        if item:
            chats.append(item)
    groups = []
    try:
        from server.store import group_registry
        for row in group_registry.list_projects():
            groups.append({
                "kind": "group",
                "id": row.get("id"),
                "title": row.get("title") or row.get("id"),
                "status": row.get("status") or "unknown",
            })
    except Exception:
        pass
    return {"chats": chats, "groups": groups}
