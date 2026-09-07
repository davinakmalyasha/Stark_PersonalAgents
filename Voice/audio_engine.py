"""
Local voice engine — Moonshine Voice STT + Kyutai Pocket TTS.

STT:  Moonshine medium-streaming (on-device, streaming).
TTS:  Pocket TTS (kyutai-labs, 100M params, CPU, quantized int8) — fast
      (~1-6x realtime), ~200ms first audio chunk streamed, voice cloning
      from an audio file (zero-shot) or a precomputed .safetensors voice.

Voices:
  - jarvis : clones `.deep_agents/tts_ref/jarvis.mp3` (or loads the
             precomputed `jarvis_pocket.safetensors` embedding when it
             exists). Requires the gated kyutai/pocket-tts weights — set
             HF_TOKEN in .env and accept the model terms on HF first.
  - built-ins : alba, anna, vera, michael, charles, ... (catalog voices,
             work without auth).

STT is the same Moonshine stream (medium-streaming) as before.
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
# Repo root (one level above Voice/): tts_ref and models live under the
# repo's .deep_agents, not under Voice/.
REPO_ROOT = ROOT.parent
# moonshine-voice 0.1.5 downloads models into its own cache root
# (%LOCALAPPDATA%\moonshine_voice\... by default). Resolve it through the
# library so ASR + TTS share the real download location instead of a
# project-local dir that never gets populated.
try:
    from moonshine_voice.download import get_cache_dir
    _CACHE_ROOT = Path(get_cache_dir())
except Exception:
    _CACHE_ROOT = Path(os.environ.get("MOONSHINE_VOICE_CACHE", str(REPO_ROOT / ".deep_agents" / "models")))
MODELS_DIR = _CACHE_ROOT / "download.moonshine.ai"
TTS_REF_DIR = REPO_ROOT / ".deep_agents" / "tts_ref"
# Clean 7s clone source — user-provided clip, converted to 16-bit mono 24kHz WAV
# (Pocket sample_rate). Old noisy 27s trailer kept as jarvis_old_27s.mp3.
JARVIS_AUDIO = TTS_REF_DIR / "jarvis_clean.wav"
JARVIS_TRANSCRIPT = TTS_REF_DIR / "jarvis.txt"
# Precomputed Pocket TTS voice embedding for jarvis (kvcache safetensors).
# Exported once via export_model_state(); loads instantly at runtime.
JARVIS_POCKET_STATE = TTS_REF_DIR / "jarvis_pocket.safetensors"

# Moonshine streaming model dir. Resolve through the library so model
# version bumps (e.g. quantized_26_08_21) never hard-break us. ASR_MODEL_DIR
# is (re)computed at load time by _resolve_asr_dir().
ASR_MODEL_DIR: Path | None = None

# STT model size — tiny-streaming is the lightest streaming variant
# (~35MB vs medium ~350MB): faster load, less RAM/CPU, tiny WER tradeoff.
# Override with STT_MODEL=tiny|base|small|medium|*-streaming.
STT_MODEL_NAME = "tiny-streaming"


def _asr_arch():
    """Always tiny-streaming — no env override, no silent fallback to medium."""
    from moonshine_voice.moonshine_api import ModelArch
    return ModelArch.TINY_STREAMING

DEFAULT_TTS_VOICE = os.environ.get("TTS_VOICE", "jarvis" if (REPO_ROOT / ".deep_agents" / "tts_ref" / "jarvis_pocket.safetensors").exists() else "alba")

# Trim the natural tail silence Pocket leaves after EOS (default None ~0.3-0.5s).
# 1 frame ≈ 80ms; 2 keeps ~0.16s natural decay so sentence boundaries don't clip.
TTS_FRAMES_AFTER_EOS = int(os.environ.get("TTS_FRAMES_AFTER_EOS", "2"))

# Built-in Pocket TTS catalog voices (work without HF auth).
POCKET_CATALOG_VOICES = {
    "alba", "anna", "vera", "fantine", "michael", "charles", "paul",
    "george", "mary", "jane", "eve", "cosette", "marius", "javert",
    "jean", "eponine", "azelma", "bill_boerst", "peter_yearsley",
    "stuart_bell", "caro_davy", "giovanni", "lola", "juergen", "rafael",
    "estelle",
}

_lock = threading.Lock()          # legacy global (kept for anything else)
_tts_load_lock = threading.Lock()  # double-checked lock for model load (OOM guard)
_asr_lock = threading.Lock()      # Phase 8-V3: ASR-only lock — TTS no longer blocks STT
_tts_lock = threading.Lock()      # Phase 8-V3: TTS-only lock — full duplex (TTS speaks while STT listens)
_asr = None
_tts_model = None       # Pocket TTS TTSModel (resident)
_tts_voice_states: dict[str, object] = {}  # voice -> voice_state (kvcache)
_capture_start_lock = threading.Lock()  # serializes MicTranscriber load (OOM guard)


# ── helpers ──────────────────────────────────────────────────────────────

def _ffmpeg_exe() -> str:
    """Locate a bundled ffmpeg (imageio_ffmpeg) so we never depend on PATH."""
    import imageio_ffmpeg
    return imageio_ffmpeg.get_ffmpeg_exe()


def audio_to_wav16k(src: str, dst: str) -> None:
    """Convert any audio to 16 kHz mono 16-bit PCM WAV via bundled ffmpeg."""
    ff = _ffmpeg_exe()
    base = [ff, "-y", "-i", src, "-ar", "16000", "-ac", "1", "-sample_fmt", "s16"]
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
        import soundfile as sf
        audio, sr = sf.read(dst)
        pad = np.zeros(int(0.15 * sr), dtype=audio.dtype)
        audio = np.concatenate([pad, audio, pad])
        sf.write(dst, audio, sr)
    except Exception:
        pass


def _write_pcm_wav(samples: list, sample_rate: int, out_path: str) -> None:
    """Write float PCM samples (-1..1) to a 16-bit PCM wav file."""
    import wave
    pcm = (np.clip(np.asarray(samples, dtype=np.float32), -1.0, 1.0) * 32767).astype(np.int16)
    with wave.open(out_path, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(int(sample_rate))
        wf.writeframes(pcm.tobytes())


# ── ASR (Moonshine, streaming) ───────────────────────────────────────────

def _resolve_asr_dir(allow_download: bool = True) -> Path | None:
    """Resolve the current medium-streaming ASR model dir via the library.

    Returns None if unavailable. Downloads the model (best-effort) when
    missing and ``allow_download`` is true — the library keeps a versioned
    cache, so this stays correct across moonshine-voice upgrades.
    """
    global ASR_MODEL_DIR
    if ASR_MODEL_DIR is not None and any(Path(ASR_MODEL_DIR).glob("*.ort")):
        return Path(ASR_MODEL_DIR)
    try:
        from moonshine_voice import download
        from moonshine_voice.moonshine_api import ModelArch
        arch = _asr_arch()
        if allow_download:
            root, _arch = download.get_model_for_language("en", arch)
        else:
            info = download.find_model_info("en", arch)
            # download_url like https://download.moonshine.ai/model/medium-streaming-en/quantized_26_08_21
            url_path = str(info.get("download_url") or "")
            rel = url_path.split("download.moonshine.ai/", 1)[-1]
            root = Path(download.get_cache_dir()) / "download.moonshine.ai" / rel
        ASR_MODEL_DIR = Path(root)
        return ASR_MODEL_DIR
    except Exception as e:
        print(f"[audio_engine] ASR model resolve failed: {e}")
        return None


def asr_available() -> bool:
    d = _resolve_asr_dir(allow_download=False)
    return d is not None and any(d.glob("*.ort"))


def _load_asr():
    global _asr
    if _asr is not None:
        return _asr
    from moonshine_voice.transcriber import Transcriber
    from moonshine_voice.moonshine_api import ModelArch
    model_dir = _resolve_asr_dir(allow_download=True)
    if model_dir is None:
        raise RuntimeError("Moonshine ASR model unavailable (download failed)")
    _asr = Transcriber(str(model_dir), model_arch=_asr_arch())
    return _asr


def transcribe_file(audio_path: str) -> str:
    """Transcribe an audio file (any format) with Moonshine medium-streaming."""
    global _asr
    with _asr_lock:
        if not asr_available():
            raise RuntimeError("Moonshine model not found — run the model download first")
        from moonshine_voice.utils import load_wav_file

        src_size = os.path.getsize(audio_path) if os.path.exists(audio_path) else 0
        print(f"[audio_engine] transcribe_file input={audio_path} bytes={src_size}")

        tr = _load_asr()
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tmp:
            tmp_path = tmp.name
        try:
            audio_to_wav16k(audio_path, tmp_path)
            audio, sr = load_wav_file(tmp_path)
            dur = len(audio) / sr if sr else 0.0
            print(f"[audio_engine] wav dur={dur:.2f}s samples={len(audio)}")
            result = tr.transcribe_without_streaming(audio, sample_rate=sr)
            text = " ".join(line.text for line in result.lines).strip()
            print(f"[audio_engine] ASR lines={len(result.lines)} text='{text[:120]}'")
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


# ── TTS (Kyutai Pocket TTS) ────────────────────────────────────────────────

def tts_available() -> bool:
    try:
        import pocket_tts  # noqa: F401
        return True
    except Exception:
        return False


def _load_tts_model():
    """Load (once) the Pocket TTS model, quantized, kept resident.

    Double-checked locking: concurrent callers (warm_up + TTS) used to both
    see _tts_model is None and load the 1GB model twice → spike to 2GB+.
    """
    global _tts_model
    if _tts_model is not None:
        return _tts_model
    with _tts_load_lock:
        if _tts_model is not None:
            return _tts_model
        from pocket_tts import TTSModel
        _tts_model = TTSModel.load_model(quantize=True)
        print(f"[audio_engine] Pocket TTS loaded (device={_tts_model.device}, sr={_tts_model.sample_rate})")
        return _tts_model


def _voice_state(voice: str):
    """Return the Pocket TTS voice state for ``voice``, cached.

    - 'jarvis' → the precomputed safetensors embedding, else clones
      jarvis.mp3 (requires the gated cloning weights → HF_TOKEN + accepted
      model terms on huggingface.co/kyutai/pocket-tts).
    - built-in catalog name (alba, anna, ...) → that voice.
    - anything else → treated as a local audio file / safetensors path.
    """
    global _tts_voice_states
    key = str(voice or "").strip() or DEFAULT_TTS_VOICE
    if key in _tts_voice_states:
        return _tts_voice_states[key]
    model = _load_tts_model()
    try:
        if key == "jarvis":
            src = str(JARVIS_POCKET_STATE) if JARVIS_POCKET_STATE.exists() else str(JARVIS_AUDIO)
            if not os.path.exists(src):
                raise FileNotFoundError(f"jarvis voice source missing: {src}")
            state = model.get_state_for_audio_prompt(src)
            # Best-effort: export the kvcache embedding so future loads are
            # instant (and cloning weights are only needed once).
            if not JARVIS_POCKET_STATE.exists() and src == str(JARVIS_AUDIO):
                try:
                    from pocket_tts import export_model_state
                    export_model_state(state, str(JARVIS_POCKET_STATE))
                    print(f"[audio_engine] exported jarvis voice embedding -> {JARVIS_POCKET_STATE}")
                except Exception as e:
                    print(f"[audio_engine] jarvis embedding export failed (non-fatal): {e}")
        elif key in POCKET_CATALOG_VOICES:
            state = model.get_state_for_audio_prompt(key)
        else:
            # local file (wav/mp3/safetensors) or hf:// URL
            state = model.get_state_for_audio_prompt(key)
        _tts_voice_states[key] = state
        print(f"[audio_engine] voice state ready: {key}")
        return state
    except Exception as e:
        print(f"[audio_engine] voice state failed ({key}): {e}")
        raise


def synthesize_tts(text: str, out_wav_path: str, voice: str = DEFAULT_TTS_VOICE) -> bool:
    """Synthesize speech with Pocket TTS (CPU, quantized). Writes a wav."""
    with _tts_lock:
        try:
            model = _load_tts_model()
            state = _voice_state(voice)
            import torch
            audio = model.generate_audio(state, text, frames_after_eos=TTS_FRAMES_AFTER_EOS)
            samples = audio.detach().cpu().numpy()
            _write_pcm_wav(samples, int(model.sample_rate), out_wav_path)
            print(f"[audio_engine] Pocket TTS OK: {len(samples) / model.sample_rate:.2f}s -> {out_wav_path}")
            return True
        except Exception as e:
            print(f"[audio_engine] Pocket TTS failed: {e}")
            return False


def synthesize_tts_stream(text: str, voice: str = DEFAULT_TTS_VOICE):
    """Synthesize speech with Pocket TTS in streaming chunks.

    Yields ``(samples_float32_1d, sample_rate)`` chunks as they are produced
    — first chunk typically lands in well under a second.
    """
    with _tts_lock:
        model = _load_tts_model()
        state = _voice_state(voice)
        for chunk in model.generate_audio_stream(state, text, frames_after_eos=TTS_FRAMES_AFTER_EOS):
            yield chunk.detach().cpu().numpy(), int(model.sample_rate)


# ── convenience ───────────────────────────────────────────────────────────

def warm_up() -> None:
    """Preload the Pocket TTS model + voice state (live-mode open)."""
    with _tts_lock:
        try:
            _load_tts_model()
            try:
                _voice_state(DEFAULT_TTS_VOICE)
            except Exception as e:
                print(f"[audio_engine] warm_up voice {DEFAULT_TTS_VOICE} failed: {e}")
            print(f"[audio_engine] warm_up complete (voice={DEFAULT_TTS_VOICE})")
        except Exception as e:
            print(f"[audio_engine] warm_up failed: {e}")


def tts_loaded() -> bool:
    """True when the Pocket TTS model is resident (warm)."""
    return _tts_model is not None


def release_tts() -> None:
    """Drop the Pocket TTS model + voice states (live-mode close).

    Frees the ~1GB resident model when the overlay is closed so chat-only
    usage doesn't pay for TTS it never uses. Idempotent and safe to call
    repeatedly — the model reloads lazily on the next warm_up.
    """
    global _tts_model, _tts_voice_states
    with _tts_lock:
        _tts_model = None
        _tts_voice_states = {}
    try:
        import gc
        gc.collect()
    except Exception:
        pass


# ── Server-side mic capture — Moonshine MicTranscriber (streaming) ──────────
# This is the documented high-level type (SKILL.md: MicTranscriber.on_text/
# on_line, load().start()). It handles sample-rate resampling (44.1k/48k → 16k)
# and device fallback internally — the hand-rolled sounddevice → queue →
# Transcriber.create_stream path broke on 44.1k mics like yours.
#
# For the audio-first (MiMo) path the overlay sends `audio: wav` with the
# captured round's PCM (drained from _live_raw_pcm). For STT (fallback) the
# transcript is ready instantly at stop (streaming, no re-transcribe).

CAPTURE_MAX_SECONDS = 120
_live_mic = None                    # MicTranscriber (streaming input)
_live_watchdog: threading.Thread | None = None
_live_stop_sentinel = object()
_live_cond = threading.Condition()
_live_active = False
_live_muted = False                 # _live_mic.mute() wrapper
_live_watchdog_gen = 0
_live_watchdog_deadline = 0.0       # re-armed by every live read (peek)
_live_rate = 16000
_live_peak = 0.0
_live_peak_at = 0.0
_live_started_at = 0.0
_live_line_text: dict[int, str] = {}
_live_latest = ""
_live_raw_pcm: list = []            # float32 chunks for wav (audio-first)


def _live_touch() -> None:
    """Re-arm the watchdog deadline — called on every live read (peek/status)."""
    global _live_watchdog_deadline
    with _live_cond:
        if _live_active:
            _live_watchdog_deadline = time.time() + CAPTURE_MAX_SECONDS


def _live_on_text(text: str) -> None:
    """Rolling hypothesis — called while the user is still speaking."""
    global _live_latest
    if text is None:
        return
    with _live_cond:
        _live_latest = str(text or "").strip()
        _live_cond.notify_all()


def _live_on_line(line) -> None:
    """Finished line — called when a line boundary is detected."""
    text = getattr(line, "text", None)
    line_id = getattr(line, "line_id", None)
    if line_id is None:
        line_id = getattr(line, "id", None)
    if text is None or not str(text).strip():
        return
    with _live_cond:
        try:
            lid = int(line_id) if line_id is not None else len(_live_line_text)
        except Exception:
            lid = len(_live_line_text)
        _live_line_text[lid] = str(text).strip()
        _live_latest = " ".join(t for _, t in sorted(_live_line_text.items()))
        _live_cond.notify_all()



def set_capture_muted(muted: bool) -> None:
    """Mute the capture while the assistant speaks (prevents self-transcription)."""
    global _live_muted
    _live_muted = bool(muted)
    try:
        if _live_mic is not None:
            _live_mic.mute(bool(muted))
    except Exception:
        pass


def _live_watchdog_loop(gen: int) -> None:
    """Auto-stop capture only after CAPTURE_MAX_SECONDS of TOTAL silence.

    Re-armed by every live read (peek) so an active live session never dies
    mid-conversation — only a truly abandoned capture gets cleaned up.
    """
    try:
        with _live_cond:
            _live_watchdog_deadline = time.time() + CAPTURE_MAX_SECONDS
        while True:
            with _live_cond:
                if gen != _live_watchdog_gen or not _live_active:
                    return
                remaining = _live_watchdog_deadline - time.time()
            if remaining <= 0:
                print("[audio_engine] capture watchdog — auto-stopping (no live reads)")
                stop_capture(discard=True)
                return
            time.sleep(min(1.0, remaining))
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
    """Begin live mic capture using the documented MicTranscriber (streaming)."""
    global _live_mic
    global _live_watchdog, _live_active, _live_started_at, _live_latest, _live_line_text
    global _live_rate, _live_watchdog_gen, _live_peak, _live_peak_at, _live_raw_pcm
    global _live_watchdog_deadline
    with _live_cond:
        if _live_active:
            return True
        if not asr_available():
            print("[audio_engine] ASR model not available")
            return False
    try:
        from moonshine_voice import MicTranscriber
    except Exception as e:
        print(f"[audio_engine] MicTranscriber import failed: {e}")
        return False
    try:
        # Serialize the (expensive, ~350MB) transcriber load — overlapping
        # start_capture calls each allocate the ONNX sessions and blow memory.
        with _capture_start_lock:
            # Build the mic. on_text = rolling hypothesis, on_line = finished lines.
            mic = (MicTranscriber().language("en")
                   .on_text(_live_on_text).on_line(_live_on_line))
            # Device selection: prefer the real mic (see _find_input_device).
            dev = _find_input_device()
            if dev is not None:
                try:
                    mic.device(dev)
                except Exception:
                    pass
            mic.load()
            mic.start()
        with _live_cond:
            _live_mic = mic
            _live_active = True
            _live_started_at = time.time()
            _live_latest = ""
            _live_line_text = {}
            _live_peak = 0.0
            _live_peak_at = 0.0
            _live_raw_pcm = []
            _live_muted = False
            _live_watchdog_deadline = time.time() + CAPTURE_MAX_SECONDS
            _live_watchdog_gen += 1
            _live_watchdog = threading.Thread(target=_live_watchdog_loop, args=(_live_watchdog_gen,), daemon=True)
            _live_watchdog.start()
        print("[audio_engine] live capture started (MicTranscriber streaming)")
        return True
    except Exception as e:
        print(f"[audio_engine] live capture start failed: {e}")
        try:
            if '_live_mic' in dir() and _live_mic is not None:
                _live_mic.close()
        except Exception:
            pass
        with _live_cond:
            _live_mic = None  # type: ignore
            _live_active = False
        return False


def live_text() -> str:
    """Latest rolling transcript (no model call — instant)."""
    _live_touch()
    with _live_cond:
        return _live_latest


def live_peak() -> float:
    _live_touch()
    with _live_cond:
        return _live_peak


def capture_active() -> bool:
    _live_touch()
    with _live_cond:
        return _live_active


def flush_round() -> str:
    """Finalize the current speech round WITHOUT stopping the mic.

    Returns the transcript accumulated since the last flush, then clears the
    rolling buffers so the next round starts fresh. The MicTranscriber keeps
    running and keeps streaming — no reload, no re-allocation, no OOM.

    This replaces the old stop+restart round boundary (stop_capture → re-load
    the ONNX transcriber) that exhausted memory after a few rounds.
    """
    global _live_latest, _live_line_text, _live_raw_pcm, _live_peak, _live_peak_at
    with _live_cond:
        text = " ".join(t for _, t in sorted(_live_line_text.items())).strip()
        _live_line_text = {}
        _live_latest = ""
        _live_raw_pcm = []
        _live_peak = 0.0
        _live_peak_at = 0.0
    # Log every flush (including empty) so the mic → round → send chain is
    # visible in the API console; empty flushes show that VAD fired but Moonshine
    # heard nothing.
    print(f"[audio_engine] flush_round: '{text[:120]}'")
    return text


def _live_collect_pcm() -> list:
    """Return all captured float32 PCM chunks for the current round (audio-first).

    MicTranscriber doesn't expose its internal buffer, so this draws from the
    browser-side raw-PCM queue (filled by capture_wav_b64's upstream). Returns
    [] when nothing captured.
    """
    with _live_cond:
        return list(_live_raw_pcm)


def capture_wav_b64() -> str:
    """Return the captured audio as base64 wav (pure Python, no ffmpeg).

    For the audio-first MiMo path. Uses whatever raw PCM the current capture
    has; returns '' if the capture is not MicTranscriber (e.g. no mic).
    The raw queue is maintained by the browser-side AudioWorklet path
    (voice.html posts raw PCM bytes alongside /api/voice/send).
    """
    import base64
    import io
    import wave
    # If the server captured via MicTranscriber, _live_raw_pcm is empty
    # (MicTranscriber captures internally). In that case the browser must
    # supply the wav bytes via the /api/voice/send body — see api_server.
    with _live_cond:
        if not _live_raw_pcm:
            return ""
        pcm = list(_live_raw_pcm)
    data = np.concatenate(pcm) if len(pcm) > 1 else pcm[0]
    pcm16 = (np.clip(np.asarray(data, dtype=np.float32), -1.0, 1.0) * 32767).astype(np.int16)
    buf = io.BytesIO()
    with wave.open(buf, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(16000)
        wf.writeframes(pcm16.tobytes())
    return base64.b64encode(buf.getvalue()).decode("ascii")


def append_raw_pcm(samples_b64: str) -> None:
    """Append browser-side raw PCM (base64 float32 16k mono) to the queue.

    Called by /api/voice/append-raw so the server has audio for the MiMo path.
    """
    if not samples_b64:
        return
    try:
        import base64
        raw = base64.b64decode(samples_b64)
        arr = np.frombuffer(raw, dtype=np.float32)
        with _live_cond:
            _live_raw_pcm.append(arr)
    except Exception:
        pass


def stop_capture(discard: bool = False) -> str:
    """Stop the capture. Unless ``discard``, return the final transcript."""
    global _live_mic, _live_active, _live_watchdog
    with _live_cond:
        if not _live_active and _live_mic is None:
            return ""
        _live_active = False
        mic = _live_mic
        _live_mic = None  # prevent double-close
    _live_watchdog = None
    if discard:
        try:
            if mic is not None:
                mic.close()
        except Exception:
            pass
        with _live_cond:
            _live_raw_pcm.clear()
        return ""
    text = ""
    if mic is not None:
        try:
            mic.stop()
            # Give the streaming decoder a moment to flush its final line.
            time.sleep(0.4)
            with _live_cond:
                text = " ".join(t for _, t in sorted(_live_line_text.items())).strip()
            mic.close()
        except Exception as e:
            print(f"[audio_engine] live stop error: {e}")
            try:
                mic.close()
            except Exception:
                pass
    print(f"[audio_engine] live capture transcript: '{text[:120]}'")
    return text
