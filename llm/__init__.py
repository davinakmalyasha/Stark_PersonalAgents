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

from .llm_config import (
    TASK_CATEGORIES,
    get_task_category,
)
from .llm_stats import (
    TokenUsageTracker,
    update_token_stats,
    calculate_cost,
)
from .loop_detector import (
    LoopGuard,
)
from .llm import (
    invoke_with_fallback,
    strip_thinking_from_messages,
    _extract_text,
    _resolve_llm_provider,
    _load_persisted_provider,
    _save_persisted_provider,
)

# Aliases for cleaner public API
LLMStats = TokenUsageTracker
get_task_model = get_task_category

__all__ = [
    "TASK_CATEGORIES",
    "get_task_category",
    "get_task_model",
    "TokenUsageTracker",
    "LLMStats",
    "update_token_stats",
    "calculate_cost",
    "LoopGuard",
    "invoke_with_fallback",
    "strip_thinking_from_messages",
    "_extract_text",
    "_resolve_llm_provider",
    "_load_persisted_provider",
    "_save_persisted_provider",
]
