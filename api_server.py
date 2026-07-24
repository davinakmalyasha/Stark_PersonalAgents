"""
FastAPI server — Bridges the React frontend with the LangGraph multi-agent pipeline.
Replaces the Gradio dashboard with SSE streaming endpoints.
"""
import os
import uuid
import asyncio
import json
import threading
import time
import queue
from typing import AsyncGenerator
from datetime import datetime, timezone, timedelta

from fastapi import FastAPI, Request, UploadFile, File
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse, JSONResponse
from pydantic import BaseModel
from dotenv import load_dotenv

from sync_manager import run_and_sync_graph
from state_sync import shared_state, safe_get_state, safe_update_state, safe_serialize_state, state_lock, reset_shared_state
import workspace_manager as wm

load_dotenv(override=True)

# Init default workspace on startup
wm.init_default_workspace()
# Clean up orphaned chat directories from previously deleted workspaces
_orphans_removed = wm.cleanup_orphaned_chat_dirs()
if _orphans_removed:
    print(f"[STARTUP] Cleaned up {_orphans_removed} orphaned workspace chat directories")

app = FastAPI(title="Deep Agents API", version="2.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)


# ── Models ──────────────────────────────────────────────────────────────────

class RunRequest(BaseModel):
    prompt: str
    model: str = "Automatic Fallback"
    temperature: float = 0.7
    workspace_path: str = ""
    chat_id: str = ""
    attachments: list[dict] = []

class ChatRequest(BaseModel):
    prompt: str


class TaskRequest(BaseModel):
    prompt: str = ""
    attachments: list[dict] = []
    mode: str = "saved"
    schedule: dict | None = None


class TaskUpdateRequest(BaseModel):
    status: str | None = None
    prompt: str | None = None
    attachments: list[dict] | None = None
    mode: str | None = None
    schedule: dict | None = None


_task_scheduler_started = False
_task_scheduler_lock = threading.Lock()


# ── SSE Helpers ─────────────────────────────────────────────────────────────

def _format_sse(event: str, data: dict) -> str:
    """Format a Server-Sent Event."""
    return f"event: {event}\ndata: {json.dumps(data)}\n\n"


# ── Routes ──────────────────────────────────────────────────────────────────

@app.get("/api/state")
async def get_state():
    """Return the current shared state as JSON."""
    return JSONResponse(content=json.loads(safe_serialize_state()))


def _workspace_path_for_id(workspace_id: str) -> str:
    if workspace_id == wm.GENERAL_WORKSPACE_ID:
        return wm.GENERAL_WORKSPACE_ID
    workspace = wm.get_workspace(workspace_id)
    if not workspace:
        raise ValueError("Workspace not found")
    return workspace.get("path", "")


def _task_now() -> datetime:
    return datetime.now(timezone.utc)


def _parse_task_time(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        return parsed.astimezone(timezone.utc) if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)
    except ValueError:
        return None


def _next_recurring_time(current: datetime, recurrence: str) -> datetime | None:
    if recurrence == "daily":
        return current + timedelta(days=1)
    if recurrence == "weekly":
        return current + timedelta(days=7)
    if recurrence == "monthly":
        month = current.month % 12 + 1
        year = current.year + (1 if current.month == 12 else 0)
        day = min(current.day, 28)
        return current.replace(year=year, month=month, day=day)
    return None


def _update_task(workspace_path: str, chat_id: str, task_id: str, **changes) -> dict | None:
    tasks = wm.get_chat_tasks(workspace_path, chat_id)
    for task in tasks:
        if task.get("id") == task_id:
            task.update(changes)
            task["updatedAt"] = _task_now().isoformat()
            wm.save_chat_tasks(workspace_path, chat_id, tasks)
            return task
    return None


def _run_task_background(workspace_id: str, workspace_path: str, chat_id: str, task: dict) -> None:
    task_id = task["id"]
    _update_task(workspace_path, chat_id, task_id, status="running", lastRunAt=_task_now().isoformat())
    try:
        prompt = task.get("prompt", "")
        attachments = task.get("attachments", [])
        wm.add_message(workspace_path, chat_id, "user", prompt, {"attachments": attachments, "taskId": task_id} if attachments else {"taskId": task_id})
        if workspace_path in ("", wm.GENERAL_WORKSPACE_ID):
            from free_agent import run_freeagent_direct
            for _ in run_freeagent_direct(prompt, chat_id=chat_id, images=[a.get("path") for a in attachments if a.get("isImage") and a.get("path")] or None):
                pass
        else:
            for _ in run_and_sync_graph(prompt, workspace_path=workspace_path, chat_id=chat_id):
                pass
        report = safe_get_state().get("outputs", {}).get("agent_report", "")
        if report and chat_id:
            wm.add_message(workspace_path, chat_id, "agent", report, {"taskId": task_id})
        recurrence = (task.get("schedule") or {}).get("recurrence", "none")
        current_next = _parse_task_time((task.get("schedule") or {}).get("nextRunAt")) or _task_now()
        next_run = _next_recurring_time(current_next, recurrence)
        if next_run:
            schedule = {**(task.get("schedule") or {}), "nextRunAt": next_run.isoformat()}
            _update_task(workspace_path, chat_id, task_id, status="scheduled", schedule=schedule)
        else:
            _update_task(workspace_path, chat_id, task_id, status="completed", completedAt=_task_now().isoformat())
    except Exception as exc:
        _update_task(workspace_path, chat_id, task_id, status="failed", error=str(exc))


def _start_task_scheduler() -> None:
    global _task_scheduler_started
    with _task_scheduler_lock:
        if _task_scheduler_started:
            return
        _task_scheduler_started = True

    def loop() -> None:
        while True:
            try:
                for workspace in wm.list_workspaces():
                    workspace_id = workspace.get("id", "")
                    workspace_path = _workspace_path_for_id(workspace_id)
                    for chat in wm.list_chats(workspace_path):
                        chat_id = chat.get("id", "")
                        for task in wm.get_chat_tasks(workspace_path, chat_id):
                            due = _parse_task_time((task.get("schedule") or {}).get("nextRunAt"))
                            if task.get("status") == "scheduled" and due and due <= _task_now():
                                threading.Thread(target=_run_task_background, args=(workspace_id, workspace_path, chat_id, task.copy()), daemon=True).start()
            except Exception as exc:
                print(f"[TASK SCHEDULER] {exc}")
            time.sleep(15)

    threading.Thread(target=loop, name="task-scheduler", daemon=True).start()


@app.get("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks")
async def list_tasks(workspace_id: str, chat_id: str):
    try:
        return JSONResponse(content=wm.get_chat_tasks(_workspace_path_for_id(workspace_id), chat_id))
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@app.post("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks")
async def create_task(workspace_id: str, chat_id: str, req: TaskRequest):
    try:
        workspace_path = _workspace_path_for_id(workspace_id)
        if not req.prompt.strip() and not req.attachments:
            return JSONResponse(status_code=400, content={"error": "A prompt or attachment is required"})
        now = _task_now()
        schedule = req.schedule or {}
        status = "scheduled" if req.mode == "scheduled" else "ready"
        task = {"id": f"task-{uuid.uuid4().hex[:12]}", "workspaceId": workspace_id, "chatId": chat_id, "prompt": req.prompt, "attachments": req.attachments, "mode": req.mode, "status": status, "schedule": schedule, "createdAt": now.isoformat(), "updatedAt": now.isoformat()}
        tasks = wm.get_chat_tasks(workspace_path, chat_id)
        tasks.insert(0, task)
        if not wm.save_chat_tasks(workspace_path, chat_id, tasks):
            return JSONResponse(status_code=404, content={"error": "Chat not found"})
        return JSONResponse(content=task)
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@app.post("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks/{task_id}/start")
async def start_task(workspace_id: str, chat_id: str, task_id: str):
    try:
        workspace_path = _workspace_path_for_id(workspace_id)
        task = next((item for item in wm.get_chat_tasks(workspace_path, chat_id) if item.get("id") == task_id), None)
        if not task:
            return JSONResponse(status_code=404, content={"error": "Task not found"})
        updated = _update_task(workspace_path, chat_id, task_id, status="running", lastRunAt=_task_now().isoformat()) or {**task, "status": "running"}
        return JSONResponse(content=updated)
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@app.patch("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks/{task_id}")
async def update_task(workspace_id: str, chat_id: str, task_id: str, req: TaskUpdateRequest):
    try:
        workspace_path = _workspace_path_for_id(workspace_id)
        if req.status is not None and req.status not in {"ready", "scheduled", "running", "completed", "failed"}:
            return JSONResponse(status_code=400, content={"error": "Invalid task status"})
        changes = req.model_dump(exclude_unset=True, exclude_none=True)
        if "prompt" in changes and not str(changes["prompt"]).strip() and not changes.get("attachments"):
            return JSONResponse(status_code=400, content={"error": "A prompt or attachment is required"})
        if changes.get("mode") not in (None, "saved", "scheduled"):
            return JSONResponse(status_code=400, content={"error": "Invalid task mode"})
        if req.status == "completed":
            changes["completedAt"] = _task_now().isoformat()
        updated = _update_task(workspace_path, chat_id, task_id, **changes)
        if not updated:
            return JSONResponse(status_code=404, content={"error": "Task not found"})
        return JSONResponse(content=updated)
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


@app.delete("/api/workspaces/{workspace_id}/chats/{chat_id}/tasks/{task_id}")
async def delete_task(workspace_id: str, chat_id: str, task_id: str):
    try:
        workspace_path = _workspace_path_for_id(workspace_id)
        tasks = wm.get_chat_tasks(workspace_path, chat_id)
        filtered = [task for task in tasks if task.get("id") != task_id]
        if len(filtered) == len(tasks):
            return JSONResponse(status_code=404, content={"error": "Task not found"})
        wm.save_chat_tasks(workspace_path, chat_id, filtered)
        return JSONResponse(content={"ok": True})
    except ValueError as exc:
        return JSONResponse(status_code=404, content={"error": str(exc)})


def _save_chat_token_usage(workspace_path: str, chat_id: str):
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


@app.post("/api/run")
async def run_pipeline(req: RunRequest, request: Request):
    """Run the agentic pipeline with SSE streaming of state updates.
    No Jarvis Router — goes directly to the main agent."""
    from workspace_manager import add_message as wm_add_message
    from chat_memory_manager import resolve_request_text

    # Reset shared state to clear stale data from previous run
    reset_shared_state()

    # Clear previous failed models and reset current run token usage
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
    prompt_with_attachments = req.prompt + attachment_note
    wm_add_message(req.workspace_path, req.chat_id, "user", req.prompt, {"attachments": req.attachments} if req.attachments else None)
    resolved = resolve_request_text(prompt_with_attachments, workspace_path=req.workspace_path, chat_id=req.chat_id)

    # Go directly to the main agent
    async def build_stream() -> AsyncGenerator[str, None]:
        """Stream state updates from the LangGraph pipeline via SSE.
        Yields graph-level events AND periodic intermediate snapshots during
        Developer tool-using loops so the frontend shows thoughts in real-time."""
        done = False
        had_error = False
        last_log_len = 0
        cancel_event = threading.Event()

        def run_in_thread():
            nonlocal done, had_error
            try:
                is_general = not req.workspace_path
                if is_general:
                    from free_agent import run_freeagent_direct
                    image_paths = [item.get("path") for item in req.attachments
                                   if item.get("isImage") and item.get("path")]
                    for state_snapshot in run_freeagent_direct(resolved, chat_id=req.chat_id, images=image_paths or None):
                        q.put(state_snapshot)
                else:
                    for state_snapshot in run_and_sync_graph(resolved, workspace_path=req.workspace_path, chat_id=req.chat_id, cancel_event=cancel_event):
                        q.put(state_snapshot)
                q.put(None)  # signal completion
            except Exception as e:
                import traceback
                had_error = True
                # Read the current shared state so the frontend sees the
                # live_terminal_log with crash details (added by sync_manager).
                # A bare {"error": ...} dict lacks live_terminal_log and
                # would cause the frontend to show "Task complete." instead.
                try:
                    err_state = json.loads(safe_serialize_state())
                except Exception:
                    err_state = {}
                err_state["error"] = str(e)
                err_state["traceback"] = traceback.format_exc()
                q.put(err_state)
                q.put(None)
            finally:
                done = True

        q: queue.Queue = queue.Queue()
        thread = threading.Thread(target=run_in_thread, daemon=True)
        thread.start()

        last_ver = 0
        while not done or not q.empty():
            if await request.is_disconnected():
                cancel_event.set()  # Signal the graph thread to stop
                break
            try:
                item = q.get(timeout=0.05)
                if item is None:
                    break
                if isinstance(item, dict):
                    yield _format_sse("state", item)
                    last_ver = item.get("state_version", 0)
            except queue.Empty:
                if not done:
                    snap = json.loads(safe_serialize_state())
                    cur_ver = snap.get("state_version", 0)
                    if cur_ver > last_ver:
                        last_ver = cur_ver
                        snap["_intermediate"] = True
                        yield _format_sse("state", snap)
                await asyncio.sleep(0.05)

        # Save token usage for the chat
        _save_chat_token_usage(req.workspace_path, req.chat_id)

        # The frontend persists one unified trace message containing the final
        # answer and execution metadata. Do not save a second plain agent
        # message here, otherwise every response appears twice in the chat.
        agent_report = safe_get_state().get("outputs", {}).get("agent_report", "")

        # Also save a session note to .agent-memory/ so the ORCHESTRATOR (not chat) remembers
        try:
            if agent_report and req.workspace_path:
                from memory_manager import update_project_meta
                snippet = agent_report[:500].replace("\n", " ")
                update_project_meta(req.workspace_path, "Last Session", snippet)
        except Exception as e:
            print(f"[API] Error saving session memory: {e}")

        # Final state
        yield _format_sse("state", json.loads(safe_serialize_state()))
        yield _format_sse("done", {"status": "error" if had_error else "complete"})

    return StreamingResponse(build_stream(), media_type="text/event-stream")


@app.post("/api/session/reset")
async def reset_session():
    """Reset the shared state for a new session."""
    empty_outputs = {
        "requirements": "", "tech_spec": "", "code": "",
        "test_report": "", "devops_config": "", "analytics_report": "",
        "gherkin": "", "mermaid": "",
    }
    safe_update_state({
        "active_node": "",
        "next_agent": "",
        "completed_nodes": [],
        "thoughts": {
            "supervisor": "", "developer": "",
        },
        "client_request": "",
        "outputs": empty_outputs,
        "project_path": "",
        "agents_plan": "",
        "deep_agents_log": [],
        "live_terminal_log": "[SESSION] Reset. Ready for a new request.\n",
        "token_usage": {
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cache_hit_tokens": 0,
            "total_cache_miss_tokens": 0,
            "total_cost": 0.0,
            "calls": [],
        },
    })
    return JSONResponse(content={"status": "ok"})


@app.get("/api/history")
async def get_history():
    """Return recent prompt history."""
    try:
        from chat_memory_manager import load_chat_memory
        mem = load_chat_memory()
        messages = mem.get("current_day_messages", [])
        user_prompts = [m["text"] for m in messages if m.get("sender") == "user"]
        seen = set()
        user_prompts = [x for x in user_prompts if not (x in seen or seen.add(x))]
        return JSONResponse(content={"prompts": user_prompts[:10] if user_prompts else ["No recent prompts"]})
    except Exception:
        return JSONResponse(content={"prompts": ["No recent prompts"]})


@app.get("/api/models")
async def get_models():
    """Return available model list."""
    return JSONResponse(content={
        "models": [
            {"id": "Automatic Fallback", "name": "Automatic Fallback", "provider": "auto"},
            {"id": "deepseek/deepseek-v4-flash-20260423:free", "name": "DeepSeek V4 Flash (Free)", "provider": "openrouter"},
            {"id": "deepseek/deepseek-chat", "name": "DeepSeek Chat", "provider": "openrouter"},
            {"id": "deepseek/deepseek-r1", "name": "DeepSeek R1", "provider": "openrouter"},
            {"id": "openrouter/owl-alpha", "name": "Owl Alpha (OpenRouter)", "provider": "openrouter"},
            {"id": "qwen/qwen3-coder-480b-a35b-07-25:free", "name": "Qwen Coder 480B (Free)", "provider": "openrouter"},
            {"id": "nvidia/nemotron-3-ultra-550b-a55b:free", "name": "Nemotron Ultra 550B (Free)", "provider": "openrouter"},
            {"id": "google/gemma-4-31b-it:free", "name": "Gemma 4 31B (Free)", "provider": "openrouter"},
            {"id": "openai/gpt-oss-120b:free", "name": "GPT-OSS 120B (Free)", "provider": "openrouter"},
            {"id": "openai/gpt-oss-20b:free", "name": "GPT-OSS 20B (Free)", "provider": "openrouter"},
            {"id": "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free", "name": "Nemotron Nano 30B (Free)", "provider": "openrouter"},
            {"id": "groq/llama-3.3-70b-versatile", "name": "Llama 3.3 70B (Groq)", "provider": "groq"},
            {"id": "groq/llama-3.1-8b-instant", "name": "Llama 3.1 8B (Groq)", "provider": "groq"},
            {"id": "gemini/gemini-2.5-flash", "name": "Gemini 2.5 Flash", "provider": "gemini"},
            {"id": "gemini/gemini-1.5-pro", "name": "Gemini 1.5 Pro", "provider": "gemini"},
            {"id": "gemini/gemini-1.5-flash", "name": "Gemini 1.5 Flash", "provider": "gemini"},
        ]
    })


@app.get("/api/health")
async def health():
    return {"status": "ok", "service": "Deep Agents API v2.0"}


# ═══════════════════════════════════════════════════════════════════════════════
# Workspace & Chat API
# ═══════════════════════════════════════════════════════════════════════════════

@app.get("/api/workspaces")
async def get_workspaces():
    return JSONResponse(content={"workspaces": wm.list_workspaces()})


@app.post("/api/workspaces")
async def create_workspace(req: Request):
    body = await req.json()
    path = body.get("path", "").strip()
    name = body.get("name", "").strip()
    if not path:
        return JSONResponse(content={"error": "Path required"}, status_code=400)
    try:
        ws = wm.add_workspace(path, name)
        return JSONResponse(content=ws)
    except ValueError as e:
        return JSONResponse(content={"error": str(e)}, status_code=400)


@app.delete("/api/workspaces/{workspace_id}")
async def delete_workspace(workspace_id: str):
    ok = wm.remove_workspace(workspace_id)
    return JSONResponse(content={"status": "ok" if ok else "not found"})


@app.get("/api/workspaces/{workspace_id}/chats")
async def get_chats(workspace_id: str):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    chats = wm.list_chats(ws["path"])
    return JSONResponse(content={"chats": chats})


@app.post("/api/workspaces/{workspace_id}/chats")
async def create_chat_endpoint(workspace_id: str, req: Request):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    body = await req.json()
    title = body.get("title", "")
    model = body.get("model", "Automatic Fallback")
    chat = wm.create_chat(ws["path"], title, model)
    return JSONResponse(content=chat)


@app.get("/api/workspaces/{workspace_id}/chats/{chat_id}")
async def get_chat_endpoint(workspace_id: str, chat_id: str):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    chat = wm.get_chat(ws["path"], chat_id)
    if not chat:
        return JSONResponse(content={"error": "Chat not found"}, status_code=404)
    return JSONResponse(content=chat)


@app.get("/api/workspaces/{workspace_id}/chats/{chat_id}/subagents/{subagent_name}")
async def get_subagent_history_endpoint(workspace_id: str, chat_id: str, subagent_name: str):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    
    valid_subagents = {"Dev", "BSA", "DevSecOps", "SDET", "FreeAgent", "Vision"}
    if subagent_name not in valid_subagents:
        return JSONResponse(content={"error": "Invalid subagent name"}, status_code=400)
    
    from subagent_swarm import load_subagent_history
    try:
        messages = load_subagent_history(chat_id, subagent_name)
        serialized = []
        for m in messages:
            role = "user"
            m_type = type(m).__name__
            if m_type == "SystemMessage":
                role = "system"
            elif m_type == "AIMessage":
                role = "assistant"
            elif m_type == "HumanMessage":
                role = "user"
            
            serialized.append({
                "role": role,
                "content": m.content,
            })
        return JSONResponse(content=serialized)
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


@app.delete("/api/workspaces/{workspace_id}/chats/{chat_id}")
async def delete_chat_endpoint(workspace_id: str, chat_id: str):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    ok = wm.delete_chat(ws["path"], chat_id)
    return JSONResponse(content={"status": "ok" if ok else "not found"})


@app.post("/api/workspaces/{workspace_id}/chats/{chat_id}/messages")
async def add_message_endpoint(workspace_id: str, chat_id: str, req: Request):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    body = await req.json()
    role = body.get("role", "user")
    content = body.get("content", "")
    meta = body.get("metadata")
    msg = wm.add_message(ws["path"], chat_id, role, content, meta)
    if not msg:
        return JSONResponse(content={"error": "Chat not found"}, status_code=404)
    return JSONResponse(content=msg)


@app.post("/api/workspaces/{workspace_id}/chats/{chat_id}/attachments")
async def upload_chat_attachments(workspace_id: str, chat_id: str, files: list[UploadFile] = File(...)):
    """Store chat attachments outside the workspace and return safe agent-readable paths."""
    ws = wm.get_workspace(workspace_id)
    if not ws or not wm.get_chat(ws["path"], chat_id):
        return JSONResponse(content={"error": "Chat not found"}, status_code=404)
    if len(files) > 8:
        return JSONResponse(content={"error": "A maximum of 8 files can be uploaded"}, status_code=400)

    upload_dir = os.path.join(os.path.dirname(__file__), ".deep_agents", "uploads", workspace_id, chat_id)
    os.makedirs(upload_dir, exist_ok=True)
    attachments = []
    for item in files:
        filename = os.path.basename(item.filename or "attachment")
        contents = await item.read()
        if len(contents) > 20 * 1024 * 1024:
            return JSONResponse(content={"error": f"{filename} is larger than 20 MB"}, status_code=400)
        stored_name = f"{uuid.uuid4().hex[:8]}-{filename}"
        target = os.path.join(upload_dir, stored_name)
        with open(target, "wb") as handle:
            handle.write(contents)
        mime = item.content_type or "application/octet-stream"
        attachments.append({
            "id": stored_name, "name": filename, "type": mime, "size": len(contents),
            "path": target, "isImage": mime.startswith("image/"),
        })
    return JSONResponse(content={"attachments": attachments})


@app.put("/api/workspaces/{workspace_id}/chats/{chat_id}/title")
async def update_chat_title(workspace_id: str, chat_id: str, req: Request):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    body = await req.json()
    title = body.get("title", "")
    ok = wm.update_chat_title(ws["path"], chat_id, title)
    return JSONResponse(content={"status": "ok" if ok else "not found"})


# ═══════════════════════════════════════════════════════════════════════════════
# Settings & Rules API
# ═══════════════════════════════════════════════════════════════════════════════


@app.get("/api/settings/general-profile")
async def get_general_profile():
    """Get the General mode user profile (instructions + memory)."""
    profile_path = r"d:\MyProject\LangChain\.deep_agents\general_profile.json"
    default = {
        "user_info": {"name": "", "birthday": "", "education": ""},
        "instructions": "",
        "memory": []
    }
    if os.path.isfile(profile_path):
        try:
            with open(profile_path, "r", encoding="utf-8") as f:
                return JSONResponse(content=json.load(f))
        except Exception:
            pass
    return JSONResponse(content=default)


@app.post("/api/settings/general-profile")
async def save_general_profile(req: Request):
    """Save the General mode user profile."""
    profile_path = r"d:\MyProject\LangChain\.deep_agents\general_profile.json"
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(profile_path), exist_ok=True)
        with open(profile_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


@app.get("/api/settings/profile")

async def get_profile():
    profile_path = r"d:\MyProject\LangChain\.deep_agents\user_profile.json"
    default_profile = {
        "user_info": {"name": "Davin Akmal Yasha"},
        "global_rules": []
    }
    if os.path.isfile(profile_path):
        try:
            with open(profile_path, "r", encoding="utf-8") as f:
                return JSONResponse(content=json.load(f))
        except Exception:
            pass
    return JSONResponse(content=default_profile)

@app.post("/api/settings/profile")
async def save_profile(req: Request):
    profile_path = r"d:\MyProject\LangChain\.deep_agents\user_profile.json"
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(profile_path), exist_ok=True)
        with open(profile_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)

