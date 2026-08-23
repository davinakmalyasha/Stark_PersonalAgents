import difflib
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)
_DESKTOP = os.path.join(_ROOT, "Desktop")
if _DESKTOP not in sys.path:
    sys.path.insert(0, _DESKTOP)
_VOICE = os.path.join(_ROOT, "Voice")
if _VOICE not in sys.path:
    sys.path.insert(0, _VOICE)
"""
FastAPI server — Bridges the React frontend with the LangGraph multi-agent pipeline.
Replaces the Gradio dashboard with SSE streaming endpoints.
"""
import os
import sys

# ── Interpreter guard ─────────────────────────────────────────────────────
# The voice engine (Moonshine + Kokoro) only exists in venv312. No matter how
# this file is launched (`python api_server.py`, a .bat, Electron, ...) we
# re-exec ourselves with venv312 so the local ASR/TTS always work.
def _ensure_venv312() -> None:
    try:
        here = os.path.dirname(os.path.abspath(__file__))
        venv_py = os.path.join(here, "venv312", "Scripts", "python.exe")
        if not os.path.exists(venv_py):
            print("[api_server] WARNING: venv312 not found — voice engines (Moonshine/Kokoro) unavailable")
            return
        venv_dir = os.path.normpath(os.path.dirname(venv_py)).lower()
        if not os.path.normpath(sys.prefix).lower().startswith(os.path.dirname(venv_dir)):
            print(f"[api_server] Re-executing with {venv_py} (current: {sys.executable})")
            os.execv(venv_py, [venv_py] + sys.argv)
    except Exception as e:
        print(f"[api_server] Interpreter guard failed ({e}) — continuing")

_ensure_venv312()

# ── Voice module path (audio_engine, voice_services moved to Voice/) ──────────
_VOICE_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "Voice")
if _VOICE_DIR not in sys.path:
    sys.path.insert(0, _VOICE_DIR)

_DESKTOP_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)), "Desktop")
if _DESKTOP_DIR not in sys.path:
    sys.path.insert(0, _DESKTOP_DIR)

import uuid
import asyncio
import json
import threading
import time
import queue
import hashlib
import re
import tempfile
from typing import AsyncGenerator
from datetime import datetime, timezone, timedelta

from fastapi import FastAPI, Request, UploadFile, File, Form
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import StreamingResponse, JSONResponse, FileResponse
from pydantic import BaseModel
from dotenv import load_dotenv

from sync.sync_manager import run_and_sync_graph
from sync.state_sync import shared_state, safe_get_state, safe_update_state, safe_serialize_state, state_lock, reset_shared_state
import workspace_manager as wm

load_dotenv(override=True)

# Init default workspace on startup
wm.init_default_workspace()
# Clean up orphaned chat directories from previously deleted workspaces
_orphans_removed = wm.cleanup_orphaned_chat_dirs()
if _orphans_removed:
    print(f"[STARTUP] Cleaned up {_orphans_removed} orphaned workspace chat directories")

# Start MCP servers in the background (codex-style lifecycle: spawn once at
# startup, keep alive, cache tools/list). Connection happens on a daemon
# thread; tools become visible to the agents as servers report "connected".
try:
    from tools.mcp.mcp_bridge import mcp_start_all
    threading.Thread(target=mcp_start_all, kwargs={"background": True}, daemon=True).start()
except Exception as e:
    print(f"[STARTUP] MCP init failed: {e}")

app = FastAPI(title="Deep Agents API", version="2.0")

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_methods=["*"],
    allow_headers=["*"],
)


@app.get("/api/health/voice")
async def voice_health():
    """Engine status — quick check that Moonshine + Kokoro are loadable."""
    asr_ok = False
    tts_ok = False
    asr_detail = "ok"
    tts_detail = "ok"
    try:
        import audio_engine
        asr_ok = audio_engine.asr_available()
        if not asr_ok:
            asr_detail = "model files missing (run the moonshine download)"
    except Exception as e:
        asr_detail = f"unavailable: {e}"
    try:
        import kokoro  # noqa: F401
        tts_ok = True
    except Exception as e:
        tts_detail = f"unavailable: {e}"
    return JSONResponse(content={
        "interpreter": sys.executable,
        "asr": {"ok": asr_ok, "detail": asr_detail},
        "tts": {"ok": tts_ok, "detail": tts_detail},
    })


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


