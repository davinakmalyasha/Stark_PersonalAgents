"""
Run pipeline router — Phase 7: /api/run SSE + /api/stream/{chat_id} mirror.
Rebuilt on asyncio.Queue (no 0.05s polling) + server.realtime.sse + run_tracker.
"""
import asyncio
import json
import threading
import time
import queue
from typing import AsyncGenerator

from fastapi import APIRouter, Request
from fastapi.responses import StreamingResponse, JSONResponse
from pydantic import BaseModel

from server.realtime.sse import format_sse as _format_sse, mirror_publish as _mirror_publish
from server.store.run_tracker import set_active_run, clear_active_run

router = APIRouter(tags=["runs"])

_run_contexts: dict[str, dict] = {}
_run_contexts_lock = threading.Lock()


def _collect_run_artifacts(project_path: str, context: dict) -> dict:
    """Build the file-change summary and review data for one agent run.

    Phase 7: extracted from api_server (git-diff delta + task tracker merge).
    """
    import os
    from server.api_server import (
        _git_status_snapshot, _normalize_artifact_path, _read_file_bytes,
        _git_head_bytes, _text_diff, _file_hash,
    )
    from sync.state_sync import safe_get_state

    baseline = context.get("baseline", {})
    current = _git_status_snapshot(project_path)
    candidate_paths: set[str] = set()

    for path, status in current.items():
        normalized = _normalize_artifact_path(project_path, path)
        if normalized is not None and (path not in baseline or baseline[path] != status):
            candidate_paths.add(normalized)

    try:
        from sync.sync_helpers import load_task_tracking
        task_data = load_task_tracking(project_path, safe_get_state().get("chat_id", "")) or {}
        artifacts = task_data.get("artifacts", {})
        for group in ("files_created", "files_modified", "files_deleted"):
            for path in artifacts.get(group, []) or []:
                normalized = _normalize_artifact_path(project_path, path)
                if normalized is not None:
                    candidate_paths.add(normalized)
    except Exception:
        pass

    files: list[dict] = []
    for path in sorted(candidate_paths):
        baseline_content = context.get("dirty_files", {}).get(path, _git_head_bytes(project_path, path))
        current_content = _read_file_bytes(project_path, path)
        if baseline_content == current_content:
            continue
        diff, additions, deletions = _text_diff(baseline_content, current_content, path)
        status = "added" if baseline_content is None and current_content is not None else "deleted" if current_content is None else "modified"
        files.append({
            "path": path,
            "status": status,
            "additions": additions,
            "deletions": deletions,
            "diff": diff,
            "after_hash": _file_hash(current_content),
        })

    tests_passing = False
    try:
        from sync.sync_helpers import load_task_tracking
        task_data = load_task_tracking(project_path, safe_get_state().get("chat_id", "")) or {}
        tests_passing = bool((task_data.get("artifacts", {}) or {}).get("tests_passing"))
    except Exception:
        pass

    created = [item["path"] for item in files if item["status"] == "added"]
    modified = [item["path"] for item in files if item["status"] == "modified"]
    deleted = [item["path"] for item in files if item["status"] == "deleted"]
    artifacts_result = {
        "files_created": created,
        "files_modified": modified,
        "files_deleted": deleted,
        "files": files,
        "total_additions": sum(item["additions"] for item in files),
        "total_deletions": sum(item["deletions"] for item in files),
        "tests_passing": tests_passing,
        "run_id": context["run_id"],
    }
    context["artifacts"] = artifacts_result
    with _run_contexts_lock:
        _run_contexts[context["run_id"]] = context
    return artifacts_result


def _restore_run_file(context: dict, item: dict) -> tuple[bool, str]:
    import os
    from server.api_server import _normalize_artifact_path, _read_file_bytes, _file_hash, _git_head_bytes
    project_path = context["workspace_path"]
    path = _normalize_artifact_path(project_path, item.get("path", ""))
    if path is None:
        return False, "Path is outside the workspace"
    current = _read_file_bytes(project_path, path)
    if _file_hash(current) != item.get("after_hash"):
        return False, "File changed after this run"
    if path in context.get("dirty_files", {}):
        baseline = context["dirty_files"][path]
    else:
        baseline = _git_head_bytes(project_path, path)
    absolute = os.path.join(project_path, path)
    if baseline is None:
        try:
            if os.path.exists(absolute):
                os.remove(absolute)
            return True, "Removed"
        except OSError as exc:
            return False, str(exc)
    try:
        os.makedirs(os.path.dirname(absolute), exist_ok=True)
        with open(absolute, "wb") as handle:
            handle.write(baseline)
        return True, "Restored"
    except OSError as exc:
        return False, str(exc)