@app.get("/api/workspaces/{workspace_id}/rules")
async def get_workspace_rules(workspace_id: str):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    rules_path = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "rules", f"{workspace_id}.json")
    default_rules = {
        "stack": {},
        "workspace_rules": []
    }
    if os.path.isfile(rules_path):
        try:
            with open(rules_path, "r", encoding="utf-8") as f:
                return JSONResponse(content=json.load(f))
        except Exception:
            pass
    return JSONResponse(content=default_rules)

@app.post("/api/workspaces/{workspace_id}/rules")
async def save_workspace_rules(workspace_id: str, req: Request):
    ws = wm.get_workspace(workspace_id)
    if not ws:
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    rules_path = os.path.join(r"d:\MyProject\LangChain", ".deep_agents", "rules", f"{workspace_id}.json")
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(rules_path), exist_ok=True)
        with open(rules_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


def start_consolidation_daemon():
    """Starts a periodic background daemon thread for memory consolidation."""
    import time
    from memory_consolidation_daemon import consolidate_memories
    
    def daemon_loop():
        print("[DAEMON] Background memory consolidation daemon started.")
        while True:
            try:
                consolidate_memories()
            except Exception as e:
                print(f"[DAEMON] Memory consolidation failed: {e}")
            # Run every 6 hours
            time.sleep(21600)
            
    threading.Thread(target=daemon_loop, daemon=True).start()


# ── Voice transcription ────────────────────────────────────────────────────────

@app.post("/api/transcribe")
async def transcribe_audio(file: UploadFile = File(...)):
    """Transcribe audio using Groq Whisper (free STT)."""
    GROQ_API_KEY = os.getenv("GROQ_API_KEY") or ""
    if not GROQ_API_KEY:
        # Fallback: try comma-separated env var
        keys_str = os.getenv("GROQ_API_KEYS", "")
        if keys_str:
            GROQ_API_KEY = keys_str.split(",")[0].strip()
    if not GROQ_API_KEY:
        return {"error": "Groq API key not configured (set GROQ_API_KEY or GROQ_API_KEYS)"}, 400

    audio_bytes = await file.read()
    if not audio_bytes:
        return {"error": "Empty audio file"}, 400

    try:
        import httpx
        # Build multipart form for Groq Whisper
        files = {
            "file": (file.filename or "audio.webm", audio_bytes, file.content_type or "audio/webm"),
            "model": (None, "whisper-large-v3-turbo"),
        }
        headers = {"Authorization": f"Bearer {GROQ_API_KEY}"}
        async with httpx.AsyncClient(timeout=30) as client:
            resp = await client.post(
                "https://api.groq.com/openai/v1/audio/transcriptions",
                files=files,
                headers=headers,
            )
        if resp.status_code != 200:
            print(f"[Transcribe] Groq API error {resp.status_code}: {resp.text}")
            return {"error": f"Groq API error: {resp.status_code}"}, 502
        result = resp.json()
        text = result.get("text", "").strip()
        if not text:
            return {"text": ""}, 200
        return {"text": text}, 200
    except Exception as e:
        print(f"[Transcribe] Error: {e}")
        return {"error": str(e)}, 500


# ── Startup ─────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import uvicorn
    import sys
    
    reload_mode = "--reload" in sys.argv

    _start_task_scheduler()
    
    # Start Telegram bot in background
    try:
        from bot import bot
        threading.Thread(target=bot.infinity_polling, daemon=True).start()
        print("[API] Telegram bot started in background.")
    except Exception as e:
        print(f"[API] Failed to start Telegram bot: {e}")
        
    # Start Memory Consolidation Daemon in background
    try:
        start_consolidation_daemon()
    except Exception as e:
        print(f"[API] Failed to start memory consolidation daemon: {e}")
        
    if reload_mode:
        uvicorn.run("api_server:app", host="127.0.0.1", port=8000, reload=True)
    else:
        print("[API] Running in production mode (reload disabled).")
        uvicorn.run(app, host="127.0.0.1", port=8000)
