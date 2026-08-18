"""
Local voice engine — Moonshine (STT) + Kokoro (TTS).

Runs fully on-device (CPU), free, no API keys. Lazy-loads models and
never keeps ASR + TTS resident at the same time to keep RAM low
(~1 GB peak on a 4-core laptop).

Engines:
  - ASR: Moonshine Voice (medium-streaming, onnxruntime DLL)
  - TTS: Kokoro-82M v1.0 (torch CPU)

Voice cloning: drop `jarvis.wav` (+ `jarvis.txt` transcript) into
`.deep_agents/tts_ref/` and the JARVIS slot uses it. Until then the
closest male voice (`am_michael`) is used.
"""

import os
import queue
import subprocess
import threading
import tempfile
import time
from pathlib import Path

import numpy as np  # MUST be imported before any PortAudio callback thread runs

ROOT = Path(__file__).parent
MODELS_DIR = Path(os.environ.get("MOONSHINE_VOICE_CACHE", str(ROOT / ".deep_agents" / "models")))
HF_CACHE = ROOT / ".deep_agents" / "hf_cache"
TTS_REF_DIR = ROOT / ".deep_agents" / "tts_ref"
JARVIS_AUDIO = TTS_REF_DIR / "jarvis.wav"
JARVIS_TRANSCRIPT = TTS_REF_DIR / "jarvis.txt"
JARVIS_EMBEDDING = TTS_REF_DIR / "jarvis.pt"

# Moonshine medium-streaming model dir (downloaded via moonshine-voice)
ASR_MODEL_DIR = MODELS_DIR / "download.moonshine.ai" / "model" / "medium-streaming-en" / "quantized_26_07_30"

DEFAULT_TTS_VOICE = os.environ.get("TTS_VOICE", "am_michael")

os.environ.setdefault("HF_HOME", str(HF_CACHE))
os.environ.setdefault("MOONSHINE_VOICE_CACHE", str(MODELS_DIR))

_lock = threading.Lock()
_asr = None
_tts_model = None
_tts_pipe = None
_voice_cache: dict[str, object] = {}


# ── helpers ──────────────────────────────────────────────────────────────

def _ffmpeg_exe() -> str:
    """Locate a bundled ffmpeg (imageio_ffmpeg) so we never depend on PATH."""
    import imageio_ffmpeg
    return imageio_ffmpeg.get_ffmpeg_exe()


def audio_to_wav16k(src: str, dst: str) -> None:
    """Convert any audio to 16 kHz mono 16-bit PCM WAV via bundled ffmpeg.

    Loudness-normalized so quiet mic input is boosted (and screaming is
    kept in range) before it reaches the ASR model.
    """
    ff = _ffmpeg_exe()
    base = [ff, "-y", "-i", src, "-ar", "16000", "-ac", "1", "-sample_fmt", "s16"]
    # Adaptive loudness normalization; falls back to plain conversion if the
    # filter rejects a very short clip.
    try:
        subprocess.run(
            [*base, "-af", "loudnorm=I=-16:TP=-1.5:LRA=11", dst],
            check=True, capture_output=True, timeout=60,
        )
        return
    except Exception:
        subprocess.run([*base, dst], check=True, capture_output=True, timeout=60)


def _pad_wav(dst: str) -> None:
    """Prepend/append a little silence (helps short-clip ASR)."""
    try:
        import numpy as np
        import soundfile as sf
        audio, sr = sf.read(dst)
        pad = np.zeros(int(0.15 * sr), dtype=audio.dtype)
        audio = np.concatenate([pad, audio, pad])
        sf.write(dst, audio, sr)
    except Exception:
        pass


# ── ASR (Moonshine) ───────────────────────────────────────────────────────

def asr_available() -> bool:
    return ASR_MODEL_DIR.exists() and any(ASR_MODEL_DIR.glob("*.ort"))


def _load_asr():
    global _asr
    if _asr is not None:
        return _asr
    from moonshine_voice.transcriber import Transcriber
    from moonshine_voice.moonshine_api import ModelArch
    _asr = Transcriber(str(ASR_MODEL_DIR), model_arch=ModelArch.MEDIUM_STREAMING)
    return _asr


def _unload_asr():
    global _asr
    if _asr is not None:
        try:
            _asr.close()
        except Exception:
            pass
        _asr = None


