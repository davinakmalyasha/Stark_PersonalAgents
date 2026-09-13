"""
LLM Configuration — canonical model ids + capability registry.

Provider/mode routing lives in llm/provider_modes.py; this module only holds
the canonical DeepSeek ids (env-overridable) and the vision/audio capability
registry consumed by provider listings and vision plumbing.

Model names are env-overridable so the whole stack can be routed through a compatible
gateway (e.g. OpenCode Go, https://opencode.ai/zen/go/v1) without code edits:
    DEEPSEEK_MODEL_FLASH=deepseek-v4.1-flash      # current DeepSeek flagship flash
    DEEPSEEK_MODEL_PRO=deepseek-v4.1-flash          # V4-Pro retired; routes to V4.1-Flash
"""
import os

DEEPSEEK_MODEL_FLASH = os.getenv("DEEPSEEK_MODEL_FLASH", "deepseek-v4.1-flash")
DEEPSEEK_MODEL_PRO = os.getenv("DEEPSEEK_MODEL_PRO", "deepseek-v4.1-flash")

# ── Vision capability registry ────────────────────────────────────────────────
# DeepSeek-V4.1-Flash has native multimodal vision, so the separate legacy
# vision model is retired; the vision slot points at the same V4.1-Flash id.
# supports_vision() is consumed by provider listings and the vision channel.
DEEPSEEK_MODEL_VISION = os.getenv("DEEPSEEK_MODEL_VISION", "deepseek-v4.1-flash")

#: Per-provider audio-capable model (canonical name mapped in llm.py).
DEEPSEEK_MODEL_AUDIO = os.getenv("DEEPSEEK_MODEL_AUDIO", "deepseek-v4-audio")

#: Models that accept audio input parts (OpenAI input_audio).
AUDIO_MODELS = os.getenv("AUDIO_MODELS", "mimo-v2.5,deepseek-v4-audio").lower().split(",")
VISION_MODELS = {DEEPSEEK_MODEL_VISION}


def supports_vision(model: str) -> bool:
    """True if the model accepts image input.

    V4.1-Flash has native vision without a 'vision' substring, so exact ids
    in VISION_MODELS are authoritative; the 'vision' substring keeps future
    vision variants auto-supported without edits."""
    m = (model or "").lower()
    return m in {v.lower() for v in VISION_MODELS} or ("vision" in m) or ("v4.1-flash" in m) or (m in ("deepseek-flash",))


def supports_audio(model: str) -> bool:
    """True if the model accepts audio input (OpenAI input_audio parts)."""
    m = (model or "").lower()
    return any(a and a in m for a in AUDIO_MODELS) or m in {v.lower() for v in AUDIO_MODELS}


TASK_CATEGORIES = {
    "orchestrator": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
    ],
    "complex": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
    ],
    "fixing": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
    ],
    "critic": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
    ],
    "standard": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
    ],
    "lightweight": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
    ],
}