class TTSRequest(BaseModel):
    text: str
    voice: str = "en-US-JennyNeural"
    engine: str = "auto"  # auto | edge | kokoro


class CleanupRequest(BaseModel):
    text: str


class TaskUpdateRequest(BaseModel):
    status: str | None = None
    prompt: str | None = None
    attachments: list[dict] | None = None
    mode: str | None = None
    schedule: dict | None = None


_task_scheduler_started = False
_run_contexts: dict[str, dict] = {}
_run_contexts_lock = threading.Lock()


def _git_status_snapshot(project_path: str) -> dict[str, str]:
    """Return git status entries keyed by path for one run's change summary."""
    if not project_path:
        return {}
    try:
        import subprocess
        result = subprocess.run(
            ["git", "status", "--porcelain=v1", "--untracked-files=all"],
            capture_output=True,
            text=True,
            cwd=project_path,
            timeout=5,
        )
        if result.returncode != 0:
            return {}
        snapshot: dict[str, str] = {}
        for line in result.stdout.splitlines():
            if len(line) < 4:
                continue
            status = line[:2]
            path = line[3:]
            if " -> " in path:
                path = path.split(" -> ", 1)[1]
            snapshot[path] = status
        return snapshot
    except Exception:
        return {}


def _normalize_artifact_path(project_path: str, path: str) -> str | None:
    """Return a safe workspace-relative path, or None for an out-of-scope path."""
    if not path:
        return None
    root = os.path.realpath(project_path)
    candidate = os.path.realpath(path if os.path.isabs(path) else os.path.join(root, path))
    try:
        if os.path.commonpath([root, candidate]) != root:
            return None
    except ValueError:
        return None
    return os.path.relpath(candidate, root).replace("\\", "/")


def _read_file_bytes(project_path: str, relative_path: str) -> bytes | None:
    normalized = _normalize_artifact_path(project_path, relative_path)
    if normalized is None:
        return None
    absolute = os.path.join(project_path, normalized)
    try:
        with open(absolute, "rb") as handle:
            return handle.read()
    except OSError:
        return None


def _file_hash(content: bytes | None) -> str | None:
    return hashlib.sha256(content).hexdigest() if content is not None else None


def _git_head_bytes(project_path: str, relative_path: str) -> bytes | None:
    try:
        import subprocess
        result = subprocess.run(
            ["git", "show", f"HEAD:{relative_path}"],
            capture_output=True,
            cwd=project_path,
            timeout=5,
        )
        return result.stdout if result.returncode == 0 else None
    except Exception:
        return None


def _capture_run_context(project_path: str, chat_id: str) -> dict:
    baseline = _git_status_snapshot(project_path)
    dirty_files: dict[str, bytes | None] = {}
    for path in baseline:
        normalized = _normalize_artifact_path(project_path, path)
        if normalized is not None and baseline[path] != "  ":
            dirty_files[normalized] = _read_file_bytes(project_path, normalized)
    return {
        "run_id": f"run-{uuid.uuid4().hex[:16]}",
        "workspace_path": project_path,
        "chat_id": chat_id,
        "baseline": baseline,
        "dirty_files": dirty_files,
        "artifacts": None,
        "undone": False,
    }


def _text_diff(before: bytes | None, after: bytes | None, path: str) -> tuple[str, int, int]:
    """Return a readable unified diff and line additions/deletions."""
    try:
        before_lines = (before or b"").decode("utf-8").splitlines(keepends=True)
        after_lines = (after or b"").decode("utf-8").splitlines(keepends=True)
    except UnicodeDecodeError:
        return "", 0, 0
    diff = "".join(difflib.unified_diff(
        before_lines,
        after_lines,
        fromfile=f"a/{path}",
        tofile=f"b/{path}",
    ))
    additions = 0
    deletions = 0
    for tag, start_before, end_before, start_after, end_after in difflib.SequenceMatcher(None, before_lines, after_lines).get_opcodes():
        if tag in ("replace", "delete"):
            deletions += end_before - start_before
        if tag in ("replace", "insert"):
            additions += end_after - start_after
    return diff, additions, deletions


