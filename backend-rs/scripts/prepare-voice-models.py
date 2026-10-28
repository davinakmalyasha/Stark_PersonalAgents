#!/usr/bin/env python3
"""Prepare the local voice asset tree used by release builds.

Layout:
  models/tts/english_18m_2026-10/model.safetensors
  models/tts/english_18m_2026-10/tokenizer.json
  models/tts/voices/{jarvis,alba,anna}.safetensors
  models/stt/tiny-streaming-en/*.ort + tokenizer.bin

The public TTS weights/tokenizer/catalog voices are downloaded from the non-gated
`kyutai/pocket-tts-without-voice-cloning` repository. JARVIS must be exported once with the
gated model (see the Rust voice docs); pass it with --jarvis-state. The Moonshine tiny-streaming
files are copied from an existing moonshine_voice cache or STARK_STT_MODEL_DIR.
"""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
HF = "https://huggingface.co"
TTS_REPO = "kyutai/pocket-tts-without-voice-cloning"
TTS_REV = "a7056f505a4a8ca72d8b5fb1440071d6f1a35739"
TTS_FILES = [
    "languages/english_18m_2026-10/model.safetensors",
    "languages/english_18m_2026-10/tokenizer.json",
    "languages/english_18m_2026-10/embeddings/alba.safetensors",
    "languages/english_18m_2026-10/embeddings/anna.safetensors",
]
STT_FILES = [
    "encoder.ort",
    "decoder_kv.ort",
    "frontend.weights.ort",
    "cross_kv.ort",
    "adapter.ort",
    "frontend.model.ort",
    "streaming_config.json",
    "tokenizer.bin",
]


def download(url: str, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.is_file() and target.stat().st_size > 0:
        print(f"keep {target}")
        return
    print(f"download {url}")
    tmp = target.with_suffix(target.suffix + ".tmp")
    with urllib.request.urlopen(url, timeout=300) as response, open(tmp, "wb") as out:
        shutil.copyfileobj(response, out)
    tmp.replace(target)


def download_tts(output: Path) -> None:
    for rel in TTS_FILES:
        url = f"{HF}/{TTS_REPO}/resolve/{TTS_REV}/{rel}"
        target = output / "tts" / Path(rel).relative_to("languages/english_18m_2026-10")
        if "/embeddings/" in rel:
            target = output / "tts" / "voices" / Path(rel).name
        download(url, target)


def find_stt(model: str) -> Path:
    if value := os.environ.get("STARK_STT_MODEL_DIR"):
        path = Path(value)
        if path.is_dir():
            return path
    local = Path(os.environ.get("LOCALAPPDATA", "")) / "moonshine_voice"
    candidates = sorted(
        local.glob(f"moonshine_voice/Cache/download.moonshine.ai/model/{model}-streaming-en/*"),
        reverse=True,
    )
    for candidate in candidates:
        if candidate.is_dir() and all((candidate / name).is_file() for name in STT_FILES):
            return candidate
    raise SystemExit(
        f"{model}-streaming model not found. Set STARK_STT_MODEL_DIR to the quantized directory."
    )


def copy_stt(output: Path, model: str) -> None:
    source = find_stt(model)
    target = output / "stt" / f"{model}-streaming-en"
    target.mkdir(parents=True, exist_ok=True)
    for name in STT_FILES:
        shutil.copy2(source / name, target / name)
        print(f"copy {source / name} -> {target / name}")


def copy_jarvis(output: Path, source: Path) -> None:
    if not source.is_file():
        raise SystemExit(f"JARVIS voice state not found: {source}")
    target = output / "tts" / "voices" / "jarvis.safetensors"
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
    print(f"copy {source} -> {target}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output", type=Path, default=ROOT / "models")
    parser.add_argument("--jarvis-state", type=Path)
    parser.add_argument("--skip-tts", action="store_true")
    parser.add_argument("--skip-stt", action="store_true")
    parser.add_argument(
        "--stt-model",
        choices=("medium", "small", "tiny"),
        default="medium",
        help="Moonshine streaming model to bundle (default: medium)",
    )
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if not args.skip_tts:
        download_tts(output)
    jarvis = args.jarvis_state or (
        Path(os.environ["STARK_JARVIS_STATE"])
        if os.environ.get("STARK_JARVIS_STATE")
        else None
    )
    if jarvis is not None:
        copy_jarvis(output, jarvis)
    elif not args.skip_tts:
        print(
            "warning: no --jarvis-state; the default JARVIS voice will not be bundled",
            file=sys.stderr,
        )
    if not args.skip_stt:
        copy_stt(output, args.stt_model)
    print(f"voice assets ready at {output}")


if __name__ == "__main__":
    main()
