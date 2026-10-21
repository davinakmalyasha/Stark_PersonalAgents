"""
Voice / TTS router — live-mode endpoints for the Electron voice overlay.

Phase 7: rebuilt from api_server.py after the package split. Serves:

  GET  /api/turn_detection         TurnDetection config (VAD portable)
  POST /api/voice/start            Begin server-side mic capture (Moonshine
                                   MicTranscriber streaming).
  GET  /api/voice/status           {active}
  POST /api/voice/stop             {discard:bool} → final transcript {text}
  POST /api/voice/mute             {muted:bool} — suppress self-transcription
  POST /api/voice/interrupt        Barge-in with truncation accounting
  GET  /api/voice/peek             {text, level, active} — rolling transcript
  POST /api/voice/send             {} → final {text} (no re-transcribe; the
                                   streaming decoder already flushed it)
  POST /api/transcribe             File upload → Moonshine transcript
  POST /api/tts                    Full wav synthesis (Pocket TTS, cached)
  POST /api/tts/stream             SSE wav chunks (first chunk < 1s warm)
  WS   /api/voice/ws               Live relay: transcript push + control +
                                   live-screen capture requests (registered in
                                   api_server.py as a raw Starlette route)

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

from server.services import screen_bridge

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
# Voice-clone state never leaves audio_engine: the worker only passes
# (text, voice-name) in and gets float32 PCM samples out, so cancelling or
# re-framing chunks cannot corrupt the safetensors voice tensors.
_TTS_WORKER_QUEUE: queue.Queue = queue.Queue()
_TTS_WORKER_LOCK = threading.Lock()
_TTS_WORKER_STARTED = False
_TTS_WORKER_DONE = object()  # sentinel for "utterance finished"
_TTS_ACTIVE_CANCEL: threading.Event | None = None
_TTS_ACTIVE_LOCK = threading.Lock()


def _cancel_current_tts() -> None:
    """Cancel the in-flight utterance + purge queued (not-yet-started) ones.

    Barge-in path: the Pocket model call itself is serial, so the current
    sentence-chunk finishes, then the generator sees the event set between
    chunks and aborts early. Queued sentences are dropped outright.
    Voice state is preserved (Pocket copies it per call when copy_state=True).
    """
    with _TTS_ACTIVE_LOCK:
        ev = _TTS_ACTIVE_CANCEL
        if ev is not None:
            try:
                ev.set()
            except Exception:
                pass
    # Purge pending items; each carries its own out_q which must still get DONE
    # so its SSE consumer terminates instead of hanging on q.get().
    while True:
        try:
            _item = _TTS_WORKER_QUEUE.get_nowait()
        except queue.Empty:
            break
        try:
            _out_q = _item[2] if len(_item) >= 3 else None
            if _out_q is not None:
                _out_q.put(_TTS_WORKER_DONE)
        except Exception:
            pass


def _ensure_tts_worker() -> None:
    global _TTS_WORKER_STARTED
    with _TTS_WORKER_LOCK:
        if _TTS_WORKER_STARTED:
            return

        def _worker():
            global _TTS_ACTIVE_CANCEL
            while True:
                item = _TTS_WORKER_QUEUE.get()
                if item is None:
                    break
                # Back-compat: old items are (text, voice, out_q).
                if len(item) == 3:
                    text, voice, out_q = item
                    cancel_ev: threading.Event | None = None
                else:
                    text, voice, out_q, cancel_ev = item
                if cancel_ev is None:
                    cancel_ev = threading.Event()
                with _TTS_ACTIVE_LOCK:
                    _TTS_ACTIVE_CANCEL = cancel_ev
                try:
                    # Pre-check: barge-in arrived while queued → skip synthesis.
                    if cancel_ev.is_set():
                        continue
                    import audio_engine
                    for chunk in audio_engine.synthesize_tts_stream(text, voice, cancel_ev):
                        if cancel_ev.is_set():
                            break
                        out_q.put(chunk)
                except Exception as e:
                    try:
                        out_q.put(e)
                    except Exception:
                        pass
                finally:
                    try:
                        out_q.put(_TTS_WORKER_DONE)
                    except Exception:
                        pass
                    with _TTS_ACTIVE_LOCK:
                        if _TTS_ACTIVE_CANCEL is cancel_ev:
                            _TTS_ACTIVE_CANCEL = None
        threading.Thread(target=_worker, name="tts-worker", daemon=True).start()
        _TTS_WORKER_STARTED = True


class TTSRequest(BaseModel):
    text: str
    voice: str = "jarvis"
    engine: str = "pocket"  # pocket only
    format: str = "wav"  # wav (compat) | pcm16 (raw int16 base64, no WAV header)


class VoiceSendRequest(BaseModel):
    audio: str = ""          # base64 wav (browser AudioWorklet raw PCM, 16k mono)
    audio_format: str = "wav"


# ── TurnDetection ──────────────────────────────────────────────────────


@router.get("/api/turn_detection")
async def turn_detection_config():
    """Return TurnDetection — single source of truth for VAD (mirrors pydantic TurnDetection)."""
    from server.api_server import TURN_DETECTION
    return JSONResponse(content=TURN_DETECTION)


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


# ── Barge-in with truncation accounting ────────────────────────────────


class VoiceInterruptRequest(BaseModel):
    played_ms: int = 0


@router.post("/api/voice/interrupt")
async def voice_interrupt(req: VoiceInterruptRequest):
    """Barge-in truncation — record interrupted_at_ms for history.

    Browser has already flushed local audio (stopSpeaking). Played_ms is how
    many ms were actually heard before the user talked over. Stored on the
    engine module so handoff to text can add an interruption note.

    Also cancels in-flight + queued TTS so the next utterance starts
    immediately instead of queueing behind the interrupted one.
    """
    try:
        import audio_engine
        played = max(int(getattr(req, "played_ms", 0) or 0), 0)
        audio_engine._last_interrupt_ms = played  # type: ignore[attr-defined]
        audio_engine._last_interrupt_at = __import__("time").time()  # type: ignore[attr-defined]
        try:
            _cancel_current_tts()
        except Exception:
            pass
        return JSONResponse(content={"interrupted_at_ms": played, "tts_cancelled": True})
    except Exception as e:
        return JSONResponse(status_code=503, content={"error": str(e)})


class TTSCancelRequest(BaseModel):
    gen_id: str = ""  # accepted for future per-utterance cancel; currently cancels all


@router.post("/api/tts/cancel")
async def tts_cancel(req: TTSCancelRequest):
    """Cancel in-flight + queued TTS (barge-in fast path, fire-and-forget)."""
    try:
        _cancel_current_tts()
        return JSONResponse(content={"cancelled": True})
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


@router.post("/api/voice/release_asr")
async def voice_release_asr():
    """Free the ASR fallback model (live mode closed). Idempotent."""
    try:
        import audio_engine
        await asyncio.to_thread(audio_engine.release_asr)
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


# ── WebSocket relay (browser AudioWorklet ↔ backend) ─────────────────
#
# NOTE: on purpose NOT declared with @router.websocket — FastAPI 0.141's
# websocket routing rejects upgrades with HTTP 403 in this environment
# (a bare Starlette WebSocketRoute works). api_server.py registers this
# handler as a raw Starlette route on the same path.


async def voice_ws(websocket):
    """Browser ↔ backend live relay — transcript push + control.

    Primary path (replaces 300ms HTTP peek polling when connected):
      server → client {type:"transcript", text, active} on change (~150ms tick)
      client → server {type:"send"} → {type:"final", text} (flush_round, no re-STT)
      client → server {type:"interrupt", played_ms} → cancels TTS + records ms
      client → server {type:"ping"} → {type:"pong"}
    Live screen sharing (voice overlay):
      server → client {type:"screen-capture", req_id} — capture a fresh frame
      server → client {type:"screen-mode", req_id, mode} — switch sharing mode
      client → server {type:"screen-ack", req_id, ok, mode?, error?} — resolve
      client → server {type:"screen-state", sharing, mode, source} — broadcast
    Binary PCM16 frames are acked (handshake compat; server STT stays on the
    server-side MicTranscriber device capture — no audio is reinterpreted).
    HTTP /api/voice/peek + /api/voice/send remain as automatic fallback.
    """
    await websocket.accept()
    stop_push = asyncio.Event()
    # Outbox for server-initiated control frames (screen_bridge runs in worker
    # threads); the push loop is the ONLY sender here, so nothing interleaves.
    wake = asyncio.Event()
    outbox: asyncio.Queue = asyncio.Queue()
    screen_bridge.attach(websocket, asyncio.get_running_loop(), outbox, wake)

    async def _push_loop():
        last = ""
        try:
            import audio_engine
            while not stop_push.is_set():
                while True:
                    try:
                        payload = outbox.get_nowait()
                    except asyncio.QueueEmpty:
                        break
                    try:
                        await websocket.send_text(payload)
                    except Exception:
                        return
                try:
                    text = audio_engine.live_text()
                    active = audio_engine.capture_active()
                except Exception:
                    text, active = "", False
                if text != last:
                    last = text
                    try:
                        await websocket.send_text(json.dumps({"type": "transcript", "text": text, "active": active}))
                    except Exception:
                        return
                try:
                    await asyncio.wait_for(wake.wait(), timeout=0.15)
                except asyncio.TimeoutError:
                    pass
                wake.clear()
        except Exception:
            pass

    push_task = asyncio.create_task(_push_loop())
    try:
        while True:
            msg = await websocket.receive()
            # Binary PCM16 frame -> ack (handshake compat, no reinterpretation).
            if "bytes" in msg and isinstance(msg["bytes"], (bytes, bytearray)):
                continue
            elif "text" in msg and msg["text"]:
                try:
                    data = json.loads(msg["text"])
                except Exception:
                    continue
                t = data.get("type")
                if t == "screen-ack":
                    try:
                        screen_bridge.resolve_ws_ack(data)
                    except Exception:
                        pass
                elif t == "screen-state":
                    try:
                        screen_bridge.update_state(data)
                    except Exception:
                        pass
                elif t == "interrupt":
                    import audio_engine  # type: ignore
                    pm = int((data.get("played_ms") or 0))
                    audio_engine._last_interrupt_ms = pm  # type: ignore[attr-defined]  # noqa: SLF001
                    audio_engine._last_interrupt_at = __import__("time").time()  # type: ignore[attr-defined]
                    try:
                        _cancel_current_tts()
                    except Exception:
                        pass
                    try:
                        await websocket.send_text(json.dumps({"type": "interrupted", "played_ms": pm}))
                    except Exception:
                        break
                elif t == "send":
                    try:
                        import audio_engine as _ae
                        text = (await asyncio.to_thread(_ae.flush_round) or "").strip()
                        await websocket.send_text(json.dumps({"type": "final", "text": text, "engine": "moonshine"}))
                    except Exception as e:
                        try:
                            await websocket.send_text(json.dumps({"type": "final", "text": "", "error": str(e)}))
                        except Exception:
                            break
                elif t == "cancel-tts":
                    try:
                        _cancel_current_tts()
                    except Exception:
                        pass
                elif t == "ping":
                    try:
                        await websocket.send_text(json.dumps({"type": "pong"}))
                    except Exception:
                        break
    except Exception:
        pass
    finally:
        try:
            screen_bridge.detach(websocket)
        except Exception:
            pass
        try:
            stop_push.set()
        except Exception:
            pass
        try:
            await asyncio.wait_for(asyncio.shield(push_task), timeout=1.0)
        except Exception:
            try:
                push_task.cancel()
            except Exception:
                pass
        try:
            await websocket.close()
        except Exception:
            pass


# ── TTS ────────────────────────────────────────────────────────────────────


_TTS_PRUNE_AT = 0.0


def _tts_cache_path(voice: str, text: str) -> str:
    global _TTS_PRUNE_AT
    os.makedirs(_TTS_CACHE_DIR, exist_ok=True)
    # Prune stale entries (older than 48h) at most once/hour and only on a
    # cache MISS path — never block a cache hit with os.listdir+stat.
    # The miss check happens below; this just computes the path here.
    key = (voice + "\x00" + text).encode("utf-8")
    return os.path.join(_TTS_CACHE_DIR, f"pocket_{hashlib.md5(key).hexdigest()[:20]}.wav")


def _tts_maybe_prune() -> None:
    global _TTS_PRUNE_AT
    now = time.time()
    if now - _TTS_PRUNE_AT < 3600:
        return
    _TTS_PRUNE_AT = now
    try:
        for name in os.listdir(_TTS_CACHE_DIR):
            p = os.path.join(_TTS_CACHE_DIR, name)
            try:
                if os.path.isfile(p) and now - os.path.getmtime(p) > 48 * 3600:
                    os.remove(p)
            except OSError:
                pass
    except Exception:
        pass


# Max text length cached on the /api/tts/stream path. Short ack sentences
# ("Sure.", "On it.") repeat verbatim and are exactly what benefits; long
# replies rarely repeat, so don't pay accumulation/memory for them.
_TTS_STREAM_CACHE_MAX_CHARS = 400


def _read_cache_frame(cache_path: str, want_pcm16: bool) -> str | None:
    """Render one SSE data frame from a cached wav. None on any problem.

    Fail-open by design: callers fall through to live synthesis.
    """
    import wave
    try:
        with wave.open(cache_path, "rb") as wf:
            if wf.getnchannels() != 1 or wf.getsampwidth() != 2:
                return None
            sr = int(wf.getframerate())
            raw = wf.readframes(int(wf.getnframes()))
        if not raw:
            return None
        if want_pcm16:
            b64 = base64.b64encode(raw).decode("ascii")
            return f"data: {json.dumps({'pcm16': b64, 'sr': sr, 'n': len(raw) // 2})}\n\n"
        with open(cache_path, "rb") as f:
            b64 = base64.b64encode(f.read()).decode("ascii")
        return f"data: {json.dumps({'chunk': b64, 'sr': sr})}\n\n"
    except Exception:
        return None


def _write_cache_wav(cache_path: str, samples, sr: int) -> None:
    """Persist streamed float32 samples as int16 mono wav (best-effort).

    Same on-disk format as /api/tts so both endpoints share entries.
    Writes to a temp file + os.replace so readers never see a partial wav.
    """
    import wave
    tmp = cache_path + f".tmp{os.getpid()}"
    try:
        import numpy as _np
        arr = _np.asarray(samples, dtype=_np.float32)
        pcm = (_np.clip(arr, -1.0, 1.0) * 32767).astype(_np.int16)
        with wave.open(tmp, "wb") as wf:
            wf.setnchannels(1)
            wf.setsampwidth(2)
            wf.setframerate(int(sr))
            wf.writeframes(pcm.tobytes())
        os.replace(tmp, cache_path)
    except Exception:
        try:
            if os.path.exists(tmp):
                os.remove(tmp)
        except OSError:
            pass


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
            _tts_maybe_prune()
            ok = await asyncio.to_thread(audio_engine.synthesize_tts, text, out_path, voice)
            if not ok or not os.path.exists(out_path) or os.path.getsize(out_path) == 0:
                return JSONResponse(status_code=502, content={"error": "Pocket TTS generation failed"})
        return FileResponse(out_path, media_type="audio/wav")
    except Exception as e:
        print(f"[TTS] Pocket engine unavailable: {e}")
        return JSONResponse(status_code=503, content={"error": f"Pocket TTS unavailable: {e}"})


@router.post("/api/tts/stream")
async def tts_stream(req: TTSRequest, request: Request):
    """Stream Pocket TTS chunks as SSE (first chunk typically < 1s warm).

    wav mode (compat):  data: {"chunk": "<base64 wav bytes>", "sr": 24000}
    pcm16 mode:         data: {"pcm16": "<base64 int16 mono>", "sr": 24000, "n": samples}
    Final:              data: {"done": true} (+ "cancelled": true when barged-in)

    True streaming: one serialized worker thread yields chunks as generated,
    consumer yields SSE frames immediately — TTFB ~200ms, no concurrency.
    pcm16 skips the per-chunk WAV header + decodeAudioData step: the browser
    builds an AudioBuffer straight from int16 (~33% smaller than wav-base64,
    no async decode per chunk). Voice tensors are unaffected — the worker
    still passes (text, voice-name) in and PCM samples out.
    """
    text = (req.text or "").strip()
    if not text:
        return JSONResponse(status_code=400, content={"error": "Empty text"})
    voice = req.voice or "jarvis"
    want_pcm16 = (getattr(req, "format", "") or "").strip().lower() in ("pcm16", "pcm", "raw")

    async def gen() -> AsyncGenerator[str, None]:
        import queue as _q

        q: _q.Queue = _q.Queue()
        cancel_ev = threading.Event()

        # Stream cache: repeated sentences skip synthesis entirely. Same
        # key/format as /api/tts so previews, fallbacks, and streams share
        # entries. Fail-open: any cache problem falls through to synthesis.
        cache_path = _tts_cache_path(voice, text)
        use_cache = len(text) <= _TTS_STREAM_CACHE_MAX_CHARS
        if use_cache and not cancel_ev.is_set():
            try:
                if os.path.exists(cache_path) and os.path.getsize(cache_path) > 0:
                    hit = await asyncio.to_thread(_read_cache_frame, cache_path, want_pcm16)
                    if hit is not None:
                        yield hit
                        yield f"data: {json.dumps({'done': True, 'cancelled': False})}\n\n"
                        return
                _tts_maybe_prune()
            except Exception:
                pass

        _ensure_tts_worker()
        _TTS_WORKER_QUEUE.put((text, voice, q, cancel_ev))

        # If the browser disconnects mid-stream (barge-in navigation away),
        # cancel synthesis so the worker slot frees for the next utterance.
        async def _watch_disconnect() -> None:
            try:
                if request is None:
                    return
                while True:
                    await asyncio.sleep(0.5)
                    try:
                        if await request.is_disconnected():
                            cancel_ev.set()
                            return
                    except Exception:
                        return
            except Exception:
                pass

        watcher = asyncio.create_task(_watch_disconnect())
        try:
            import numpy as np

            def _to_pcm16(samples: np.ndarray) -> bytes:
                pcm = (np.clip(np.asarray(samples, dtype=np.float32), -1.0, 1.0) * 32767).astype(np.int16)
                return pcm.tobytes()

            def _wav_bytes(samples: np.ndarray, sr: int) -> bytes:
                import wave
                buf = io.BytesIO()
                with wave.open(buf, "wb") as wf:
                    wf.setnchannels(1)
                    wf.setsampwidth(2)
                    wf.setframerate(int(sr))
                    wf.writeframes(_to_pcm16(samples))
                return buf.getvalue()

            # Coalesce tiny Pocket chunks (<~80ms) so the browser does fewer
            # AudioBuffer appends without adding audible latency.
            pending: np.ndarray | None = None
            pending_sr = 0
            # Raw chunks for the stream cache (written on clean completion).
            cache_parts: list = []
            cache_sr = 0

            def _flush_pending():
                nonlocal pending, pending_sr
                if pending is None or len(pending) == 0:
                    return None
                out = pending
                sr = pending_sr
                pending = None
                return out, sr

            async def _emit(samples, sr: int):
                import numpy as _np
                arr = _np.asarray(samples, dtype=_np.float32)
                if want_pcm16:
                    b64 = base64.b64encode(_to_pcm16(arr)).decode("ascii")
                    return f"data: {json.dumps({'pcm16': b64, 'sr': int(sr), 'n': int(arr.shape[0])})}\n\n"
                b64 = base64.b64encode(_wav_bytes(arr, sr)).decode("ascii")
                return f"data: {json.dumps({'chunk': b64, 'sr': int(sr)})}\n\n"

            while True:
                item = await asyncio.to_thread(q.get)
                if item is _TTS_WORKER_DONE:
                    break
                if isinstance(item, Exception):
                    # Flush coalesced audio first so partial speech isn't lost.
                    flushed = _flush_pending()
                    if flushed is not None:
                        yield await _emit(*flushed)
                    raise item
                samples, sr = item
                if samples is None or len(samples) == 0:
                    continue
                if cancel_ev.is_set():
                    break
                import numpy as _np2
                arr = _np2.asarray(samples, dtype=_np2.float32)
                if use_cache and not cancel_ev.is_set():
                    cache_parts.append(arr)
                    if not cache_sr:
                        cache_sr = int(sr)
                if pending is None:
                    pending, pending_sr = arr, int(sr)
                else:
                    import numpy as _np3
                    pending = _np3.concatenate([pending, arr])
                # Emit when we have ~80ms+ buffered (≈1920 samples @24kHz).
                if pending is not None and pending.shape[0] >= 1920:
                    flushed = _flush_pending()
                    if flushed is not None:
                        yield await _emit(*flushed)
            flushed = _flush_pending()
            if flushed is not None and not cancel_ev.is_set():
                yield await _emit(*flushed)
            # Persist short replies for instant replay next time. Skipped
            # when barged-in (cancelled) or errored — never cache partials.
            if use_cache and cache_parts and not cancel_ev.is_set():
                try:
                    import numpy as _npc
                    full = _npc.concatenate(cache_parts) if len(cache_parts) > 1 else cache_parts[0]
                    await asyncio.to_thread(_write_cache_wav, cache_path, full, cache_sr or 24000)
                except Exception:
                    pass
            yield f"data: {json.dumps({'done': True, 'cancelled': bool(cancel_ev.is_set())})}\n\n"
        except Exception as e:
            print(f"[TTS/stream] failed: {e}")
            try:
                yield f"data: {json.dumps({'error': str(e)})}\n\n"
            except Exception:
                pass
        finally:
            try:
                watcher.cancel()
            except Exception:
                pass

    return StreamingResponse(gen(), media_type="text/event-stream")