def transcribe_file(audio_path: str) -> str:
    """Transcribe an audio file (any format) with Moonshine medium-streaming.

    Detailed logging helps diagnose mic-capture issues in the field.
    """
    global _asr, _tts_model, _tts_pipe
    with _lock:
        # Free TTS memory before loading ASR (never both resident)
        _tts_model = None
        _tts_pipe = None
        _voice_cache.clear()
        import gc
        gc.collect()
        if not asr_available():
            raise RuntimeError("Moonshine model not found — run the model download first")
        from moonshine_voice.utils import load_wav_file
        import numpy as np

        src_size = os.path.getsize(audio_path) if os.path.exists(audio_path) else 0
        print(f"[audio_engine] transcribe_file input={audio_path} bytes={src_size}")

        tr = _load_asr()
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            tmp_path = tmp.name
        try:
            audio_to_wav16k(audio_path, tmp_path)
            audio, sr = load_wav_file(tmp_path)
            peak = float(np.max(np.abs(audio))) if len(audio) else 0.0
            dur = len(audio) / sr if sr else 0.0
            print(f"[audio_engine] wav dur={dur:.2f}s peak={peak:.3f} samples={len(audio)}")
            result = tr.transcribe_without_streaming(audio, sample_rate=sr)
            text = " ".join(line.text for line in result.lines).strip()
            print(f"[audio_engine] ASR lines={len(result.lines)} text='{text[:120]}'")
            # Retry once with a boost + padding when the first pass came up empty
            if not text and dur > 0.2:
                print("[audio_engine] empty result — retrying with volume boost + padding")
                try:
                    subprocess.run(
                        [_ffmpeg_exe(), "-y", "-i", audio_path, "-af", "volume=18dB", "-ar", "16000", "-ac", "1", "-sample_fmt", "s16", tmp_path],
                        check=True, capture_output=True, timeout=60,
                    )
                    _pad_wav(tmp_path)
                    audio2, sr2 = load_wav_file(tmp_path)
                    result2 = tr.transcribe_without_streaming(audio2, sample_rate=sr2)
                    text2 = " ".join(line.text for line in result2.lines).strip()
                    print(f"[audio_engine] retry text='{text2[:120]}'")
                    if text2:
                        return text2
                except Exception as e:
                    print(f"[audio_engine] retry failed: {e}")
            return text
        finally:
            try:
                os.remove(tmp_path)
            except OSError:
                pass


# ── TTS (Kokoro) ──────────────────────────────────────────────────────────

def _load_tts():
    global _tts_model, _tts_pipe
    if _tts_model is not None and _tts_pipe is not None:
        return _tts_model, _tts_pipe
    from kokoro.model import KModel
    from kokoro.pipeline import KPipeline
    model = KModel(repo_id="hexgrad/Kokoro-82M")
    pipe = KPipeline(lang_code="a", model=model, device="cpu")
    _tts_model, _tts_pipe = model, pipe
    return model, pipe


def _unload_tts():
    global _tts_model, _tts_pipe
    _tts_model = None
    _tts_pipe = None
    _voice_cache.clear()


def _resolve_voice(voice: str):
    """Return the voice embedding for a Kokoro voice id.

    'jarvis' → cloned embedding when available (falls back to am_michael).
    """
    if voice == "jarvis":
        if JARVIS_EMBEDDING.exists():
            return str(JARVIS_EMBEDDING)
        voice = "am_michael"
    if voice not in _voice_cache:
        import torch
        if voice.endswith(".pt"):
            path = voice if Path(voice).exists() else str(TTS_REF_DIR / voice)
            _voice_cache[voice] = torch.load(path, weights_only=True)
        else:
            # Lazy-download from the model repo (voices/{voice}.pt)
            from huggingface_hub import hf_hub_download
            f = hf_hub_download(repo_id="hexgrad/Kokoro-82M", filename=f"voices/{voice}.pt")
            _voice_cache[voice] = torch.load(f, weights_only=True)
    return _voice_cache[voice]


def tts_available() -> bool:
    return True  # kokoro is a pip dependency; availability checked at call time


def synthesize_tts(text: str, out_wav_path: str, voice: str = DEFAULT_TTS_VOICE) -> bool:
    """Synthesize speech with Kokoro (CPU). Returns True on success."""
    global _asr
    with _lock:
        # Free ASR memory before loading TTS
        _asr = None
        try:
            import numpy as np
            import soundfile as sf
            _, pipe = _load_tts()
            voice_emb = _resolve_voice(voice)
            chunks = list(pipe(text, voice=voice_emb, speed=1.0))
            audio = np.concatenate([c.audio for c in chunks])
            sf.write(out_wav_path, audio, 24000)
            return True
        except Exception as e:
            print(f"[audio_engine] Kokoro TTS failed: {e}")
            return False


