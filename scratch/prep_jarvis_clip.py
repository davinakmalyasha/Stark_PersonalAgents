#!/usr/bin/env python3
"""
Convert a user-provided voice clip to the clean 7s Jarvis reference:
  1. Any audio (mp3/wav/m4a/ogg...) -> 16-bit mono 24kHz WAV (Pocket sample rate)
  2. Trim to exactly 7s (middle 7s if longer; keep full if shorter)
  3. Best-effort loudness normalize (loudnorm) for a stable clone
  4. Write .deep_agents/tts_ref/jarvis_clean.wav

Usage:
  venv\\Scripts\\python.exe scratch/prep_jarvis_clip.py <path-to-your-clip>
"""
import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TTS_REF = REPO_ROOT / ".deep_agents" / "tts_ref"
OUT = TTS_REF / "jarvis_clean.wav"
TARGET_SEC = 7.0


def ffmpeg():
    import imageio_ffmpeg
    return imageio_ffmpeg.get_ffmpeg_exe()


def probe_duration(src: str) -> float:
    ff = ffmpeg()
    r = subprocess.run([ff, "-i", src], capture_output=True, text=True, timeout=30)
    # ffmpeg prints duration to stderr like: Duration: 00:00:27.04
    import re
    m = re.search(r"Duration:\s*(\d+):(\d+):(\d+\.?\d*)", r.stderr or "")
    if not m:
        return 0.0
    h, mi, s = m.groups()
    return int(h) * 3600 + int(mi) * 60 + float(s)


def main() -> int:
    p = argparse.ArgumentParser(description="Prep 7s clean Jarvis clone reference")
    p.add_argument("src", help="path to your voice clip (mp3/wav/m4a/ogg/... )")
    args = p.parse_args()

    src = os.path.abspath(args.src)
    if not os.path.isfile(src):
        print(f"[err] file not found: {src}", file=sys.stderr)
        return 1
    TTS_REF.mkdir(parents=True, exist_ok=True)

    ff = ffmpeg()
    print(f"[1/3] source: {src} ({os.path.getsize(src)/1e6:.1f} MB)")

    dur = probe_duration(src)
    print(f"[1/3] duration: {dur:.2f}s")

    # Build the trim filter: middle 7s if longer, full if shorter
    if dur > TARGET_SEC + 0.5:
        start = max(0.0, (dur - TARGET_SEC) / 2)
        print(f"[2/3] trimming to {TARGET_SEC:.0f}s (middle window, start={start:.2f}s)")
        trim = ["-ss", f"{start:.3f}", "-t", f"{TARGET_SEC:.3f}"]
    else:
        print(f"[2/3] clip already <= {TARGET_SEC:.0f}s — keeping full length")
        trim = []

    tmp = TTS_REF / "jarvis_clean.tmp.wav"
    cmd = [ff, "-y"]
    if trim:
        cmd += trim
    cmd += ["-i", src, "-ar", "24000", "-ac", "1", "-sample_fmt", "s16"]
    # loudnorm first pass; if it fails, plain conversion
    cmd += ["-af", "loudnorm=I=-16:TP=-1.5:LRA=11", str(tmp)]
    try:
        subprocess.run(cmd, check=True, capture_output=True, timeout=120)
    except Exception:
        # fallback: no loudnorm
        cmd2 = [ff, "-y"] + trim + ["-i", src, "-ar", "24000", "-ac", "1", "-sample_fmt", "s16", str(tmp)]
        subprocess.run(cmd2, check=True, capture_output=True, timeout=120)

    os.replace(tmp, OUT)
    size = OUT.stat().st_size / 1e6
    dur2 = probe_duration(str(OUT))
    print(f"[3/3] wrote {OUT} ({size:.2f} MB, {dur2:.2f}s, 24kHz mono s16)")
    print("[ok] jarvis_clean.wav ready — now run:")
    print("  venv\\Scripts\\python.exe scratch\\test_pocket_standalone.py --voice jarvis --stream --play")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