def _collect_run_artifacts(project_path: str, context: dict) -> dict:
    """Build the file-change summary and review data for one agent run."""
    baseline = context.get("baseline", {})
    current = _git_status_snapshot(project_path)
    candidate_paths: set[str] = set()

    for path, status in current.items():
        normalized = _normalize_artifact_path(project_path, path)
        if normalized is not None and (path not in baseline or baseline[path] != status):
            candidate_paths.add(normalized)

    # Supplement Git's status delta with the task tracker. This catches edits
    # to files that were already dirty before the run.
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


@app.post("/api/runs/{run_id}/undo")
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


# ── MCP servers ──────────────────────────────────────────────────────────────

class McpServerUpdate(BaseModel):
    command: str = ""
    args: list[str] = []
    url: str = ""
    env: dict[str, str] = {}
    env_vars: list[str] = []
    cwd: str = ""
    scopes: list[str] = []
    enabled_tools: list[str] = []
    disabled_tools: list[str] = []
    approval: str = "auto"  # auto | prompt
    instructions: str = ""
    startup_timeout_sec: float = 10.0
    tool_timeout_sec: float = 60.0


def _mcp_manager():
    from tools.mcp.mcp_bridge import get_manager
    return get_manager()


@app.get("/api/mcp/servers")
async def list_mcp_servers():
    """List configured servers and their live connection status."""
    mgr = _mcp_manager()
    status = {s["name"]: s for s in mgr.status_snapshot()}
    out = []
    for name, cfg in mgr.load_config().get("servers", {}).items():
        merged = dict(cfg)
        merged["name"] = name
        merged["status"] = status.get(name, {}).get("status", "stopped")
        merged["error"] = status.get(name, {}).get("error", "")
        merged["tool_count"] = status.get(name, {}).get("tool_count", 0)
        merged["transport"] = "http" if cfg.get("url") else "stdio"
        out.append(merged)
    return JSONResponse(content={"servers": out})


@app.post("/api/mcp/servers/{server_name}")
async def upsert_mcp_server(server_name: str, update: McpServerUpdate):
    """Create or replace a server config; restarts it if configured."""
    try:
        snapshot = _mcp_manager().upsert_server(server_name, update.model_dump(exclude_none=True))
        return JSONResponse(content={"servers": snapshot})
    except ValueError as e:
        return JSONResponse(content={"error": str(e)}, status_code=400)


@app.delete("/api/mcp/servers/{server_name}")
async def delete_mcp_server(server_name: str):
    """Remove a server config and disconnect it."""
    snapshot = _mcp_manager().remove_server(server_name)
    return JSONResponse(content={"servers": snapshot})


