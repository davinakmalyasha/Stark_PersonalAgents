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

## transcribe.cpp / Parakeet Unified English

- Engine: https://github.com/handy-computer/transcribe.cpp
  (Anomaly's pinned fork `60bed89dab84f7c1fddc3a17ad7fa8a27ddfba19`)
- Engine license: MIT
- Model: NVIDIA `parakeet-unified-en-0.6b`, Q8_0 GGUF conversion by
  handy-computer, 731,357,568 bytes
- Model license: NVIDIA Open Model License
  https://www.nvidia.com/en-us/agreements/enterprise-software/nvidia-open-model-license/
- SHA-256: `4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795`

## JARVIS voice

- `jarvis.safetensors` is derived from a user-provided reference clip.
  Confirm that you have the rights to redistribute that voice before publishing an installer.
