"""
Memory Package
Provides unified memory management, chat memory, context compaction, daemons, and embedding services.
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

from .memory_io import (
    load_memory,
    save_memory,
)
from .memory_manager import (
    ensure_memory,
    update_project_meta,
    load_cached_code_map,
    load_cached_project_identity,
    refresh_code_map,
    ensure_project_identity,
    update_feature,
    append_lesson,
    is_valid_build_request,
    CODE_MAP_START,
    CODE_MAP_END,
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
from .memory_consolidation_daemon import (
    consolidate_memories,
)
from .dev_memory_helper import (
    get_developer_memory_context,
)
from .chat_context import (
    build_chat_context,
)
from .context_budget import (
    estimate_tokens,
)
from .context_compaction import (
    tier1_compact,
    tier2_compact,
    checkpoint_compact,
    prune_stale_tool_outputs,
    build_structured_resume_summary,
    invalidate_stale_reads,
    compact_session_log,
    truncation_note,
    strip_thinking,
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
    "load_memory",
    "save_memory",
    "ensure_memory",
    "update_project_meta",
    "load_cached_code_map",
    "load_cached_project_identity",
    "refresh_code_map",
    "ensure_project_identity",
    "update_feature",
    "append_lesson",
    "is_valid_build_request",
    "CODE_MAP_START",
    "CODE_MAP_END",
    "load_chat_memory",
    "save_chat_memory",
    "add_chat_message",
    "resolve_request_text",
    "summarize_session_chunk",
    "summarize_previous_day",
    "is_continue_command",
    "consolidate_memories",
    "get_developer_memory_context",
    "build_chat_context",
    "estimate_tokens",
    "tier1_compact",
    "tier2_compact",
    "checkpoint_compact",
    "prune_stale_tool_outputs",
    "build_structured_resume_summary",
    "invalidate_stale_reads",
    "compact_session_log",
    "truncation_note",
    "strip_thinking",
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