@app.post("/api/mcp/servers/{server_name}/restart")
async def restart_mcp_server(server_name: str):
    """Disconnect and reconnect a single server (pick up config changes)."""
    try:
        snapshot = _mcp_manager().restart_server(server_name)
        return JSONResponse(content={"servers": snapshot})
    except ValueError as e:
        return JSONResponse(content={"error": str(e)}, status_code=404)


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
            from agents.free_agent import run_freeagent_direct
            for _ in run_freeagent_direct(prompt, chat_id=chat_id, images=[a.get("path") for a in attachments if a.get("isImage") and a.get("path")] or None):
                pass
        else:
            from verify_helper import run_graph_with_verify
            for _ in run_graph_with_verify(prompt, workspace_path=workspace_path, chat_id=chat_id):
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
    from memory.chat_memory_manager import resolve_request_text

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
        # Vision: stash image paths so the agent's Zone-2 builder can inject
        # them as real image blocks (visible from turn one). FreeAgent gets
        # them directly below; the graph path consumes the shared_state key.
        try:
            _img_paths = [item.get("path") for item in req.attachments
                          if item.get("isImage") and item.get("path")]
            if _img_paths:
                safe_update_state({"attachment_image_paths": [str(p) for p in _img_paths]})
        except Exception:
            pass
    prompt_with_attachments = req.prompt + attachment_note
    wm_add_message(req.workspace_path, req.chat_id, "user", req.prompt, {"attachments": req.attachments} if req.attachments else None)
    resolved = resolve_request_text(prompt_with_attachments, workspace_path=req.workspace_path, chat_id=req.chat_id)
    run_context = _capture_run_context(req.workspace_path, req.chat_id)

    # P9: baseline workspace ids so the final snapshot can flag newly created
    # projects (orchestrator create_project tool) for the sidebar refresh.
    try:
        _workspace_ids_before_run = {w.get("id") for w in wm.list_workspaces()}
    except Exception:
        _workspace_ids_before_run = set()

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
                # ── P9: direct-to-Dev routing — messages bound for an existing
                # project's Dev agent skip the orchestrator entirely. This runs
                # before the general/workspace split so cross-project routing
                # works from the General chat too. New project creation is
                # decided by the orchestrator LLM (create_project tool), never
                # hardcoded here.
                # NOTE: when the message comes FROM a project chat we fall
                # through to run_graph_with_verify — it routes the same way and
                # still runs the verification gate after the Dev turn.
                try:
                    if not req.workspace_path:
                        from agents.project_router import route_request, stream_direct
                        route_kind, project = route_request(resolved, "", req.chat_id)
                        if route_kind == "project_dev" and project:
                            for state_snapshot in stream_direct(project, resolved, "", req.chat_id):
                                q.put(state_snapshot)
                            q.put(None)  # signal completion
                            return
                except Exception as e:
                    print(f"[API] Project routing error (falling back to pipeline): {e}")

                is_general = not req.workspace_path
                if is_general:
                    from agents.free_agent import run_freeagent_direct
                    image_paths = [item.get("path") for item in req.attachments
                                   if item.get("isImage") and item.get("path")]
                    for state_snapshot in run_freeagent_direct(resolved, chat_id=req.chat_id, images=image_paths or None):
                        q.put(state_snapshot)
                else:
                    from verify_helper import run_graph_with_verify
                    for state_snapshot in run_graph_with_verify(resolved, workspace_path=req.workspace_path, chat_id=req.chat_id, cancel_event=cancel_event):
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
        last_bus_drain = 0.0
        while not done or not q.empty():
            if await request.is_disconnected():
                cancel_event.set()  # Signal the graph thread to stop
                break

            # ── P10: forward live token deltas from the StreamBus (text /
            # reasoning / end) as SSE `delta` frames ──
            try:
                from stream_bus import drain as _bus_drain, cleanup as _bus_cleanup
                deltas = _bus_drain(req.chat_id)
                if deltas:
                    for d in deltas:
                        yield _format_sse("delta", d)
                now = time.time()
                if now - last_bus_drain > 60:
                    last_bus_drain = now
                    _bus_cleanup()
            except Exception:
                pass

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
        # Final state, including the files changed during this run.
        final_state = json.loads(safe_serialize_state())
        final_state["artifacts"] = _collect_run_artifacts(req.workspace_path, run_context)

        # P9: tell the frontend a new project workspace appeared this run
        # (created via the orchestrator's create_project tool) so the sidebar
        # can refresh without a manual reload.
        try:
            new_ids = {w.get("id") for w in wm.list_workspaces()} - _workspace_ids_before_run
            if new_ids:
                final_state["workspace_created"] = True
                final_state["created_workspace_ids"] = sorted(new_ids)
        except Exception:
            pass

        yield _format_sse("state", final_state)
        yield _format_sse("done", {"status": "error" if had_error else "complete"})

    return StreamingResponse(build_stream(), media_type="text/event-stream")


class InboxRequest(BaseModel):
    chat_id: str = ""
    message: str = ""


@app.post("/api/steer")
async def api_steer(req: InboxRequest):
    """Queue a steering message: delivered after the current step's tool calls."""
    from inbox import steer, pending_counts
    ok = steer(req.chat_id, req.message)
    return {"ok": ok, "pending": pending_counts(req.chat_id)}


