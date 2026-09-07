"""
Health / provider / models / history / state / session router â€”
Phase 7 moved from server/api_server.py (health:1548, provider:1499, models:1473,
history:1458, state:574, session/reset:1425).
"""
from fastapi import APIRouter, Request; from fastapi.responses import JSONResponse

router = APIRouter(tags=["health"])


@router.get("/api/health")
async def health():
    """Engine health â€” always up if the process is serving."""
    return JSONResponse(content={"ok": True, "code_version": "pocket-tts-stream-v1"})


@router.get("/api/health/voice")
async def voice_health():
    """Engine status â€” quick check that Moonshine (ASR) + Pocket TTS load."""
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
        import audio_engine
        tts_ok = audio_engine.tts_available()
    except Exception as e:
        tts_detail = f"unavailable: {e}"
    return JSONResponse(content={
        "interpreter": __import__("sys").executable,
        "asr": {"ok": asr_ok, "detail": asr_detail},
        "tts": {"ok": tts_ok, "detail": tts_detail},
    })


@router.get("/api/provider")
async def get_provider():
    from llm.llm import _resolve_llm_provider, _load_persisted_provider
    from llm.provider_modes import MODES, MODE_MATRIX, get_role_plan
    provider = _load_persisted_provider() or "deepseek"
    cfg = _resolve_llm_provider()
    return JSONResponse(content={
        "provider": provider,
        "modes": list(MODES),
        "matrix": {k: v for k, v in MODE_MATRIX.items()},
        "base_url": cfg.get("base", ""),
        "model_flash": cfg.get("flash", ""),
        "model_pro": cfg.get("pro", ""),
        "has_key": bool(cfg.get("key")),
        "role_plan": get_role_plan("Orchestrator"),
    })


@router.post("/api/provider")
async def set_provider(req: Request):
    from llm.llm import _save_persisted_provider, _resolve_llm_provider
    from llm.provider_modes import MODES
    try:
        body = await req.json()
    except Exception:
        return JSONResponse(content={"error": "invalid json"}, status_code=400)
    provider = (body.get("provider") or "").strip().lower()
    if provider not in MODES:
        return JSONResponse(content={"error": f"Unknown provider '{provider}'"}, status_code=400)
    _save_persisted_provider(provider)
    cfg = _resolve_llm_provider()
    return JSONResponse(content={"provider": provider, "base_url": cfg.get("base", "")})


@router.get("/api/models")
async def get_models():
    from llm.llm import _map_model
    from llm.llm_config import TASK_CATEGORIES
    models = []
    for cat, queue in TASK_CATEGORIES.items():
        for item in queue:
            m = _map_model(item.get("model", ""))
            if m and m not in models:
                models.append(m)
    return JSONResponse(content={"models": models})


@router.get("/api/history")
async def get_history(limit: int = 20):
    from memory.chat_memory_manager import load_chat_memory
    try:
        from sync.state_sync import safe_get_state
        chat_id = safe_get_state().get("chat_id", "")
        workspace_path = safe_get_state().get("project_path", "")
        msgs = load_chat_memory(workspace_path=workspace_path, chat_id=chat_id)
        prompts = [m.get("content", "") for m in msgs if m.get("role") == "user"][-limit:]
        return JSONResponse(content={"prompts": prompts})
    except Exception:
        return JSONResponse(content={"prompts": []})


@router.get("/api/state")
async def get_state():
    from sync.state_sync import safe_serialize_state
    return JSONResponse(content=__import__("json").loads(safe_serialize_state()))


@router.post("/api/session/reset")
async def reset_session():
    from sync.state_sync import reset_shared_state
    reset_shared_state()
    return JSONResponse(content={"ok": True})



