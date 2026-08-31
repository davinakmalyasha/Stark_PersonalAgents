"""
Memory Package
Provides unified memory management, chat memory, context compaction, and embedding services.
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

from .memory_manager import (
    load_cached_code_map,
    load_cached_project_identity,
    refresh_code_map,
    refresh_code_map_context,
    ensure_project_cache,
    ensure_project_identity,
    is_valid_build_request,
)
from .chat_memory_manager import (
    load_chat_memory,
    save_chat_memory,
    add_chat_message,
    resolve_request_text,
    summarize_session_chunk,
    summarize_previous_day,
    is_continue_command,
)
from .chat_context import (
    build_chat_context,
)
from .context_budget import (
    estimate_tokens,
    estimate_message_tokens,
    estimate_messages,
)
from .context_compaction import (
    CompactionPolicy,
    get_compaction_policy,
    truncation_note,
)
from .repo_map_generator import (
    RepoMapGenerator,
    get_file_signatures,
    get_file_signatures_with_lines,
)

from .embedding_service import (
    embed,
    embed_cached,
    embed_batch,
    cosine_similarity,
    vector_store,
    vector_search,
    vector_remove,
    vector_clear,
    vector_count,
    vector_stats,
    flush_vector_buffer,
)

__all__ = [
    "load_cached_code_map",
    "load_cached_project_identity",
    "refresh_code_map",
    "refresh_code_map_context",
    "ensure_project_cache",
    "ensure_project_identity",
    "is_valid_build_request",
    "load_chat_memory",
    "save_chat_memory",
    "add_chat_message",
    "resolve_request_text",
    "summarize_session_chunk",
    "summarize_previous_day",
    "is_continue_command",
    "build_chat_context",
    "estimate_tokens",
    "estimate_message_tokens",
    "estimate_messages",
    "CompactionPolicy",
    "get_compaction_policy",
    "truncation_note",
    "embed",
    "embed_cached",
    "embed_batch",
    "cosine_similarity",
    "vector_store",
    "vector_search",
    "vector_remove",
    "vector_clear",
    "vector_count",
    "vector_stats",
    "flush_vector_buffer",
    "RepoMapGenerator",
    "get_file_signatures",
    "get_file_signatures_with_lines",
]
