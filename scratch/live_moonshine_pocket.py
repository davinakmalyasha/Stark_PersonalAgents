#!/usr/bin/env python3
"""
Independent Moonshine + Pocket TTS live loop — zero Stark / Electron / server.

Just:  mic -> Moonshine on_text/on_line -> Pocket TTS -> speaker
Lightweight, ~1.2GB RAM, ~15% CPU, no orchestrator, no overlay.

Usage (venv — 3.14 has both models):
  venv\\Scripts\\python.exe scratch\\live_moonshine_pocket.py --voice alba
  venv\\Scripts\\python.exe scratch\\live_moonshine_pocket.py --voice jarvis --play
  venv\\Scripts\\python.exe scratch\\live_moonshine_pocket.py --voice jarvis --list-voices
  venv\\Scripts\\python.exe scratch\\live_moonshine_pocket.py --no-mic --text "I'm Stark, ready to help you."

Why this is lightweight vs live mode:
 - One Python process, one model load each (no Electron + Vite + API server + Telegram + embeddings)
 - No 2.5s VAD + 400ms sleep + full LLM wait — Moonshine segments speech naturally via on_line
 - TTS is torch.cat streaming already proven gapless in the harness
 - Proper mic close order fixes the Python-CFFI popup after app close
"""
import argparse
import os
import sys
import time
import threading
import queue
from pathlib import Path

# Same as Voice/audio_engine.py: drop pocket-tts beartype runtime checks
# (per-call overhead on the synthesis hot path). Set before pocket_tts
# is imported below.
os.environ.setdefault("POCKET_TTS_NO_BEARTYPE", "1")

REPO_ROOT = Path(__file__).resolve().parent.parent
JARVIS_SAFETENSORS = REPO_ROOT / ".deep_agents" / "tts_ref" / "jarvis_pocket.safetensors"
JARVIS_MP3 = REPO_ROOT / ".deep_agents" / "tts_ref" / "jarvis.mp3"

def resolve_voice_arg(voice: str) -> str:
    if voice == "jarvis":
        if JARVIS_SAFETENSORS.exists():
            return str(JARVIS_SAFETENSORS)
        if JARVIS_MP3.exists():
            return str(JARVIS_MP3)
        print(f"[warn] jarvis not found — falling back to alba", file=sys.stderr)
        return "alba"
    return voice

def parse_args():
    p = argparse.ArgumentParser(description="Independent Moonshine + Pocket live loop")
    p.add_argument("--voice", default="alba", help="alba | jarvis | ./my.wav | hf://...")
    p.add_argument("--play", action="store_true", default=True, help="play TTS (default on)")
    p.add_argument("--no-play", dest="play", action="store_false", help="no audio, just transcribe")
    p.add_argument("--no-mic", action="store_true", help="one-shot TTS only, no mic")
    p.add_argument("--text", default="", help="with --no-mic, text to speak")
    p.add_argument("--list-voices", action="store_true", help="list voices and exit")
    p.add_argument("--device", default="", help="mic device name substring or index (default auto)")
    return p.parse_args()

