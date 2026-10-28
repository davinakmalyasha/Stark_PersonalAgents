# Third-party voice notices

The release bundle contains or links the following components. Keep this file, the
`POCKET_TTS_LICENSE` file, and the model license with the installed application.

## Pocket TTS

- Source: https://github.com/kyutai-labs/pocket-tts
- Rust port license: MIT (see `backend-rs/crates/stark-voice/POCKET_TTS_LICENSE`)
- Model: `kyutai/pocket-tts-without-voice-cloning`, `english_18m_2026-10`
- Model license: CC-BY-4.0
- Attribution: Kyutai Pocket TTS, 18.2M English model

## ptts / xn-ptts

- Source: https://github.com/gradium-ai/xn-ptts
- Fork used: https://github.com/gabrieldemarmiesse/xn-ptts at
  `7efa26973f81bba42cf2ace8da2c1eb9b605d54d`
- Distributed under the upstream repository's license.

## Moonshine Voice / moonshine-rs

- Source: https://github.com/moonshine-ai/moonshine and
  https://github.com/ghchinoy/moonshine-rs
- License: MIT (Moonshine models and the Rust wrapper)
- Model: tiny-streaming English, quantized

## ONNX Runtime

- Source: https://github.com/microsoft/onnxruntime
- License: MIT
- Windows runtime DLL version 1.26.x is bundled next to `stark-server.exe`.

## JARVIS voice

- `jarvis.safetensors` is derived from a user-provided reference clip.
  Confirm that you have the rights to redistribute that voice before publishing an installer.