# ── convenience ───────────────────────────────────────────────────────────

def warm_up() -> None:
    """Preload the TTS pipeline (used by the voice overlay first-run)."""
    with _lock:
        try:
            _load_tts()
        except Exception as e:
            print(f"[audio_engine] warm_up failed: {e}")


# ── Server-side mic capture — OFFICIAL streaming path ─────────────────────
# Mirrors moonshine_voice.mic_transcriber.MicTranscriber (the code behind
# `moonshine-voice mic`): sounddevice → queue → worker thread →
# Stream.add_audio(). The C library resamples to 16kHz internally and keeps
# streaming state between chunks, so live updates are incremental (fast),
# not whole-buffer re-transcription.

CAPTURE_MAX_SECONDS = 120
_live_lock = threading.Lock()
_live_transcriber = None
_live_stream = None
_live_sd_stream = None
_live_queue: queue.Queue | None = None
_live_worker: threading.Thread | None = None
_live_watchdog: threading.Thread | None = None
_live_stop_sentinel = object()
_live_cond = threading.Condition()
_live_active = False
_live_muted = False  # MicTranscriber.mute(): drop audio while the assistant speaks
_live_watchdog_gen = 0
_live_rate = 16000
_live_peak = 0.0  # decaying mic level for the live meter
_live_peak_at = 0.0
_live_started_at = 0.0
_live_line_text: dict[int, str] = {}
_live_latest = ""


def _live_on_event(event) -> None:
    """Keep the latest rolling transcript from stream events."""
    line = getattr(event, "line", None)
    if line is None or not line.text:
        return
    with _live_cond:
        _live_line_text[line.line_id] = line.text
        _live_latest = " ".join(t for _, t in sorted(_live_line_text.items()))
        _live_cond.notify_all()


def _live_worker_loop() -> None:
    """Feed the stream in ~1s blocks — the exact pattern verified to work
    (file chunk test). Coalescing small PortAudio blocks into ~1s runs keeps
    the streaming model happy and lowers per-call overhead."""
    pending = []
    while True:
        item = _live_queue.get()
        if item is _live_stop_sentinel:
            if pending:
                data = pending[0] if len(pending) == 1 else np.concatenate(pending)
                try:
                    _live_stream.add_audio(data, _live_rate)
                except Exception as e:
                    print(f"[audio_engine] live add_audio error: {e}")
            break
        pending.append(item[0])
        if sum(len(p) for p in pending) >= 16000:
            data = pending[0] if len(pending) == 1 else np.concatenate(pending)
            pending = []
            try:
                _live_stream.add_audio(data, _live_rate)
            except Exception as e:
                print(f"[audio_engine] live add_audio error: {e}")


def _live_audio_callback(in_data, frames, time_info, status) -> None:
    global _live_peak, _live_peak_at
    if not _live_active or _live_muted or in_data is None:
        return
    try:
        x = np.asarray(in_data, dtype=np.float32).flatten()
        now = time.time()
        with _live_cond:
            if now - _live_peak_at > 0.15:  # decay meter
                _live_peak *= 0.75
            _live_peak = max(_live_peak, float(np.max(np.abs(x))))
            _live_peak_at = now
        _live_queue.put((x, _live_rate))
    except Exception:
        pass


def live_peak() -> float:
    with _live_cond:
        return _live_peak


def set_capture_muted(muted: bool) -> None:
    """Mute the capture while the assistant speaks (like MicTranscriber.mute)."""
    global _live_muted
    with _live_cond:
        _live_muted = bool(muted)


def _live_watchdog_loop(gen: int) -> None:
    try:
        time.sleep(CAPTURE_MAX_SECONDS)
        if gen == _live_watchdog_gen and _live_active:
            print("[audio_engine] capture watchdog — auto-stopping")
            stop_capture(discard=True)
    except Exception:
        pass


def _find_input_device() -> int | None:
    """Prefer the physical Realtek mic; never the Oculus virtual device."""
    try:
        import sounddevice as sd
        for i, info in enumerate(sd.query_devices()):
            name = info.get("name", "")
            if info.get("max_input_channels", 0) > 0 and "Microphone Array" in name:
                return i
    except Exception:
        pass
    return None


