#!/usr/bin/env python3
"""
Pocket TTS standalone tester — zero Stark imports.

Run outside the Stark harness to A/B any voice quickly.

  python scratch/test_pocket_standalone.py --voice jarvis --text "Loud and clear, Sir." --stream
  python scratch/test_pocket_standalone.py --voice alba --text "Hello world" --play
  python scratch/test_pocket_standalone.py --voice ./my_clean.wav --text "Test" --stream
  python scratch/test_pocket_standalone.py --list-voices
  python scratch/test_pocket_standalone.py --voice jarvis --text "Hi" --no-quantize

Requires: pocket-tts, torch, scipy, sounddevice (optional for --play).
Python 3.10+ / torch 2.5+ / pocket-tts 3.x
"""
import argparse
import os
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
JARVIS_SAFETENSORS = REPO_ROOT / ".deep_agents" / "tts_ref" / "jarvis_pocket.safetensors"
JARVIS_WAV = REPO_ROOT / ".deep_agents" / "tts_ref" / "jarvis_clean.wav"  # clean 7s reference
JARVIS_MP3 = REPO_ROOT / ".deep_agents" / "tts_ref" / "jarvis.mp3"
TMP_DIR = Path(os.environ.get("TEMP", str(REPO_ROOT / ".tmp")))
TMP_DIR.mkdir(parents=True, exist_ok=True)

CATALOG_VOICES = [
    "alba", "anna", "vera", "fantine", "michael", "charles", "paul",
    "george", "mary", "jane", "eve", "cosette", "marius", "javert",
    "jean", "eponine", "azelma", "bill_boerst", "peter_yearsley",
    "stuart_bell", "caro_davy", "giovanni", "lola", "juergen", "rafael", "estelle",
]


def resolve_voice_arg(voice: str) -> str:
    """Map 'jarvis' to the clean 7s reference (safetensors when exists, else wav, else mp3)."""
    if voice == "jarvis":
        if JARVIS_SAFETENSORS.exists():
            return str(JARVIS_SAFETENSORS)
        if JARVIS_WAV.exists():
            return str(JARVIS_WAV)
        if JARVIS_MP3.exists():
            return str(JARVIS_MP3)
        print(f"[warn] jarvis voice requested but none of the references exist — falling back to alba", file=sys.stderr)
        return "alba"
    return voice


def main() -> int:
    p = argparse.ArgumentParser(description="Pocket TTS standalone tester")
    p.add_argument("--voice", default="alba", help="voice: jarvis | alba | anna | ... | ./my.wav | hf://... | ./my.safetensors")
    p.add_argument("--text", default="Loud and clear, Sir. This is Stark, fully operational and at your service.", help="text to synthesize")
    p.add_argument("--stream", action="store_true", help="use generate_audio_stream (first chunk timing)")
    p.add_argument("--play", action="store_true", help="play the result via sounddevice")
    p.add_argument("--no-quantize", action="store_true", help="disable int8 quantize (fp32)")
    p.add_argument("--list-voices", action="store_true", help="list catalog voices and exit")
    p.add_argument("--out", default="", help="output wav path (default: temp dir)")
    args = p.parse_args()

    if args.list_voices:
        print("Catalog voices (no auth):")
        for v in CATALOG_VOICES:
            print(f"  {v}")
        print("\nLocal jarvis:")
        print(f"  {JARVIS_SAFETENSORS} ({'exists' if JARVIS_SAFETENSORS.exists() else 'missing'})")
        print(f"  {JARVIS_WAV} ({'exists' if JARVIS_WAV.exists() else 'missing'})")
        print(f"  {JARVIS_MP3} ({'exists' if JARVIS_MP3.exists() else 'missing'})")
        return 0

    # load .env for HF_TOKEN if present (without printing it)
    try:
        from dotenv import load_dotenv
        load_dotenv(dotenv_path=str(REPO_ROOT / ".env"), override=False)
    except Exception:
        pass

    try:
        from pocket_tts import TTSModel
    except ImportError as e:
        print(f"pocket-tts not installed: {e}", file=sys.stderr)
        print("  pip install pocket-tts  (in venv312)", file=sys.stderr)
        return 2

    voice_src = resolve_voice_arg(args.voice)
    print(f"voice: {args.voice} -> {voice_src}")
    print(f"text: {args.text[:80]!r} ({len(args.text)} chars)")
    print(f"quantize: {not args.no_quantize}")

    # --- load model
    quantize = not args.no_quantize
    t0 = time.time()
    try:
        model = TTSModel.load_model(quantize=quantize)
    except Exception as e:
        print(f"load_model failed: {e}", file=sys.stderr)
        return 3
    t_load = time.time() - t0
    print(f"load_model: {t_load:.1f}s  device={model.device}  sr={model.sample_rate}  has_voice_cloning={getattr(model, 'has_voice_cloning', '?')}")

    # --- voice state
    t0 = time.time()
    try:
        voice_state = model.get_state_for_audio_prompt(voice_src)
    except Exception as e:
        print(f"get_state_for_audio_prompt failed: {e}", file=sys.stderr)
        return 4
    t_voice = time.time() - t0
    print(f"voice state: {t_voice:.1f}s")

    # --- RSS
    try:
        import psutil
        rss = psutil.Process().memory_info().rss / 1e6
        print(f"RSS: {rss:.0f} MB")
    except Exception:
        pass

    # --- generate
    out_path = Path(args.out) if args.out else TMP_DIR / f"pocket_test_{args.voice}_{int(time.time())}.wav"
    out_path.parent.mkdir(parents=True, exist_ok=True)

    if args.stream:
        t0 = time.time()
        first = None
        chunks: list = []
        n = 0
        try:
            for chunk in model.generate_audio_stream(voice_state, args.text):
                n += 1
                if first is None:
                    first = time.time() - t0
                    print(f"FIRST chunk at {first:.2f}s  {chunk.shape[0] / model.sample_rate:.2f}s audio")
                chunks.append(chunk)
            total = time.time() - t0
            import torch
            audio = torch.cat(chunks, dim=0) if chunks else torch.zeros(0)
            dur = audio.shape[-1] / model.sample_rate if audio.numel() else 0.0
            print(f"stream done: {n} chunks  total {total:.1f}s  audio {dur:.1f}s  {dur/total:.2f}x realtime")
        except Exception as e:
            print(f"generate_audio_stream failed: {e}", file=sys.stderr)
            import traceback; traceback.print_exc()
            return 5
    else:
        t0 = time.time()
        try:
            audio = model.generate_audio(voice_state, args.text)
        except Exception as e:
            print(f"generate_audio failed: {e}", file=sys.stderr)
            import traceback; traceback.print_exc()
            return 5
        total = time.time() - t0
        dur = audio.shape[-1] / model.sample_rate if audio.numel() else 0.0
        print(f"generate: {total:.1f}s  audio {dur:.1f}s  {dur/total:.2f}x realtime")

    # --- write wav
    try:
        import scipy.io.wavfile
        scipy.io.wavfile.write(str(out_path), model.sample_rate, audio.detach().cpu().numpy())
        print(f"wrote: {out_path}  ({out_path.stat().st_size / 1024:.0f} KB)")
    except Exception as e:
        print(f"write wav failed: {e}", file=sys.stderr)
        return 6

    if args.play:
        try:
            import sounddevice as sd
            print("playing...")
            sd.play(audio.detach().cpu().numpy(), samplerate=model.sample_rate, blocking=True)
            print("playback done")
        except Exception as e:
            print(f"playback failed: {e}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
