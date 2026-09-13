"""
LLM Package
Provides multi-provider LLM calling, streaming, fallback orchestration, token tracking, and loop guards.
"""
import os
import sys

_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _ROOT not in sys.path:
    sys.path.insert(0, _ROOT)

_DESKTOP = os.path.join(_ROOT, "Desktop")
if os.path.isdir(_DESKTOP) and _DESKTOP not in sys.path:
    sys.path.insert(0, _DESKTOP)

_VOICE = os.path.join(_ROOT, "Voice")
if os.path.isdir(_VOICE) and _VOICE not in sys.path:
    sys.path.insert(0, _VOICE)

from .llm_config import TASK_CATEGORIES
from .llm_stats import (
    update_token_stats,
    calculate_cost,
)
from .loop_detector import (
    LoopGuard,
)
from .llm import (
    _resolve_llm_provider,
    _load_persisted_provider,
    _save_persisted_provider,
)

__all__ = [
    "TASK_CATEGORIES",
    "update_token_stats",
    "calculate_cost",
    "LoopGuard",
    "_resolve_llm_provider",
    "_load_persisted_provider",
    "_save_persisted_provider",
]