@router.post("/api/runs/{run_id}/undo")
async def undo_run(run_id: str):
    with _run_contexts_lock:
        context = _run_contexts.get(run_id)
    if not context or not context.get("artifacts"):
        return JSONResponse(status_code=404, content={"error": "This run is no longer available for undo."})
    if context.get("undone"):
        return JSONResponse(content={"status": "undone", "results": []})
    results = []
    conflicts = []
    for item in context["artifacts"].get("files", []):
        ok, detail = _restore_run_file(context, item)
        results.append({"path": item["path"], "ok": ok, "detail": detail})
        if not ok:
            conflicts.append(item["path"])
    context["undone"] = not conflicts
    return JSONResponse(content={"status": "conflict" if conflicts else "undone", "results": results, "conflicts": conflicts})


class RunRequest(BaseModel):
    prompt: str
    model: str = "Automatic Fallback"
    temperature: float = 0.7
    workspace_path: str = ""
    chat_id: str = ""
    attachments: list[dict] = []
    workspace_kind: str = ""  # hint: "" | "house" | "personal" | "dfd" | "design"


@router.post("/api/run")
async def run_pipeline(req: RunRequest, request: Request):
    """Run the agentic pipeline with SSE streaming of state updates."""
    import workspace_manager as wm
    from memory.chat_memory_manager import resolve_request_text
    from sync.state_sync import reset_shared_state, safe_update_state, safe_serialize_state, safe_get_state
    from server.api_server import _capture_run_context

    reset_shared_state()

    safe_update_state({
        "selected_model": req.model,
        "selected_temp": req.temperature,
        "is_general": not req.workspace_path,
        "failed_models": [],
        "token_usage": {
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_hit_tokens": 0,
            "total_cache_miss_tokens": 0,
            "total_cost": 0.0,
            "calls": [],
        }
    })

    attachment_note = ""
    if req.attachments:
        attachment_note = "\n\nAttached files for this request:\n" + "\n".join(
            f"- {item.get('name', 'attachment')} ({item.get('path', item.get('url', 'uploaded file'))})"
            for item in req.attachments
        )
        try:
            _img_paths = [item.get("path") for item in req.attachments
                          if item.get("isImage") and item.get("path")]
            if _img_paths:
                safe_update_state({"attachment_image_paths": [str(p) for p in _img_paths]})
        except Exception:
            pass
    prompt_with_attachments = req.prompt + attachment_note
    wm.add_message(req.workspace_path, req.chat_id, "user", req.prompt, {"attachments": req.attachments} if req.attachments else None)
    resolved = resolve_request_text(prompt_with_attachments, workspace_path=req.workspace_path, chat_id=req.chat_id)
    run_context = _capture_run_context(req.workspace_path, req.chat_id)

    try:
        _workspace_ids_before_run = {w.get("id") for w in wm.list_workspaces()}
    except Exception:
        _workspace_ids_before_run = set()

    async def build_stream() -> AsyncGenerator[str, None]:
        done = False
        had_error = False
        cancel_event = threading.Event()
        aq: asyncio.Queue = asyncio.Queue()

        def run_in_thread():
            nonlocal done, had_error
            try:
                from routing.router import decide_route, stream_for_kind
                decision = decide_route(
                    resolved,
                    req.workspace_path or "",
                    req.chat_id or "",
                    getattr(req, "workspace_kind", "") or "",
                )
                kind = decision.get("kind", "orchestrator")
                project = decision.get("project")
                reason = decision.get("reason", "")
                print(f"[ROUTER] kind={kind} reason={reason} workspace_path={req.workspace_path!r} kind_hint={getattr(req, 'workspace_kind', '')!r}")
                for state_snapshot in stream_for_kind(
                    kind, project, resolved, req.workspace_path or "", req.chat_id or "", req.attachments or [], cancel_event=cancel_event
                ):
                    aq.put_nowait(state_snapshot)
                aq.put_nowait(None)
                return
            except Exception as e:
                import traceback as _tb
                print(f"[ROUTER] decide/stream error (falling back): {e}\n{_tb.format_exc()}")

            try:
                is_general = not req.workspace_path
                if is_general:
                    from agents.general_agent import run_general_direct
                    image_paths = [item.get("path") for item in req.attachments
                                   if item.get("isImage") and item.get("path")]
                    for state_snapshot in run_general_direct(resolved, chat_id=req.chat_id, images=image_paths or None):
                        aq.put_nowait(state_snapshot)
                else:
                    from verify_helper import run_graph_with_verify
                    for state_snapshot in run_graph_with_verify(resolved, workspace_path=req.workspace_path, chat_id=req.chat_id, cancel_event=cancel_event):
                        aq.put_nowait(state_snapshot)
                aq.put_nowait(None)
            except Exception as e:
                import traceback
                had_error = True
                try:
                    err_state = json.loads(safe_serialize_state())
                except Exception:
                    err_state = {}
                err_state["error"] = str(e)
                err_state["traceback"] = traceback.format_exc()
                aq.put_nowait(err_state)
                aq.put_nowait(None)
            finally:
                done = True

        thread = threading.Thread(target=run_in_thread, daemon=True)
        thread.start()

        last_ver = 0
        last_bus_drain = 0.0
        # Event-driven bus wake: avoid 50ms poll when deltas arrive.
        try:
            from server.realtime.bus import (
                drain as _bus_drain,
                cleanup as _bus_cleanup,
                get_bus_event as _get_bus_event,
            )
            _bus_event = _get_bus_event(req.chat_id)
        except Exception:
            _bus_event = None

        while True:
            if await request.is_disconnected():
                cancel_event.set()
                break

            # StreamBus token deltas — drain immediately if any, else wait event-driven.
            try:
                deltas = _bus_drain(req.chat_id)
                if deltas:
                    for d in deltas:
                        _mirror_publish(req.chat_id, "delta", d)
                        yield _format_sse("delta", d)
                    # keep event cleared after drain so next emit wakes us
                    if _bus_event is not None:
                        try:
                            _bus_event.clear()
                        except Exception:
                            pass
                now = time.time()
                if now - last_bus_drain > 60:
                    last_bus_drain = now
                    _bus_cleanup()
            except Exception:
                pass

            # Wait for either state snapshot (aq) or bus event, event-driven (20ms) instead of 50+50 polling.
            try:
                # Prefer bus event wake when idle: wait on both with shorter timeout.
                if _bus_event is not None and not deltas:
                    # Brief event wait + aq poll combined — ~20ms idle cadence vs 100ms before.
                    try:
                        await asyncio.wait_for(_bus_event.wait(), timeout=0.02)
                        if _bus_event is not None:
                            try:
                                _bus_event.clear()
                            except Exception:
                                pass
                        # drain again after wake
                        try:
                            deltas2 = _bus_drain(req.chat_id)
                            if deltas2:
                                for d in deltas2:
                                    _mirror_publish(req.chat_id, "delta", d)
                                    yield _format_sse("delta", d)
                        except Exception:
                            pass
                    except asyncio.TimeoutError:
                        pass
                item = await asyncio.wait_for(aq.get(), timeout=0.02)
                if item is None:
                    break
                if isinstance(item, dict):
                    _mirror_publish(req.chat_id, "state", item)
                    yield _format_sse("state", item)
                    last_ver = item.get("state_version", 0)
            except asyncio.TimeoutError:
                if not done:
                    snap = json.loads(safe_serialize_state())
                    cur_ver = snap.get("state_version", 0)
                    if cur_ver > last_ver:
                        last_ver = cur_ver
                        snap["_intermediate"] = True
                        _mirror_publish(req.chat_id, "state", snap)
                        yield _format_sse("state", snap)

        _save_chat_token_usage(req.workspace_path, req.chat_id)

        final_state = json.loads(safe_serialize_state())
        final_state["artifacts"] = _collect_run_artifacts(req.workspace_path, run_context)

        try:
            new_ids = {w.get("id") for w in wm.list_workspaces()} - _workspace_ids_before_run
            if new_ids:
                final_state["workspace_created"] = True
                final_state["created_workspace_ids"] = sorted(new_ids)
        except Exception:
            pass

        _mirror_publish(req.chat_id, "state", final_state)
        yield _format_sse("state", final_state)
        _mirror_publish(req.chat_id, "done", {"status": "error" if had_error else "complete"})
        yield _format_sse("done", {"status": "error" if had_error else "complete"})

    async def tracked_stream() -> AsyncGenerator[str, None]:
        try:
            async for chunk in build_stream():
                yield chunk
        finally:
            clear_active_run(chat_id=req.chat_id, workspace_path=req.workspace_path)

    set_active_run(req.workspace_path, req.chat_id)
    try:
        from tools.checkpoints import begin_run
        begin_run(req.chat_id, req.workspace_path)
    except Exception:
        pass
    return StreamingResponse(tracked_stream(), media_type="text/event-stream")


