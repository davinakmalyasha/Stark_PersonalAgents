"""
Voice / TTS router — live-mode endpoints for the Electron voice overlay.

Phase 7: rebuilt from api_server.py after the package split. Serves:

  POST /api/voice/start            Begin server-side mic capture (Moonshine
                                   MicTranscriber streaming).
  GET  /api/voice/status           {active}
  POST /api/voice/stop             {discard:bool} → final transcript {text}
  POST /api/voice/mute             {muted:bool} — suppress self-transcription
  GET  /api/voice/peek             {text, level, active} — rolling transcript
  POST /api/voice/send             {} → final {text} (no re-transcribe; the
                                   streaming decoder already flushed it)
  POST /api/transcribe             File upload → Moonshine transcript
  POST /api/tts                    Full wav synthesis (Pocket TTS, cached)
  POST /api/tts/stream             SSE wav chunks (first chunk < 1s warm)

All heavy model calls run through asyncio.to_thread — the Pocket TTS / ASR
loops are CPU-bound and must never block the event loop.
"""
import asyncio
import base64
import hashlib
import io
import json
import os
import queue
import tempfile
import threading
import time
import uuid
from typing import AsyncGenerator

from fastapi import APIRouter, File, Form, Request, UploadFile
from fastapi.responses import FileResponse, JSONResponse, StreamingResponse
from pydantic import BaseModel

router = APIRouter(tags=["voice"])

_TTS_CACHE_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    ".deep_agents", "tts_cache",
)

# ── Serialized TTS worker ──────────────────────────────────────────────
# generate_audio_stream is NOT thread-safe (pocket_tts.models.tts_model.py
# docstring). Every /api/tts/stream request feeds one shared queue; a single
# worker drains it and synthesizes ONE utterance at a time — no concurrent
# model calls → no glitchy/chopped audio, no CPU contention spikes.
_TTS_WORKER_QUEUE: queue.Queue = queue.Queue()
_TTS_WORKER_LOCK = threading.Lock()
_TTS_WORKER_STARTED = False
_TTS_WORKER_DONE = object()  # sentinel for "utterance finished"

def _ensure_tts_worker() -> None:
    global _TTS_WORKER_STARTED
    with _TTS_WORKER_LOCK:
        if _TTS_WORKER_STARTED:
            return
        def _worker():
            while True:
                item = _TTS_WORKER_QUEUE.get()
                if item is None:
                    break
                text, voice, out_q = item
                try:
                    import audio_engine
                    for chunk in audio_engine.synthesize_tts_stream(text, voice):
                        out_q.put(chunk)
                except Exception as e:
                    out_q.put(e)
                finally:
                    out_q.put(_TTS_WORKER_DONE)
        threading.Thread(target=_worker, name="tts-worker", daemon=True).start()
        _TTS_WORKER_STARTED = True


class TTSRequest(BaseModel):
    text: str
    voice: str = "jarvis"
    engine: str = "pocket"  # pocket only (edge/kokoro/moonshine removed)


class VoiceSendRequest(BaseModel):
    pass


# ── Capture lifecycle ──────────────────────────────────────────────────────


@router.post("/api/voice/start")
async def voice_start():
    """Start server-side mic capture (Moonshine MicTranscriber, streaming)."""
    try:
        import audio_engine
        ok = await asyncio.to_thread(audio_engine.start_capture)
        return JSONResponse(content={"active": ok})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": f"Capture unavailable: {e}"})


@router.get("/api/voice/status")
async def voice_status():
    try:
        import audio_engine
        return JSONResponse(content={"active": audio_engine.capture_active()})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