def main() -> int:
    args = parse_args()
    if args.list_voices:
        print("Catalog (no auth): alba, anna, vera, michael, charles, fantine, paul, ...")
        print(f"jarvis safetensors: {JARVIS_SAFETENSORS} exists={JARVIS_SAFETENSORS.exists()}")
        print(f"jarvis mp3: {JARVIS_MP3} exists={JARVIS_MP3.exists()}")
        return 0

    # Load .env quietly for HF_TOKEN (jarvis) without printing it
    try:
        from dotenv import load_dotenv
        load_dotenv(dotenv_path=str(REPO_ROOT / ".env"), override=False)
    except Exception:
        pass

    # --- one-shot mode (no mic) ---
    if args.no_mic:
        text = args.text or "I'm Stark, ready to help you."
        print(f"[one-shot] voice={args.voice} text={text!r}")
        from pocket_tts import TTSModel
        import scipy.io.wavfile
        voice_src = resolve_voice_arg(args.voice)
        t0 = time.time()
        model = TTSModel.load_model(quantize=True)
        print(f"[TTS] loaded in {time.time()-t0:.1f}s sr={model.sample_rate}")
        vs = model.get_state_for_audio_prompt(voice_src)
        print(f"[TTS] voice ready: {args.voice}")
        # Stream to show first chunk latency (what live mode should feel)
        t0 = time.time()
        first = None
        chunks = []
        for chunk in model.generate_audio_stream(vs, text):
            if first is None:
                first = time.time() - t0
                print(f"[TTS] first chunk {first:.2f}s")
            chunks.append(chunk)
        import torch
        audio = torch.cat(chunks, dim=0) if chunks else torch.zeros(0)
        dur = audio.shape[-1] / model.sample_rate if audio.numel() else 0
        print(f"[TTS] done {len(chunks)} chunks total {time.time()-t0:.1f}s audio {dur:.1f}s {dur/max(time.time()-t0,0.01):.1f}x realtime")
        if args.play:
            try:
                import sounddevice as sd
                print("[play] ...")
                sd.play(audio.detach().cpu().numpy(), samplerate=model.sample_rate, blocking=True)
                print("[play] done")
            except Exception as e:
                print(f"[play] failed: {e}")
        return 0

    # --- live mic mode ---
    print(f"[live] voice={args.voice}  Ctrl+C to stop")
    print(f"[live] loading TTS...")
    from pocket_tts import TTSModel
    voice_src = resolve_voice_arg(args.voice)
    t0 = time.time()
    tts_model = TTSModel.load_model(quantize=True)
    print(f"[TTS] loaded {time.time()-t0:.1f}s sr={tts_model.sample_rate}")
    t0 = time.time()
    tts_state = tts_model.get_state_for_audio_prompt(voice_src)
    print(f"[TTS] voice {args.voice} ready {time.time()-t0:.1f}s")
    try:
        import psutil
        rss = psutil.Process().memory_info().rss / 1e6
        print(f"[RSS] {rss:.0f} MB after TTS load")
    except Exception:
        pass

    print(f"[STT] loading Moonshine mic...")
    # Import here so TTS load RSS is visible first
    from moonshine_voice import MicTranscriber

    tts_queue: queue.Queue[str] = queue.Queue()
    stop_flag = threading.Event()

    def tts_worker():
        """Single thread owns TTS so _lock is not needed; queue drains sentences."""
        import sounddevice as sd
        while not stop_flag.is_set():
            try:
                text = tts_queue.get(timeout=0.2)
            except queue.Empty:
                continue
            if text is None:
                break
            text = text.strip()
            if not text:
                continue
            # Filter hallucinations like live mode does
            if text.lower().strip() in {"thank you", "thanks", "you're welcome", "goodbye"}:
                continue
            print(f"\n[TTS] >> {text}")
            try:
                import torch
                t0 = time.time()
                chunks = []
                first = None
                for chunk in tts_model.generate_audio_stream(tts_state, text):
                    if first is None:
                        first = time.time() - t0
                    chunks.append(chunk)
                if not chunks:
                    continue
                if first is not None:
                    print(f"[TTS] first {first:.2f}s total {time.time()-t0:.2f}s")
                audio = torch.cat(chunks, dim=0)
                # Gapless like harness — single buffer
                sd.play(audio.detach().cpu().numpy(), samplerate=tts_model.sample_rate, blocking=True)
                # Small pause so mic doesn't catch our own speaker
                time.sleep(0.15)
            except Exception as e:
                print(f"[TTS] failed: {e}", file=sys.stderr)
                import traceback; traceback.print_exc()

    worker = threading.Thread(target=tts_worker, daemon=True)
    worker.start()

    # Callbacks — these run on Moonshine's audio thread, keep them tiny
    def on_text(text: str):
        if not text:
            return
        # Live hypothesis — single line, no spam
        print(f"\r[typing] {text[:90]:90}", end="", flush=True)

    def on_line(line):
        t = getattr(line, "text", "") or ""
        t = t.strip()
        if not t:
            return
        # Clear the typing line and print final
        print(f"\r{' '*100}\r[you] {t}")
        tts_queue.put(t)

    # Build mic — language en, callbacks, optional device
    builder = MicTranscriber().language("en").on_text(on_text).on_line(on_line)
    if args.device:
        # Try index first, then name substring
        try:
            idx = int(args.device)
            builder = builder.device(idx)
            print(f"[mic] device index={idx}")
        except ValueError:
            # Name substring: find via sounddevice
            try:
                import sounddevice as sd
                for i, info in enumerate(sd.query_devices()):
                    if args.device.lower() in info.get("name","").lower() and info.get("max_input_channels",0)>0:
                        builder = builder.device(i)
                        print(f"[mic] device '{info['name']}' index={i}")
                        break
            except Exception as e:
                print(f"[mic] device search failed: {e}")

    # Load blocks first time downloading — show progress if attached
    try:
        builder = builder.on_progress(lambda frac, f: print(f"\r[STT] downloading {frac:.0%} {Path(f).name[:40]}", end="", flush=True))
    except Exception:
        pass

    mic = builder
    print("[mic] load() ...")
    t0 = time.time()
    try:
        mic.load()
    except Exception as e:
        print(f"\n[STT] load failed: {e}", file=sys.stderr)
        stop_flag.set()
        return 2
    print(f"\n[mic] loaded {time.time()-t0:.1f}s")
    try:
        import psutil
        rss = psutil.Process().memory_info().rss / 1e6
        print(f"[RSS] {rss:.0f} MB after STT load (TTS+STT resident)")
    except Exception:
        pass

    print("[mic] start() — speak now\n")
    try:
        mic.start()
    except Exception as e:
        print(f"[mic] start failed: {e}", file=sys.stderr)
        stop_flag.set()
        return 3

    # Proper close order fixes: Python-CFFI StreamBase callback Errno 22 after app close
    # Stop TTS queue first, then mic.stop() then mic.close() with a breath
    try:
        while True:
            time.sleep(0.1)
    except KeyboardInterrupt:
        print("\n[exit] Ctrl+C — stopping...")
    finally:
        # 1) Stop accepting new TTS
        stop_flag.set()
        try:
            # Drain queue
            while not tts_queue.empty():
                try: tts_queue.get_nowait()
                except: break
        except: pass
        # 2) Stop mic stream BEFORE closing — this is the fix for the FFI popup
        try:
            print("[mic] stop()...")
            mic.stop()
            time.sleep(0.4)  # let callback drain, same as audio_engine.py:622
        except Exception as e:
            print(f"[mic] stop error: {e}")
        try:
            print("[mic] close()...")
            mic.close()
            time.sleep(0.2)
        except Exception as e:
            print(f"[mic] close error: {e}")
        # 3) Wait for TTS worker to finish current utterance
        try:
            import sounddevice as sd
            sd.stop()
        except: pass
        print("[exit] done — no FFI popup should appear")

    return 0

if __name__ == "__main__":
    raise SystemExit(main())