def _save_chat_token_usage(workspace_path: str, chat_id: str):
    import workspace_manager as wm
    from sync.state_sync import safe_get_state
    if not chat_id or not workspace_path:
        return
    try:
        chat = wm.get_chat(workspace_path, chat_id, include_traces=False, include_usage=True)
        if chat:
            run_stats = safe_get_state().get("token_usage", {})
            chat_stats = chat.get("token_usage") or {
                "total_input_tokens": 0,
                "total_output_tokens": 0,
                "total_cache_hit_tokens": 0,
                "total_cache_miss_tokens": 0,
                "total_cost": 0.0,
                "calls": []
            }
            if "total_cache_hit_tokens" not in chat_stats:
                chat_stats["total_cache_hit_tokens"] = 0
            if "total_cache_miss_tokens" not in chat_stats:
                chat_stats["total_cache_miss_tokens"] = 0
            chat_stats["total_input_tokens"] += run_stats.get("total_input_tokens", 0)
            chat_stats["total_output_tokens"] += run_stats.get("total_output_tokens", 0)
            chat_stats["total_cache_hit_tokens"] += run_stats.get("total_cache_hit_tokens", 0)
            chat_stats["total_cache_miss_tokens"] += run_stats.get("total_cache_miss_tokens", 0)
            chat_stats["total_cost"] += run_stats.get("total_cost", 0.0)
            chat_stats["calls"] = chat_stats.get("calls", []) + run_stats.get("calls", [])
            chat["token_usage"] = chat_stats
            wm.save_chat(workspace_path, chat_id, chat)
    except Exception as e:
        print(f"Error saving token stats for chat {chat_id}: {e}")