@app.post("/api/followup")
async def api_followup(req: InboxRequest):
    """Queue a follow-up message: delivered only after the agent is idle."""
    from inbox import follow_up, pending_counts
    ok = follow_up(req.chat_id, req.message)
    return {"ok": ok, "pending": pending_counts(req.chat_id)}


@app.post("/api/abort")
async def api_abort(req: InboxRequest):
    """Abort the current run; queued steering/follow-up are preserved."""
    from inbox import abort, pending_counts
    abort(req.chat_id, keep_inbox=True)
    return {"ok": True, "pending": pending_counts(req.chat_id)}


@app.get("/api/inbox")
async def api_inbox(chat_id: str = ""):
    from inbox import pending_counts
    return pending_counts(chat_id)


# ═══════════════════════════════════════════════════════════════════════════════
# Workspace file explorer (P10/Tier-3)
# ═══════════════════════════════════════════════════════════════════════════════

_FILE_SKIP_DIRS = {
    ".git", ".hg", ".svn", "node_modules", "venv", "venv312", ".venv",
    "__pycache__", ".pytest_cache", ".ruff_cache", ".agent-memory",
    ".deep_agents", "dist", "build", ".next", "target", "release",
}
_FILE_MAX_SIZE = 5 * 1024 * 1024  # skip files larger than 5 MB in listings


@app.get("/api/workspaces/{workspace_id}/files")
async def list_workspace_files(workspace_id: str, max_depth: int = 8):
    """Flat listing of a workspace folder tree (dirs then files, depth-capped)."""
    ws = wm.get_workspace(workspace_id)
    if not ws or not ws.get("path") or not os.path.isdir(ws["path"]):
        return JSONResponse(content={"files": [], "error": "Workspace folder not found"}, status_code=404)
    root = ws["path"]
    out = []

    def walk(d: str, depth: int, prefix: str) -> None:
        if depth > max_depth:
            return
        try:
            entries = sorted(os.listdir(d), key=str.lower)
        except Exception:
            return
        for name in entries:
            if name in _FILE_SKIP_DIRS:
                continue
            full = os.path.join(d, name)
            rel = os.path.join(prefix, name) if prefix else name
            try:
                if os.path.isdir(full):
                    out.append({"type": "dir", "path": rel, "name": name})
                    walk(full, depth + 1, rel)
                else:
                    size = os.path.getsize(full)
                    if size > _FILE_MAX_SIZE:
                        continue
                    out.append({"type": "file", "path": rel, "name": name, "size": size})
            except Exception:
                continue

    walk(root, 0, "")
    return {"files": out, "root": root}


@app.get("/api/workspaces/{workspace_id}/file")
async def get_workspace_file(workspace_id: str, path: str = ""):
    """Read a file from the workspace (path-traversal safe, 200 KB cap)."""
    ws = wm.get_workspace(workspace_id)
    if not ws or not ws.get("path"):
        return JSONResponse(content={"error": "Workspace not found"}, status_code=404)
    root = os.path.realpath(ws["path"])
    target = os.path.realpath(os.path.join(root, path))
    if target != root and not target.startswith(root + os.sep):
        return JSONResponse(content={"error": "Path escapes the workspace"}, status_code=400)
    if not os.path.isfile(target):
        return JSONResponse(content={"error": "File not found"}, status_code=404)
    try:
        with open(target, "r", encoding="utf-8", errors="replace") as f:
            content = f.read(200_000)
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)
    return {"path": path, "content": content, "truncated": len(content) >= 200_000}


class AskAnswerRequest(BaseModel):
    batch_id: str
    answers: list[dict] = []


@app.get("/api/ask/pending")
async def api_ask_pending():
    """Unanswered ask_user batches (frontend polls while an agent blocks)."""
    from user_questions import pending_batches
    return {"batches": pending_batches()}