def start_capture() -> bool:
    """Begin live mic capture using the official Moonshine streaming path.

    Idempotent — safe to call repeatedly while already capturing.
    """
    global _live_transcriber, _live_stream, _live_sd_stream, _live_queue, _live_worker
    global _live_watchdog, _live_active, _live_started_at, _live_latest, _live_line_text
    global _live_rate, _live_watchdog_gen, _live_peak, _live_peak_at, _tts_model, _tts_pipe
    with _live_cond:
        if _live_active:
            return True
        try:
            import sounddevice as sd  # noqa: F401
        except Exception as e:
            print(f"[audio_engine] sounddevice unavailable: {e}")
            return False
        # ASR model (mutually exclusive with TTS to keep RAM low)
        with _lock:
            _tts_model = None
            _tts_pipe = None
            _voice_cache.clear()
            import gc
            gc.collect()
            if not asr_available():
                return False
            from moonshine_voice.transcriber import Transcriber
            from moonshine_voice.moonshine_api import ModelArch
            if _live_transcriber is None:
                _live_transcriber = Transcriber(str(ASR_MODEL_DIR), model_arch=ModelArch.MEDIUM_STREAMING)
        try:
            _live_stream = _live_transcriber.create_stream(update_interval=0.5)
            _live_stream.add_listener(_live_on_event)
            _live_stream.start()
            _live_queue = queue.Queue()
            _live_worker = threading.Thread(target=_live_worker_loop, daemon=True, name="moonshine-live")
            _live_worker.start()
            _live_rate = 16000
            _live_device = _find_input_device()
            try:
                _live_sd_stream = sd.InputStream(
                    samplerate=16000, channels=1, dtype="float32",
                    device=_live_device, callback=_live_audio_callback,
                )
            except Exception as e:
                # Some devices reject 16k; the C API resamples whatever we feed
                info = sd.query_devices(_live_device, "input")
                _live_rate = int(info["default_samplerate"])
                print(f"[audio_engine] 16k unsupported ({e}); using {_live_rate} Hz")
                _live_sd_stream = sd.InputStream(
                    samplerate=_live_rate, channels=1, dtype="float32",
                    device=_live_device, callback=_live_audio_callback,
                )
            _live_sd_stream.start()
            _live_active = True
            _live_started_at = time.time()
            _live_latest = ""
            _live_line_text = {}
            _live_peak = 0.0
            _live_peak_at = 0.0
            _live_watchdog_gen += 1
            _live_watchdog = threading.Thread(target=_live_watchdog_loop, args=(_live_watchdog_gen,), daemon=True)
            _live_watchdog.start()
            print("[audio_engine] live capture started (official streaming path)")
            return True
        except Exception as e:
            print(f"[audio_engine] live capture start failed: {e}")
            try:
                if _live_sd_stream is not None:
                    _live_sd_stream.close()
            except Exception:
                pass
            _live_sd_stream = None
            _live_active = False
            return False


def capture_active() -> bool:
    with _live_cond:
        return _live_active


def live_text() -> str:
    """Latest rolling transcript (no model call — instant)."""
    with _live_cond:
        return _live_latest


def stop_capture(discard: bool = False) -> str:
    """Stop the capture. Unless ``discard``, flush + return the final transcript."""
    global _live_active, _live_sd_stream, _live_worker, _live_stream, _live_queue, _live_watchdog
    with _live_cond:
        if not _live_active and _live_stream is None:
            return ""
        _live_active = False
    try:
        if _live_sd_stream is not None:
            _live_sd_stream.stop()
            _live_sd_stream.close()
    except Exception:
        pass
    _live_sd_stream = None
    if _live_worker is not None:
        try:
            _live_queue.put(_live_stop_sentinel)
            _live_worker.join(timeout=60)
        except Exception:
            pass
        _live_worker = None
    _live_watchdog = None
    if discard:
        try:
            if _live_stream is not None:
                _live_stream.stop()
        except Exception:
            pass
        _live_stream = None
        return ""
    text = ""
    if _live_stream is not None:
        try:
            result = _live_stream.stop()
            text = " ".join(line.text for line in result.lines).strip()
        except Exception as e:
            print(f"[audio_engine] live stop error: {e}")
        _live_stream = None
    print(f"[audio_engine] live capture transcript: '{text[:120]}'")
    return text