@router.get("/api/runs/active")
async def get_active_run(chat_id: str = "", workspace_path: str = ""):
    from server.store.run_tracker import snapshot as _run_snapshot
    return JSONResponse(content=_run_snapshot(chat_id, workspace_path))


@router.get("/api/stream/{chat_id}")
async def stream_chat_events(chat_id: str, request: Request):
    """Subscribe to another client's run frames for one chat (live mirroring)."""
    from server.realtime.bus import subscribe as bus_subscribe, unsubscribe as bus_unsubscribe
    from server.store.run_tracker import snapshot as _run_snapshot
    from sync.state_sync import safe_serialize_state
    q = bus_subscribe(chat_id)

    async def mirror_stream() -> AsyncGenerator[str, None]:
        try:
            yield _format_sse("hello", {"chat_id": chat_id})
            run_info = _run_snapshot(chat_id=chat_id)
            if run_info.get("running"):
                try:
                    snap = json.loads(safe_serialize_state())
                    yield _format_sse("state", snap)
                except Exception:
                    pass
            while True:
                if await request.is_disconnected():
                    break
                try:
                    event, data = await asyncio.to_thread(q.get, True, 0.5)
                    yield _format_sse(event, data)
                    if event == "done":
                        break
                except queue.Empty:
                    yield ": keepalive\n\n"
        finally:
            bus_unsubscribe(chat_id, q)

    return StreamingResponse(mirror_stream(), media_type="text/event-stream")