@router.post("/api/voice/stop")
async def voice_stop(request: Request):
    """Stop capture and return the final transcript.

    Body: {"discard": true} stops without transcribing (mic turned off).
    The transcript is the streaming decoder's flushed output — zero re-STT.
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


@router.post("/api/voice/mute")
async def voice_mute(request: Request):
    """Mute the capture while the assistant speaks (prevents self-transcription)."""
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


@router.get("/api/voice/peek")
async def voice_peek():
    """Latest rolling transcript + live mic level (no model call — instant)."""
    try:
        import audio_engine
        return JSONResponse(content={
            "text": audio_engine.live_text(),
            "level": audio_engine.live_peak(),
            "active": audio_engine.capture_active(),
        })
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


@router.post("/api/voice/send")
async def voice_send(req: VoiceSendRequest):
    """Finalize the current round WITHOUT stopping capture.

    flush_round() returns the transcript accumulated since the last flush and
    clears the rolling buffers — the MicTranscriber keeps streaming, so the
    next round starts instantly (no ONNX reload, no OOM).
    """
    try:
        import audio_engine
        text = (await asyncio.to_thread(audio_engine.flush_round) or "").strip()
        if not text:
            return JSONResponse(status_code=422, content={"error": "Moonshine could not transcribe the audio (empty result)"})
        return JSONResponse(content={"text": text, "engine": "moonshine"})
    except Exception as e:
        print(f"[Voice] send failed: {e}")
        return JSONResponse(status_code=503, content={"error": f"Moonshine ASR unavailable: {e}"})


# ── STT (file fallback) ────────────────────────────────────────────────────


@router.post("/api/transcribe")
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


# ── TTS warm lifecycle (lazy: warm on live-mode open, free on close) ───────


@router.post("/api/voice/warm_tts")
async def voice_warm_tts():
    """Load the Pocket TTS model in the background (live mode opened).

    Idempotent — the model/voice state are cached, so repeated calls are
    no-ops once warm. The overlay polls /api/voice/tts_status for tts_warm.
    """
    try:
        import audio_engine
        if audio_engine.tts_loaded():
            return JSONResponse(content={"tts_warm": True, "tts_loading": False})
        def _warm():
            try:
                audio_engine.warm_up()
            except Exception as e:
                print(f"[Voice] warm_tts failed: {e}")
        threading.Thread(target=_warm, daemon=True).start()
        return JSONResponse(content={"tts_warm": False, "tts_loading": True})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": f"TTS warm unavailable: {e}"})


@router.post("/api/voice/release_tts")
async def voice_release_tts():
    """Free the Pocket TTS model (live mode closed). Idempotent."""
    try:
        import audio_engine
        await asyncio.to_thread(audio_engine.release_tts)
        return JSONResponse(content={"released": True})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


@router.get("/api/voice/tts_status")
async def voice_tts_status():
    """Report whether Pocket TTS is loaded (overlay polls while warming)."""
    try:
        import audio_engine
        return JSONResponse(content={"tts_warm": audio_engine.tts_loaded()})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


# ── TTS ────────────────────────────────────────────────────────────────────


def _tts_cache_path(voice: str, text: str) -> str:
    os.makedirs(_TTS_CACHE_DIR, exist_ok=True)
    # Prune stale entries (older than 48h)
    try:
        now = time.time()
        for name in os.listdir(_TTS_CACHE_DIR):
            p = os.path.join(_TTS_CACHE_DIR, name)
            if os.path.isfile(p) and now - os.path.getmtime(p) > 48 * 3600:
                os.remove(p)
    except Exception:
        pass
    key = (voice + "\x00" + text).encode("utf-8")
    return os.path.join(_TTS_CACHE_DIR, f"pocket_{hashlib.md5(key).hexdigest()[:20]}.wav")


@router.post("/api/tts")
async def tts(req: TTSRequest):
    """Synthesize speech with Pocket TTS (free, CPU, cached). Returns audio/wav."""
    text = (req.text or "").strip()
    if not text:
        return JSONResponse(status_code=400, content={"error": "Empty text"})
    voice = req.voice or "jarvis"

    try:
        import audio_engine
        out_path = _tts_cache_path(voice, text)
        if not os.path.exists(out_path) or os.path.getsize(out_path) == 0:
            ok = await asyncio.to_thread(audio_engine.synthesize_tts, text, out_path, voice)
            if not ok or not os.path.exists(out_path) or os.path.getsize(out_path) == 0:
                return JSONResponse(status_code=502, content={"error": "Pocket TTS generation failed"})
        return FileResponse(out_path, media_type="audio/wav")
    except Exception as e:
        print(f"[TTS] Pocket engine unavailable: {e}")
        return JSONResponse(status_code=503, content={"error": f"Pocket TTS unavailable: {e}"})


@router.post("/api/tts/stream")
async def tts_stream(req: TTSRequest):
    """Stream Pocket TTS chunks as SSE (first chunk typically < 1s warm).

    Each frame: data: {"chunk": "<base64 wav bytes>", "sr": 24000}
    Final:     data: {"done": true}

    True streaming: one serialized worker thread yields chunks as generated,
    consumer yields SSE frames immediately — TTFB ~200ms, no concurrency.
    """
    text = (req.text or "").strip()
    if not text:
        return JSONResponse(status_code=400, content={"error": "Empty text"})
    voice = req.voice or "jarvis"

    async def gen() -> AsyncGenerator[str, None]:
        import queue as _q

        q: _q.Queue = _q.Queue()

        _ensure_tts_worker()
        _TTS_WORKER_QUEUE.put((text, voice, q))

        try:
            import numpy as np

            def _wav_bytes(samples: np.ndarray, sr: int) -> bytes:
                import wave
                pcm = (np.clip(np.asarray(samples, dtype=np.float32), -1.0, 1.0) * 32767).astype(np.int16)
                buf = io.BytesIO()
                with wave.open(buf, "wb") as wf:
                    wf.setnchannels(1)
                    wf.setsampwidth(2)
                    wf.setframerate(int(sr))
                    wf.writeframes(pcm.tobytes())
                return buf.getvalue()

            while True:
                item = await asyncio.to_thread(q.get)
                if item is _TTS_WORKER_DONE:
                    break
                if isinstance(item, Exception):
                    raise item
                samples, sr = item
                if samples is None or len(samples) == 0:
                    continue
                b64 = base64.b64encode(_wav_bytes(samples, sr)).decode("ascii")
                yield f"data: {json.dumps({'chunk': b64, 'sr': int(sr)})}\n\n"
            yield f"data: {json.dumps({'done': True})}\n\n"
        except Exception as e:
            print(f"[TTS/stream] failed: {e}")
            yield f"data: {json.dumps({'error': str(e)})}\n\n"

    return StreamingResponse(gen(), media_type="text/event-stream")
