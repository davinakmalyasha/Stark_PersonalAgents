#!/usr/bin/env python3
"""TTS micro-bench: first-chunk latency + realtime factor per sentence.

A/B the beartype switch by running twice (pocket_tts reads the env flag
at import, so it needs separate processes):

  venv\\Scripts\\python.exe scratch\\bench_tts.py
  set POCKET_TTS_NO_BEARTYPE=1 && venv\\Scripts\\python.exe scratch\\bench_tts.py

Uses the catalog voice (no auth) and quantized model — same as live mode.
Prints one JSON line for before/after diffing.
"""
import json
import os
import sys
import time

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

SENTENCES = [
    "Sure, on it.",
    "Let me check the workspace and get back to you.",
    "Done. I created the files and ran a quick check.",
]

BEARTYPE_OFF = os.environ.get("POCKET_TTS_NO_BEARTYPE") == "1"


def main() -> int:
    from pocket_tts import TTSModel

    t0 = time.time()
    model = TTSModel.load_model(quantize=True)
    load_s = time.time() - t0

    t0 = time.time()
    vs = model.get_state_for_audio_prompt("alba")
    voice_s = time.time() - t0

    import torch

    rows = []
    for text in SENTENCES:
        t0 = time.time()
        first = None
        chunks = []
        for chunk in model.generate_audio_stream(vs, text, frames_after_eos=2):
            if first is None:
                first = time.time() - t0
            chunks.append(chunk)
        total = time.time() - t0
        audio = torch.cat(chunks, dim=0) if chunks else torch.zeros(0)
        dur = audio.shape[-1] / model.sample_rate if audio.numel() else 0.0
        rows.append({
            "text": text,
            "first_chunk_s": round(first or -1, 3),
            "total_s": round(total, 3),
            "audio_s": round(dur, 3),
            "rtf": round(dur / total, 2) if total > 0 else 0.0,
        })

    print(json.dumps({
        "beartype": "off" if BEARTYPE_OFF else "on",
        "load_s": round(load_s, 1),
        "voice_s": round(voice_s, 1),
        "sentences": rows,
    }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