@app.post("/api/ask/answer")
async def api_ask_answer(req: AskAnswerRequest):
    """Resolve one pending ask_user batch with the human's answers."""
    from user_questions import resolve_batch
    ok = resolve_batch(req.batch_id, req.answers)
    if not ok:
        return JSONResponse(content={"ok": False, "error": "unknown batch"}, status_code=404)
    return {"ok": True}



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
        from memory.chat_memory_manager import load_chat_memory
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
            {"id": "deepseek-v4-flash-vision-exp", "name": "DeepSeek V4 Flash Vision Exp (Go)", "provider": "opencode"},
            {"id": "deepseek/deepseek-v4-flash", "name": "DeepSeek V4 Flash 0731 (Free)", "provider": "cline"},
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


@app.get("/api/provider")
async def get_provider():
    """Return the active LLM provider (deepseek | opencode) and its resolved config summary."""
    from llm import _resolve_llm_provider, _load_persisted_provider
    provider = safe_get_state().get("llm_provider", "") or _load_persisted_provider() or "deepseek"
    cfg = _resolve_llm_provider()
    return JSONResponse(content={
        "provider": provider,
        "base_url": cfg["base"],
        "model_flash": cfg["flash"],
        "model_pro": cfg["pro"],
        "model_vision": cfg.get("vision", ""),
        "has_key": bool(cfg["key"]) and not cfg["key"].startswith("your_"),
    })


@app.post("/api/provider")
async def set_provider(req: Request):
    """Set the active LLM provider for subsequent runs AND persist it (deepseek | opencode)."""
    from llm import _save_persisted_provider
    body = await req.json()
    provider = str(body.get("provider", "")).strip().lower()
    if provider not in ("deepseek", "opencode"):
        return JSONResponse(content={"error": f"Invalid provider '{provider}'. Use 'deepseek' or 'opencode'."}, status_code=400)
    safe_update_state({"llm_provider": provider})
    _save_persisted_provider(provider)
    return JSONResponse(content={"provider": provider, "ok": True})


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
    return JSONResponse(content={"error": "Subagent history endpoint removed (legacy swarm module deleted). Use session_log directly."}, status_code=410)


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
    harness_root = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)); profile_path = os.path.join(harness_root, ".deep_agents", "general_profile.json")
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
    harness_root = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)); profile_path = os.path.join(harness_root, ".deep_agents", "general_profile.json")
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
    harness_root = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    profile_path = os.path.join(harness_root, ".deep_agents", "user_profile.json")
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
    harness_root = os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__))
    profile_path = os.path.join(harness_root, ".deep_agents", "user_profile.json")
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
    rules_path = os.path.join(os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)), ".deep_agents", "rules", f"{workspace_id}.json")
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
    rules_path = os.path.join(os.environ.get("HARNESS_ROOT") or os.path.dirname(os.path.abspath(__file__)), ".deep_agents", "rules", f"{workspace_id}.json")
    body = await req.json()
    try:
        os.makedirs(os.path.dirname(rules_path), exist_ok=True)
        with open(rules_path, "w", encoding="utf-8") as f:
            json.dump(body, f, indent=2)
        return {"status": "ok"}
    except Exception as e:
        return JSONResponse(content={"error": str(e)}, status_code=500)


# ── Voice transcription ────────────────────────────────────────────────────────

@app.post("/api/voice/start")
async def voice_start():
    """Start server-side mic capture (sounddevice, 16kHz raw — the pure Moonshine path)."""
    try:
        import audio_engine
        ok = await asyncio.to_thread(audio_engine.start_capture)
        return JSONResponse(content={"active": ok})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": f"Capture unavailable: {e}"})


@app.get("/api/voice/status")
async def voice_status():
    try:
        import audio_engine
        return JSONResponse(content={"active": audio_engine.capture_active()})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


