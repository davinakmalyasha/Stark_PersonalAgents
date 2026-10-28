# Voice in the Rust backend

The voice stack is single-process:

- **TTS**: vendored official Pocket TTS Rust port, 18.2M `english_18m_2026-10` model.
- **STT**: `moonshine-rs` / `libmoonshine` tiny-streaming.
- **Mic capture**: `cpal`, fed directly to Moonshine's streaming API.
- **Screen**: `/api/voice/ws` + `/api/vision/frame` + the `look_at_screen` / `set_screen_share` tools.

No Python, PyTorch, ONNX Python package, or TTS sidecar is used at runtime.

## Why the release target is MSVC

`moonshine-rs` ships the Windows `libmoonshine` prebuilt as MSVC static libraries. The GNU
toolchain cannot link its C++ runtime (`__CxxFrameHandler4`, MSVC STL symbols). The release
backend is therefore built for `x86_64-pc-windows-msvc`; the Tauri shell may remain GNU because
the backend is a separate process.

The TTS port itself works with GNU when its config uses `tokenizer.json` (not SentencePiece).
The vendored config does that, which avoids the native SentencePiece build entirely.

## Build prerequisites

- Rust MSVC target: `rustup target add x86_64-pc-windows-msvc`
- LLVM `clang-cl` and `lld-link` in `PATH` (MSVC target linker), or Visual Studio Build Tools.
- `cargo-xwin` when building without Visual Studio:
  `pip install cargo-xwin`
- libclang for `moonshine-sys` bindgen. On this machine:
  `pip install libclang` and set `LIBCLANG_PATH` to `venv/Lib/site-packages/clang/native`.
- Models in `backend-rs/models` (or `STARK_MODELS_DIR`).

## Model layout

```
models/
  tts/
    english_18m_2026-10/
      model.safetensors       # non-gated 18M weights, ~112 MB
      tokenizer.json
    voices/
      jarvis.safetensors      # exported once from jarvis_clean.wav with the 18M model
      alba.safetensors
      anna.safetensors
  stt/
    tiny-streaming-en/
      encoder.ort
      decoder_kv.ort
      frontend.weights.ort
      cross_kv.ort
      adapter.ort
      tokenizer.bin
```

Prepare public assets with:

```bash
python backend-rs/scripts/prepare-voice-models.py --jarvis-state path/to/jarvis_18m.safetensors
```

## Build

```bash
cd backend-rs
export CARGO_TARGET_DIR=D:/cargo-target/stark-msvc
export XWIN_CACHE_DIR=D:/cargo-target/xwin
export LIBCLANG_PATH=D:/MyProject/LangChain/venv/Lib/site-packages/clang/native
cargo-xwin build --release -p stark-server --features voice-stt \
  --target x86_64-pc-windows-msvc
```

Copy `onnxruntime.dll` (1.26.x preferred) next to `stark-server.exe`. The old 1.17 DLL shipped
inside the upstream prebuilt archive cannot load the current tiny-streaming models: they request
ONNX Runtime API 23.

For the Tauri installer, `prepare-sidecar.mjs` copies the selected backend binary, models, and
`onnxruntime.dll` into `src-tauri/{binaries,resources}`. A release build needs:

```bash
export STARK_SERVER_BIN=D:/cargo-target/stark-msvc/x86_64-pc-windows-msvc/release/stark-server.exe
export STARK_MODELS_DIR=D:/MyProject/LangChain-rust/backend-rs/models
npm run tauri build
```

## Runtime resolution

`STARK_MODELS_DIR` wins. Otherwise the server looks for `models`/`resources/models` next to the
executable or under the harness root. If no models are found, the server still starts and voice
routes return `503` instead of crashing.

## Voice answers to ask_user

`QuestionBroker` pending batches carry the originating `chat_id`
(`GET /api/ask/pending` exposes it). While the mic is on, the voice overlay
polls for batches belonging to its own chat, speaks each question with TTS,
and routes the next committed turns to `POST /api/ask/answer` instead of
starting new agent runs. The main-window question modal stays as a fallback
and is also shown during active voice runs. Verified end to end: a blocked
`ask_user` run resumed after a spoken answer and completed.

## Known limits

- `/api/transcribe` accepts WAV/MP3/AAC/FLAC/OGG/M4A through `symphonia`. Browser
  `MediaRecorder` WebM/Opus is not decoded by that path; the primary live voice flow uses
  server-side `cpal` PCM, not WebM. Add a pure-Rust Opus path or ffmpeg only if file-upload
  fallback parity is required.
- The 18M model is English-only.
- The Rust TTS sampler is deterministic but not sample-identical to Python.

## Measured on this machine (Ryzen 5 7520U, 7.8 GB RAM)

- Python 100M JARVIS, quantized PyTorch: warm **24.4 s**, warm RSS **983 MB**; one sentence took
  **3.28 s**.
- Rust 18M JARVIS, release, single process: warm **0.64 s**, warm RSS **237 MB**, one sentence
  **0.82 s**, first PCM stream chunk **92 ms**, RSS after streaming **279 MB**.
- Packaged installer: **131 MB**, including the 18M model, three voice states, Moonshine tiny,
  ONNX Runtime DLL, and the Rust sidecar.
