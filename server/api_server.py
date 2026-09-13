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
FastAPI server â€” Bridges the React frontend with the LangGraph multi-agent pipeline.
Replaces the Gradio dashboard with SSE streaming endpoints.
"""
import os
import sys

#Interpreter guard”
# Single venv (Python 3.14) is the primary interpreter. VOICE_PYTHON env var
# overrides for legacy Moonshine voice (venv312).
# Order: VOICE_PYTHON > venv (3.14) > .venv > venv312 (legacy fallback).
def _resolve_preferred_python() -> str | None:
    for candidate in (
        os.environ.get("VOICE_PYTHON", "").strip(),
        os.path.join(_ROOT, "venv", "Scripts", "python.exe"),
        os.path.join(_ROOT, ".venv", "Scripts", "python.exe"),
        os.path.join(_ROOT, "venv312", "Scripts", "python.exe"),
    ):
        if candidate and os.path.exists(candidate):
            return candidate
    return None

def _ensure_venv() -> None:
    try:
        preferred = _resolve_preferred_python()
        if not preferred:
            print("[api_server] WARNING: no venv found â€” running with system interpreter")
            return
        pref_dir = os.path.normpath(os.path.dirname(os.path.dirname(preferred))).lower()
        cur_prefix = os.path.normpath(sys.prefix).lower()
        cur_exec = os.path.normpath(sys.executable).lower()
        # Already running from preferred venv?
        if cur_prefix.startswith(pref_dir) or cur_exec == os.path.normpath(preferred).lower():
            return
        # Only re-exec if preferred is venv (3.14) and we are not already in it.
        # This makes venv/3.14 the default, venv312 only via VOICE_PYTHON.
        if "venv312" in preferred.lower() and os.path.exists(os.path.join(_ROOT, "venv", "Scripts", "python.exe")):
            # Don't auto-downgrade to 3.12 when 3.14 venv exists unless explicitly requested
            if not os.environ.get("VOICE_PYTHON"):
                return
        print(f"[api_server] Re-executing with {preferred} (current: {sys.executable})")
        os.execv(preferred, [preferred] + sys.argv)
    except Exception as e:
        print(f"[api_server] Interpreter guard failed ({e}) â€” continuing")

_ensure_venv()

# Voice module path (audio_engine, voice_services moved to Voice/)
_VOICE_DIR = os.path.join(_ROOT, "Voice")
if _VOICE_DIR not in sys.path:
    sys.path.insert(0, _VOICE_DIR)

_DESKTOP_DIR = os.path.join(_ROOT, "Desktop")
if _DESKTOP_DIR not in sys.path:
    sys.path.insert(0, _DESKTOP_DIR)

_TELEGRAM_DIR = os.path.join(_ROOT, "Telegram")
if os.path.isdir(_TELEGRAM_DIR) and _TELEGRAM_DIR not in sys.path:
    sys.path.insert(0, _TELEGRAM_DIR)

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

# ── Unique port (8123) — avoids clashes with dev servers (8000/5173/etc) ──
STARK_PORT = int(os.environ.get("STARK_PORT", "8123"))

# Force tiny-streaming STT only (no env fallback to medium — user requested).
os.environ["STT_MODEL"] = "tiny-streaming"
# Reduce OMP/ONNX thread pressure on the live path (lower burst CPU).
os.environ.setdefault("OMP_NUM_THREADS", "2")
os.environ.setdefault("ORT_NUM_THREADS", "2")

# ── TurnDetection (portable VAD config — replaces hardcoded voice.html:827) ──
# Tunable without redeploy; mirrors pydantic_ai TurnDetection(sensitivity).
TURN_DETECTION = {
    "sensitivity": os.environ.get("TURN_SENSITIVITY", "medium"),
    "prefix_padding_ms": int(os.environ.get("TURN_PREFIX_PADDING_MS", "300")),
    "silence_duration_ms": int(os.environ.get("TURN_SILENCE_MS", "300")),
    "sound_level": float(os.environ.get("TURN_SOUND_LEVEL", "0.01")),
    "min_round_ms": int(os.environ.get("TURN_MIN_ROUND_MS", "250")),
    "max_round_ms": int(os.environ.get("TURN_MAX_ROUND_MS", "30000")),
}

# ── Single-instance guard ──
# If another api_server is already listening on the configured port, this
# process exits QUIETLY (no restart storm, no double model warm-up).
def _port_in_use(port: int) -> bool:
    import socket
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as s:
        try:
            s.bind(("127.0.0.1", port))
            return False
        except OSError:
            return True

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

# ── Graceful shutdown: stop mic capture + free PortAudio (fixes CFFI popup + zombie RAM)
import atexit

def _shutdown_voice():
    try:
        import audio_engine
        try:
            audio_engine.stop_capture(discard=True)
        except Exception:
            pass
        print("[shutdown] voice capture stopped")
    except Exception:
        pass

atexit.register(_shutdown_voice)

from contextlib import asynccontextmanager

@asynccontextmanager
async def _lifespan(app):
    yield
    _shutdown_voice()

# Reassign app with lifespan (FastAPI accepts lifespan=; must be set before
# routers are included — this runs _shutdown_voice on uvicorn shutdown too).
app = FastAPI(title="Deep Agents API", version="2.0", lifespan=_lifespan)

# ── Phase 7: domain routers ──
from server.house.router import router as house_router
app.include_router(house_router)
from server.realtime.router import router as realtime_router
app.include_router(realtime_router)
from server.routers.health import router as health_router
app.include_router(health_router)
from server.routers.ui import router as ui_router
app.include_router(ui_router)
from server.routers.mcp import router as mcp_router
app.include_router(mcp_router)
from server.routers.agentsadmin import router as agentsadmin_router
app.include_router(agentsadmin_router)
from server.routers.ask import router as ask_router
app.include_router(ask_router)
from server.routers.snapshots import router as snapshots_router
app.include_router(snapshots_router)
from server.routers.workspaces import router as workspaces_router
app.include_router(workspaces_router)
from server.routers.chats import router as chats_router
app.include_router(chats_router)
from server.routers.tasks import router as tasks_router
app.include_router(tasks_router)
from server.routers.runs import router as runs_router
app.include_router(runs_router)
from server.routers.voice import router as voice_router
app.include_router(voice_router)
# Live voice socket: raw Starlette route because FastAPI 0.141's
# @router.websocket path rejects upgrades with HTTP 403 in this environment
# (bare Starlette WebSocketRoute works). Inserted first so the broken lazy
# router never claims the websocket scope.
from starlette.routing import WebSocketRoute as _WebSocketRoute
from server.routers.voice import voice_ws as _voice_ws
app.router.routes.insert(0, _WebSocketRoute("/api/voice/ws", _voice_ws))
from server.routers.screen import router as screen_router
app.include_router(screen_router)

# â”€â”€ Remote access guard (mobile/remote feature) â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
# Loopback clients (the desktop app on this machine) pass freely. Any other
# client â€” e.g. the phone over Tailscale â€” must present the pairing token.
@app.middleware("http")
async def remote_token_guard(request: Request, call_next):
    from remote_auth import is_loopback, get_pairing_token
    if is_loopback(request):
        return await call_next(request)
    path = request.url.path
    token = request.headers.get("x-api-token", "")
    if token and token == get_pairing_token():
        return await call_next(request)
    print(f"[AUTH] Rejected unpaired remote request: {path} from {request.client.host if request.client else '?'}")
    return JSONResponse(status_code=401, content={"error": "Pairing required"})


@app.get("/api/pairing")
async def pairing_info(request: Request):
    """Hand out the pairing token â€” loopback callers only (desktop Settings)."""
    from remote_auth import is_loopback, get_pairing_token
    if not is_loopback(request):
        return JSONResponse(status_code=403, content={"error": "Local only"})
    return {"token": get_pairing_token()}


@app.post("/api/pairing/rotate")
async def pairing_rotate(request: Request):
    """Invalidate the current token and issue a fresh one (local only)."""
    from remote_auth import is_loopback, reset_pairing_token
    if not is_loopback(request):
        return JSONResponse(status_code=403, content={"error": "Local only"})
    return {"token": reset_pairing_token()}


#”Models”

class RunRequest(BaseModel):
    prompt: str
    model: str = "Automatic Fallback"
    temperature: float = 0.7
    workspace_path: str = ""
    chat_id: str = ""
    attachments: list[dict] = []
    workspace_kind: str = ""  # hint: "" | "house" | "personal" — unified router uses this to select House vs General vs Orchestrator

class ChatRequest(BaseModel):
    prompt: str


class TTSRequest(BaseModel):
    text: str
    voice: str = "jarvis"
    engine: str = "pocket"  # pocket only


class CleanupRequest(BaseModel):
    text: str


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


# Static frontend serving (mobile/remote feature)
# Serves the production Vite build from frontend/dist on the same origin as the
# API, so the phone only needs http://<laptop>:8000. Registered LAST: every
# /api/* route above matches first; anything else falls back to the SPA shell.
# â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•â•

_FRONTEND_DIST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "frontend", "dist")

if os.path.isdir(_FRONTEND_DIST):
    _assets_dir = os.path.join(_FRONTEND_DIST, "assets")
    if os.path.isdir(_assets_dir):
        from fastapi.staticfiles import StaticFiles
        app.mount("/assets", StaticFiles(directory=_assets_dir), name="spa-assets")

    @app.get("/{full_path:path}", include_in_schema=False)
    async def spa_fallback(full_path: str):
        """Serve real static files (favicon/manifest/icons); index.html otherwise."""
        candidate = os.path.realpath(os.path.join(_FRONTEND_DIST, full_path))
        dist_root = os.path.realpath(_FRONTEND_DIST)
        if full_path and candidate.startswith(dist_root + os.sep) and os.path.isfile(candidate):
            return FileResponse(candidate)
        index_path = os.path.join(_FRONTEND_DIST, "index.html")
        if os.path.isfile(index_path):
            return FileResponse(index_path)
        return JSONResponse(status_code=404, content={"error": "Frontend build not found (run npm run build)"})
else:
    print("[STARTUP] frontend/dist not found â€” UI serving disabled (Electron/dev mode unaffected)")


# â”€â”€ Startup â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

if __name__ == "__main__":
    import uvicorn
    import sys

    # Logfire observability (no-op until LOGFIRE_ENABLED=1 + token set).
    try:
        from llm.observability import instrument_logfire
        if instrument_logfire():
            print("[STARTUP] Logfire instrumentation enabled.")
    except Exception:
        pass

    reload_mode = "--reload" in sys.argv

    # Single-instance guard: if port already held, exit quietly (no storm, no
    # duplicate model warm-up). Check BEFORE starting warm threads.
    if _port_in_use(STARK_PORT):
        print(f"[API] Port {STARK_PORT} already in use — another api_server is running. Exiting quietly.")
        sys.exit(0)

    from server.services.task_scheduler import start_task_scheduler as _start_task_scheduler
    _start_task_scheduler()

    # NOTE: The embedding model (fastembed bge-large ~1.3GB) is deliberately
    # NOT warmed at boot - it is lazy-loaded on first embed() call in
    # memory/embedding_service.py, gated on _embedding_model_warm().
    # Keeping it cold frees ~1.3GB of resident RAM for the harness.

    # NOTE: The Pocket TTS model is deliberately NOT warmed at boot — it is
    # lazy-loaded when live mode opens (POST /api/voice/warm_tts) and freed
    # when it closes (POST /api/voice/release_tts), so chat-only usage never
    # pays for ~1GB of resident TTS. The overlay shows a loading state while
    # it warms (~10s: torch/onnxruntime imports + model + voice state).

    # Warm the orchestrator LLM client in the background so the first voice
    # turn doesn't pay the 50-90s cold-call (mimo first request spins up the
    # provider connection). A trivial invoke primes the HTTP pool.
    def _warm_llm() -> None:
        try:
            from llm.pai_transport import pai_native_turn
            from llm.provider_modes import MODE_MATRIX
            orch = MODE_MATRIX.get("command", {}).get("orchestrator") or {}
            model = orch.get("model") if isinstance(orch, dict) else "xiaomi/mimo-v2.5"
            pai_native_turn(
                role="Orchestrator",
                messages=[{"role": "user", "content": "Reply with the single word: ready"}],
                model=model,
                temp=0.3,
                stream_emit=False,
            )
            print(f"[API] Orchestrator LLM warmed ({model}).")
        except Exception as e:
            print(f"[API] Orchestrator LLM warm-up skipped: {e}")

    threading.Thread(target=_warm_llm, daemon=True).start()
    
    # Start Telegram bot in background (text-only; no voice sessions touch it).
    # Opt out with START_TELEGRAM=0 for the leanest voice-only footprint —
    # one less polling thread competing with STT/TTS for CPU wakeups.
    if os.environ.get("START_TELEGRAM", "1").lower() in ("0", "false", "no", "off"):
        print("[API] Telegram bot disabled (START_TELEGRAM=0).")
    else:
        try:
            from bot import bot
            threading.Thread(target=bot.infinity_polling, daemon=True).start()
            print("[API] Telegram bot started in background.")
        except Exception as e:
            print(f"[API] Failed to start Telegram bot: {e}")
        
    if reload_mode:
        # Import-string form (required for reload): resolve as a package module
        # since the file lives in server/, and Electron's cwd is the repo root.
        uvicorn.run("server.api_server:app", host="127.0.0.1", port=STARK_PORT, reload=True)
    else:
        bind_host = os.environ.get("STARK_BIND", "127.0.0.1")
        bind_port = int(os.environ.get("STARK_PORT", str(STARK_PORT)))
        if bind_host == "0.0.0.0":
            print("[API] Remote mode: listening on all interfaces â€” pairing token required for non-local clients.")
        print(f"[API] Running in production mode (reload disabled) on port {bind_port}.")
        uvicorn.run(app, host=bind_host, port=bind_port)