@app.post("/api/voice/stop")
async def voice_stop(request: Request):
    """Stop capture and return the final transcript (official streaming flush).

    Body: {"discard": true} stops without transcribing (mic turned off).
    """
    discard = False
    try:
        body = await request.json()
        discard = bool(body.get("discard"))
    except Exception:
        pass
    try:
        import audio_engine
        text = (await asyncio.to_thread(audio_engine.stop_capture, discard) or "").strip()
        if discard:
            return JSONResponse(content={"text": "", "engine": "moonshine", "discarded": True})
        if not text:
            return JSONResponse(status_code=422, content={"error": "Moonshine could not transcribe the audio (empty result)"})
        return JSONResponse(content={"text": text, "engine": "moonshine"})
    except Exception as e:
        import traceback
        print(f"[Voice] stop/transcribe failed: {e}")
        traceback.print_exc()
        return JSONResponse(status_code=503, content={"error": f"Moonshine ASR unavailable: {e}"})


@app.post("/api/voice/mute")
async def voice_mute(request: Request):
    """Mute the capture while Stark speaks (prevents self-transcription)."""
    muted = False
    try:
        body = await request.json()
        muted = bool(body.get("muted"))
    except Exception:
        pass
    try:
        import audio_engine
        await asyncio.to_thread(audio_engine.set_capture_muted, muted)
        return JSONResponse(content={"muted": muted})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


@app.post("/api/voice/peek")
async def voice_peek():
    """Latest rolling transcript + live mic level (no model call)."""
    try:
        import audio_engine
        text = audio_engine.live_text()
        return JSONResponse(content={"text": text, "level": audio_engine.live_peak(), "active": audio_engine.capture_active()})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


@app.post("/api/transcribe")
async def transcribe_audio(file: UploadFile = File(...), language: str = Form("")):
    """Transcribe audio with the on-device Moonshine engine (free, CPU)."""
    audio_bytes = await file.read()
    if not audio_bytes:
        return JSONResponse(status_code=400, content={"error": "Empty audio file"})

    tmp_path = None
    try:
        import audio_engine
        tmp_path = os.path.join(tempfile.gettempdir(), f"stt_{uuid.uuid4().hex}.webm")
        with open(tmp_path, "wb") as f:
            f.write(audio_bytes)
        print(f"[Transcribe] received {len(audio_bytes)} bytes ({file.filename or 'round.webm'})")
        text = (await asyncio.to_thread(audio_engine.transcribe_file, tmp_path) or "").strip()
        if not text:
            print("[Transcribe] empty transcript from Moonshine")
            return JSONResponse(status_code=422, content={"error": "Moonshine could not transcribe the audio (empty result)"})
        print(f"[Transcribe] OK: '{text[:100]}'")
        return JSONResponse(content={"text": text, "engine": "moonshine"})
    except Exception as e:
        import traceback
        print(f"[Transcribe] Local ASR unavailable: {e}")
        traceback.print_exc()
        return JSONResponse(status_code=503, content={"error": f"Moonshine ASR unavailable: {e}"})
    finally:
        if tmp_path and os.path.exists(tmp_path):
            try:
                os.remove(tmp_path)
            except OSError:
                pass


def _is_clean_polish(text: str) -> bool:
    """Reject free-model outputs that aren't a plain transcript rewrite.

    Free models sometimes role-play ("NextGS: ... User Safety: safe") or
    echo instructions instead of returning the corrected text only.
    """
    t = (text or "").strip()
    if not t or len(t) > 1200:
        return False
    if t.lower().startswith(("[freeagent", "error", "as an ai", "i can't")):
        return False
    if "reply with only the corrected text" in t.lower():
        return False
    # Speaker labels / role-play patterns
    if re.search(
        r"(^|\n)\s*(?:"
        r"(user|assistant|system|ai|human|bot|model|nextgs?|safety)[a-z]*\s*[:：]"
        r"|user\s+safety\s*[:：]"
        r"|\[(?:freeagent|ai|assistant|user)[^\]]*\]"
        r")",
        t,
        re.IGNORECASE | re.MULTILINE,
    ):
        return False
    return True


@app.post("/api/cleanup")
async def cleanup_text(req: CleanupRequest):
    """Grammar-polish a voice transcript with a free LLM call (Gemini-style fix).

    Fixes grammar/spelling/punctuation for imperfect speech-to-text output while
    keeping meaning, language (Indonesian/English) and technical terms.
    Long input (>800 chars) or suspicious model output passes through untouched.
    """
    text = (req.text or "").strip()
    if not text:
        return JSONResponse(content={"text": ""})
    if len(text) > 800:
        return JSONResponse(content={"text": text})
    try:
        from agents import free_agent
        prompt = (
            "You are a grammar corrector. Fix the grammar, spelling, and punctuation "
            "of the voice transcript below. Keep the exact meaning, wording, language "
            "(Indonesian or English), and technical terms. Do NOT answer the question, "
            "do NOT add labels, do NOT role-play, do NOT add explanations. Reply with "
            "ONLY the corrected transcript text, nothing else.\n\n"
            + text
        )
        polished = await asyncio.to_thread(free_agent.chat, prompt)
        polished = (polished or "").strip()
        if not _is_clean_polish(polished):
            return JSONResponse(content={"text": text})
        return JSONResponse(content={"text": polished})
    except Exception as e:
        print(f"[Cleanup] Error: {e}")
        return JSONResponse(content={"text": text})


# ── TTS helpers ──────────────────────────────────────────────────────────────
# (edge-tts removed — voice now runs on the on-device Kokoro engine only)


@app.post("/api/tts")
async def tts(req: TTSRequest):
    """Synthesize speech with the on-device Kokoro engine (free, CPU).

    Returns an audio/wav stream. Generated once per (voice, text) pair and
    cached so repeated replies are instant.
    """
    text = (req.text or "").strip()
    if not text:
        return JSONResponse(status_code=400, content={"error": "Empty text"})
    voice = req.voice or "am_michael"

    cache_dir = os.path.join(os.path.dirname(__file__), ".deep_agents", "tts_cache")
    os.makedirs(cache_dir, exist_ok=True)

    # Prune stale cache entries (older than 48h)
    try:
        now = time.time()
        for name in os.listdir(cache_dir):
            p = os.path.join(cache_dir, name)
            if os.path.isfile(p) and now - os.path.getmtime(p) > 48 * 3600:
                os.remove(p)
    except Exception:
        pass

    try:
        import audio_engine
        tts_key = (voice + '\x00' + text).encode('utf-8')
        out_path = os.path.join(cache_dir, f"kokoro_{hashlib.md5(tts_key).hexdigest()[:20]}.wav")
        if not os.path.exists(out_path):
            ok = await asyncio.to_thread(audio_engine.synthesize_tts, text, out_path, voice)
            if not ok or not os.path.exists(out_path) or os.path.getsize(out_path) == 0:
                return JSONResponse(status_code=502, content={"error": "Kokoro TTS generation failed"})
        return FileResponse(out_path, media_type="audio/wav")
    except Exception as e:
        print(f"[TTS] Kokoro engine unavailable: {e}")
        return JSONResponse(status_code=503, content={"error": f"Kokoro TTS unavailable: {e}"})


# ── Startup ─────────────────────────────────────────────────────────────────

if __name__ == "__main__":
    import uvicorn
    import sys
    
    reload_mode = "--reload" in sys.argv

    _start_task_scheduler()

    # Warm the embedding model in the background (the first semantic search
    # would otherwise stall the agent loop for seconds-minutes loading ONNX).
    def _warm_embeddings() -> None:
        try:
            from tools import _embedding_model_available
            if not _embedding_model_available():
                return
            from memory.embedding_service import embed
            embed("warmup")
            print("[API] Embedding model warmed.")
        except Exception as e:
            print(f"[API] Embedding warm-up skipped: {e}")

    threading.Thread(target=_warm_embeddings, daemon=True).start()
    
    # Start Telegram bot in background
    try:
        from bot import bot
        threading.Thread(target=bot.infinity_polling, daemon=True).start()
        print("[API] Telegram bot started in background.")
    except Exception as e:
        print(f"[API] Failed to start Telegram bot: {e}")
        
    if reload_mode:
        uvicorn.run("api_server:app", host="127.0.0.1", port=8000, reload=True)
    else:
        print("[API] Running in production mode (reload disabled).")
        uvicorn.run(app, host="127.0.0.1", port=8000)
