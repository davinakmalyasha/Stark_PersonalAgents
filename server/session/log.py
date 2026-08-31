"""
Session JSONL — append-only, tree-structured session log (Pi-style format).

One JSONL file per session under ``<workspace>/.deep_agents/sessions/``.
Every entry is a typed, JSON-serializable record with an ``id`` and a
``parentId``, forming a tree inside a single file (in-place branching).

The LLM message history is DERIVED from the log via
:meth:`SessionManager.build_context_entries` + :func:`convert_to_llm` —
never stored separately. Compaction writes a ``compaction`` entry whose
``retainedTail`` is a self-contained checkpoint, so context rebuilds skip
the older entries entirely.

Entry vocabulary (adapted from the Pi session format):
  session            — header line (metadata only, no id/parentId)
  message            — user / assistant / toolResult / custom_message
  compaction         — summary + first_kept_entry_id + retainedTail checkpoint
  branch_summary     — summary of an abandoned branch (tree navigation)
  model_change       — provider/model switch mid-session
  thinking_level_change — reasoning level switch mid-session
  custom             — extension state, NOT part of LLM context
  label              — user bookmark on an entry
  session_info       — display name

This module must stay dependency-light (no imports from tools/llm/orchestrator)
so it cannot create import cycles; LangChain imports are lazy.
"""

from __future__ import annotations

import json
import os
import threading
import uuid
from datetime import datetime
from typing import Annotated, Any, Literal, Optional, Union

from pydantic import BaseModel, Field, TypeAdapter, field_validator

# ────────────────────────────────────────────────────────────────────────────
# Content blocks
# ────────────────────────────────────────────────────────────────────────────


class TextBlock(BaseModel):
    type: Literal["text"] = "text"
    text: str


class ThinkingBlock(BaseModel):
    type: Literal["thinking"] = "thinking"
    thinking: str


class ToolCallBlock(BaseModel):
    type: Literal["toolCall"] = "toolCall"
    id: str = ""
    name: str
    arguments: dict = Field(default_factory=dict)


class ImageBlock(BaseModel):
    type: Literal["image"] = "image"
    data: str = ""
    mime_type: str = "image/png"


ContentBlock = Annotated[
    Union[TextBlock, ThinkingBlock, ToolCallBlock, ImageBlock],
    Field(discriminator="type"),
]

CONTENT_BLOCK_ADAPTER = TypeAdapter(ContentBlock)


# ────────────────────────────────────────────────────────────────────────────
# Token / cost accounting (mirrors llm_stats token_usage shape)
# ────────────────────────────────────────────────────────────────────────────


class Usage(BaseModel):
    input: int = 0
    output: int = 0
    cache_read: int = 0
    cache_write: int = 0
    total_tokens: int = 0
    cost: float = 0.0


# ────────────────────────────────────────────────────────────────────────────
# Entries
# ────────────────────────────────────────────────────────────────────────────


class SessionHeader(BaseModel):
    type: Literal["session"] = "session"
    version: int = 1
    id: str
    timestamp: int
    cwd: str = ""
    parent_session: Optional[str] = None
    delegation_depth: int = 0
    name: Optional[str] = None
    chat_id: Optional[str] = None


class SessionEntryBase(BaseModel):
    type: str
    id: str
    parent_id: Optional[str] = None
    timestamp: int = 0  # filled by SessionManager._make before append


class MessageEntry(SessionEntryBase):
    type: Literal["message"] = "message"
    role: Literal["user", "assistant", "toolResult", "custom_message"]
    content: Union[str, list[ContentBlock]] = ""
    # assistant fields
    provider: Optional[str] = None
    model: Optional[str] = None
    usage: Optional[Usage] = None
    stop_reason: Optional[str] = None  # stop | length | toolUse | error | aborted
    error_message: Optional[str] = None
    # toolResult fields
    tool_call_id: Optional[str] = None
    tool_name: Optional[str] = None
    is_error: bool = False
    # custom_message fields
    custom_type: Optional[str] = None
    display: bool = True
    # source attribution (user | inject | goal | subagent-report | ...)
    source: Optional[str] = None
    # tool-private metadata (never sent verbatim to the model)
    details: Optional[dict] = None

    @field_validator("content", mode="before")
    @classmethod
    def _coerce_content(cls, v: Any) -> Any:
        if v is None:
            return ""
        if isinstance(v, str):
            return v
        if isinstance(v, (list, tuple)):
            out = []
            for item in v:
                if isinstance(item, BaseModel):
                    out.append(item)
                elif isinstance(item, dict):
                    try:
                        out.append(CONTENT_BLOCK_ADAPTER.validate_python(item))
                    except Exception:
                        out.append({"type": "text", "text": str(item)})
                else:
                    out.append({"type": "text", "text": str(item)})
            return out
        return str(v)


class CompactionEntry(SessionEntryBase):
    type: Literal["compaction"] = "compaction"
    summary: str = ""
    first_kept_entry_id: Optional[str] = None
    retained_tail: Optional[list[MessageEntry]] = None
    tokens_before: int = 0
    details: Optional[dict] = None  # {read_files: [...], modified_files: [...]}


class BranchSummaryEntry(SessionEntryBase):
    type: Literal["branch_summary"] = "branch_summary"
    summary: str = ""
    from_id: Optional[str] = None
    details: Optional[dict] = None


class ModelChangeEntry(SessionEntryBase):
    type: Literal["model_change"] = "model_change"
    provider: Optional[str] = None
    model_id: Optional[str] = None


class ThinkingLevelChangeEntry(SessionEntryBase):
    type: Literal["thinking_level_change"] = "thinking_level_change"
    thinking_level: str = "disabled"


class CustomEntry(SessionEntryBase):
    type: Literal["custom"] = "custom"
    custom_type: str = ""
    data: Any = None


class LabelEntry(SessionEntryBase):
    type: Literal["label"] = "label"
    target_id: Optional[str] = None
    label: Optional[str] = None


class SessionInfoEntry(SessionEntryBase):
    type: Literal["session_info"] = "session_info"
    name: Optional[str] = None


SessionEntry = Annotated[
    Union[
        MessageEntry,
        CompactionEntry,
        BranchSummaryEntry,
        ModelChangeEntry,
        ThinkingLevelChangeEntry,
        CustomEntry,
        LabelEntry,
        SessionInfoEntry,
    ],
    Field(discriminator="type"),
]

# Pydantic only attaches validation helpers to Annotated unions when they are
# processed inside a model or a TypeAdapter — hence the adapter for open().
SESSION_ENTRY_ADAPTER = TypeAdapter(SessionEntry)


# ────────────────────────────────────────────────────────────────────────────
# Block helpers
# ────────────────────────────────────────────────────────────────────────────


def text_block(text: str) -> TextBlock:
    return TextBlock(text=text)


def tool_call_block(name: str, arguments: dict, call_id: str = "") -> ToolCallBlock:
    return ToolCallBlock(id=call_id, name=name, arguments=arguments)


def thinking_block(text: str) -> ThinkingBlock:
    return ThinkingBlock(thinking=text)


def message_text(content: Union[str, list[ContentBlock]]) -> str:
    """Join a message's text blocks into a plain string."""
    if isinstance(content, str):
        return content
    parts = []
    for block in content:
        if block.type == "text":
            parts.append(block.text)
        elif block.type == "thinking":
            parts.append(block.thinking)
    return "\n".join(parts)


def entry_to_dict(entry: SessionEntry | SessionHeader) -> dict:
    return entry.model_dump(mode="json", exclude_none=False)


# ────────────────────────────────────────────────────────────────────────────
# Storage helpers
# ────────────────────────────────────────────────────────────────────────────


def session_dir_for(workspace: str) -> str:
    """Default session storage directory for a workspace."""
    return os.path.join(workspace, ".deep_agents", "sessions")


def _now_ms() -> int:
    return int(datetime.now().timestamp() * 1000)


def _new_id() -> str:
    return uuid.uuid4().hex[:8]


def _default_filename() -> str:
    return f"{int(datetime.now().timestamp() * 1000)}_{uuid.uuid4().hex[:8]}.jsonl"


# ────────────────────────────────────────────────────────────────────────────
# SessionManager
# ────────────────────────────────────────────────────────────────────────────


class SessionManager:
    """Append-only tree session log (Pi-style)."""

    def __init__(
        self,
        path: Optional[str],
        header: SessionHeader,
        entries: Optional[list[SessionEntry]] = None,
        leaf_id: Optional[str] = None,
    ):
        self._path = path
        self._header = header
        self._entries: list[SessionEntry] = list(entries or [])
        self._by_id = {e.id: e for e in self._entries}
        self._leaf_id = leaf_id if leaf_id in self._by_id else None
        self._lock = threading.RLock()

    # ── Factories ────────────────────────────────────────────────────────────

    @classmethod
    def create(
        cls,
        cwd: str,
        session_dir: Optional[str] = None,
        parent_session: Optional[str] = None,
        delegation_depth: int = 0,
        name: Optional[str] = None,
        chat_id: Optional[str] = None,
    ) -> "SessionManager":
        session_dir = session_dir or session_dir_for(cwd)
        os.makedirs(session_dir, exist_ok=True)
        path = os.path.join(session_dir, _default_filename())
        header = SessionHeader(
            id=uuid.uuid4().hex,
            timestamp=_now_ms(),
            cwd=cwd,
            parent_session=parent_session,
            delegation_depth=delegation_depth,
            name=name,
            chat_id=chat_id,
        )
        mgr = cls(path, header)
        with open(path, "a", encoding="utf-8") as f:
            f.write(json.dumps(entry_to_dict(header), ensure_ascii=False) + "\n")
        return mgr

    @classmethod
    def open(cls, path: str) -> "SessionManager":
        """Load a session file. Leaf defaults to the last entry (file tail).

        A trailing partial line (crash) is dropped silently.
        """
        header: Optional[SessionHeader] = None
        entries: list[SessionEntry] = []
        with open(path, "r", encoding="utf-8") as f:
            lines = f.read().splitlines()
        for line in lines:
            if not line.strip():
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue  # crash residue
            if obj.get("type") == "session":
                try:
                    header = SessionHeader.model_validate(obj)
                except Exception:
                    header = None
                continue
            try:
                entries.append(SESSION_ENTRY_ADAPTER.validate_python(obj))
            except Exception:
                continue
        if header is None:
            raise ValueError(f"Not a session file (no header): {path}")
        leaf_id = entries[-1].id if entries else None
        return cls(path, header, entries, leaf_id)

    @classmethod
    def open_or_create(
        cls,
        path: Optional[str],
        cwd: str,
        session_dir: Optional[str] = None,
        parent_session: Optional[str] = None,
        delegation_depth: int = 0,
    ) -> "SessionManager":
        if path and os.path.isfile(path):
            return cls.open(path)
        return cls.create(cwd, session_dir, parent_session, delegation_depth)

    @classmethod
    def fork_from(
        cls,
        source_path: str,
        target_cwd: str,
        session_dir: Optional[str] = None,
        name: Optional[str] = None,
    ) -> "SessionManager":
        """Copy a source session into a new session file (full history kept)."""
        src = cls.open(source_path)
        mgr = cls.create(target_cwd, session_dir, parent_session=source_path, name=name)
        for entry in src._entries:
            mgr._append(entry)
        return mgr

    @classmethod
    def in_memory(cls, cwd: str = "") -> "SessionManager":
        header = SessionHeader(id=uuid.uuid4().hex, timestamp=_now_ms(), cwd=cwd)
        return cls(None, header)

    # ── Accessors ────────────────────────────────────────────────────────────

    @property
    def header(self) -> SessionHeader:
        return self._header

    def get_leaf_id(self) -> Optional[str]:
        return self._leaf_id

    def get_leaf_entry(self) -> Optional[SessionEntry]:
        if self._leaf_id is None:
            return None
        return self._by_id.get(self._leaf_id)

    def get_entry(self, entry_id: str) -> Optional[SessionEntry]:
        return self._by_id.get(entry_id)

    def get_entries(self) -> list[SessionEntry]:
        return list(self._entries)

    def get_children(self, parent_id: Optional[str]) -> list[SessionEntry]:
        return [e for e in self._entries if e.parent_id == parent_id]

    def get_header(self) -> SessionHeader:
        return self._header

    def get_cwd(self) -> str:
        return self._header.cwd

    def get_session_id(self) -> str:
        return self._header.id

    def get_session_file(self) -> Optional[str]:
        return self._path

    def is_persisted(self) -> bool:
        return self._path is not None

    def get_session_name(self) -> Optional[str]:
        name = self._header.name
        for entry in reversed(self._entries):
            if isinstance(entry, SessionInfoEntry) and entry.name:
                return entry.name
        return name

    def get_tree(self) -> dict[str, Any]:
        """Full tree: {entry_id: {"entry": ..., "children": [ids]}}."""
        tree: dict[str, Any] = {}
        children_map: dict[Optional[str], list[str]] = {}
        for e in self._entries:
            children_map.setdefault(e.parent_id, []).append(e.id)
            tree[e.id] = {"entry": e, "children": children_map.setdefault(e.id, [])}
        return tree

    def get_branch(self, from_id: Optional[str] = None) -> list[SessionEntry]:
        """Walk from an entry (default: leaf) to the root, root-first order."""
        entry_id = from_id or self._leaf_id
        if entry_id is None:
            return []
        chain: list[SessionEntry] = []
        seen: set[str] = set()
        current: Optional[SessionEntry] = self._by_id.get(entry_id)
        while current is not None and current.id not in seen:
            seen.add(current.id)
            chain.append(current)
            if current.parent_id is None:
                break
            current = self._by_id.get(current.parent_id)
        chain.reverse()
        return chain

    # ── Navigation ───────────────────────────────────────────────────────────

    def branch(self, entry_id: str) -> None:
        """Move the leaf to an earlier entry (in-place branching)."""
        if entry_id not in self._by_id:
            raise KeyError(f"Unknown entry id: {entry_id}")
        with self._lock:
            self._leaf_id = entry_id

    def reset_leaf(self) -> None:
        with self._lock:
            self._leaf_id = None

    # ── Appends ──────────────────────────────────────────────────────────────

    def _append(self, entry: SessionEntry) -> SessionEntry:
        with self._lock:
            if entry.id not in self._by_id:
                self._by_id[entry.id] = entry
                self._entries.append(entry)
            self._leaf_id = entry.id
            if self._path:
                with open(self._path, "a", encoding="utf-8") as f:
                    f.write(json.dumps(entry_to_dict(entry), ensure_ascii=False) + "\n")
            return entry

    def _make(self, entry: SessionEntry) -> SessionEntry:
        entry.timestamp = _now_ms()
        entry.parent_id = self._leaf_id
        return self._append(entry)

    def append_message(
        self,
        role: Literal["user", "assistant", "toolResult", "custom_message"],
        content: Union[str, list[ContentBlock], list[dict], None] = "",
        *,
        provider: Optional[str] = None,
        model: Optional[str] = None,
        usage: Optional[Union[Usage, dict]] = None,
        stop_reason: Optional[str] = None,
        error_message: Optional[str] = None,
        tool_call_id: Optional[str] = None,
        tool_name: Optional[str] = None,
        is_error: bool = False,
        custom_type: Optional[str] = None,
        display: bool = True,
        source: Optional[str] = None,
        details: Optional[dict] = None,
        entry_id: Optional[str] = None,
    ) -> str:
        if usage is not None and not isinstance(usage, Usage):
            usage = Usage.model_validate(usage)
        entry = MessageEntry(
            type="message",
            id=entry_id or _new_id(),
            role=role,
            content=content or "",
            provider=provider,
            model=model,
            usage=usage,
            stop_reason=stop_reason,
            error_message=error_message,
            tool_call_id=tool_call_id,
            tool_name=tool_name,
            is_error=is_error,
            custom_type=custom_type,
            display=display,
            source=source,
            details=details,
        )
        return self._make(entry).id

    def append_assistant(
        self,
        content: Union[str, list[ContentBlock], list[dict]],
        *,
        provider: Optional[str] = None,
        model: Optional[str] = None,
        usage: Optional[Union[Usage, dict]] = None,
        stop_reason: Optional[str] = None,
        error_message: Optional[str] = None,
        source: Optional[str] = None,
    ) -> str:
        return self.append_message(
            "assistant", content, provider=provider, model=model, usage=usage,
            stop_reason=stop_reason, error_message=error_message, source=source,
        )

    def append_tool_result(
        self,
        content: Union[str, list[ContentBlock], list[dict]],
        *,
        tool_call_id: str,
        tool_name: Optional[str] = None,
        is_error: bool = False,
        details: Optional[dict] = None,
    ) -> str:
        return self.append_message(
            "toolResult", content, tool_call_id=tool_call_id,
            tool_name=tool_name, is_error=is_error, details=details,
        )

    def append_compaction(
        self,
        summary: str,
        *,
        first_kept_entry_id: Optional[str] = None,
        retained_tail: Optional[list[Union[MessageEntry, dict]]] = None,
        tokens_before: int = 0,
        read_files: Optional[list[str]] = None,
        modified_files: Optional[list[str]] = None,
    ) -> str:
        if retained_tail is not None:
            retained_tail = [
                m if isinstance(m, MessageEntry) else MessageEntry.model_validate(m)
                for m in retained_tail
            ]
        details = {}
        if read_files:
            details["read_files"] = read_files
        if modified_files:
            details["modified_files"] = modified_files
        entry = CompactionEntry(
            type="compaction",
            id=_new_id(),
            summary=summary,
            first_kept_entry_id=first_kept_entry_id,
            retained_tail=retained_tail,
            tokens_before=tokens_before,
            details=details or None,
        )
        return self._make(entry).id

    def append_branch_summary(
        self, summary: str, *, from_id: Optional[str] = None,
        read_files: Optional[list[str]] = None,
        modified_files: Optional[list[str]] = None,
    ) -> str:
        details = {}
        if read_files:
            details["read_files"] = read_files
        if modified_files:
            details["modified_files"] = modified_files
        entry = BranchSummaryEntry(
            type="branch_summary", id=_new_id(), summary=summary,
            from_id=from_id, details=details or None,
        )
        return self._make(entry).id

    def append_model_change(self, provider: Optional[str], model_id: Optional[str]) -> str:
        return self._make(ModelChangeEntry(
            type="model_change", id=_new_id(), provider=provider, model_id=model_id,
        )).id

    def append_thinking_level_change(self, thinking_level: str) -> str:
        return self._make(ThinkingLevelChangeEntry(
            type="thinking_level_change", id=_new_id(), thinking_level=thinking_level,
        )).id

    def append_custom_entry(self, custom_type: str, data: Any = None) -> str:
        return self._make(CustomEntry(
            type="custom", id=_new_id(), custom_type=custom_type, data=data,
        )).id

    def append_custom_message(
        self, custom_type: str, content: Union[str, list[ContentBlock], list[dict]],
        *, display: bool = True, details: Optional[dict] = None,
    ) -> str:
        return self.append_message(
            "custom_message", content, custom_type=custom_type,
            display=display, details=details,
        )

    def append_label_change(self, target_id: str, label: Optional[str] = None) -> str:
        return self._make(LabelEntry(
            type="label", id=_new_id(), target_id=target_id, label=label,
        )).id

    def append_session_info(self, name: str) -> str:
        return self._make(SessionInfoEntry(
            type="session_info", id=_new_id(), name=name,
        )).id

    # ── Context building ─────────────────────────────────────────────────────

    def build_context_entries(self) -> list[SessionEntry]:
        """Active branch (leaf -> root) with compaction checkpoints applied.

        The LAST ``compaction`` entry on the path wins: with ``retainedTail``
        it is a self-contained checkpoint (replaces everything before it);
        without one (legacy), the kept tail between ``first_kept_entry_id``
        and the compaction is re-included. Entries after the compaction follow.
        """
        path = self.get_branch()
        last_comp_idx = None
        for i, e in enumerate(path):
            if isinstance(e, CompactionEntry):
                last_comp_idx = i
        if last_comp_idx is None:
            return list(path)

        comp = path[last_comp_idx]
        out: list[SessionEntry] = [comp]
        if comp.retained_tail is None and comp.first_kept_entry_id:
            kept = [
                e for e in path[:last_comp_idx]
                if _entry_at_or_after(e, path[:last_comp_idx], comp.first_kept_entry_id)
            ]
            out.extend(kept)
        out.extend(path[last_comp_idx + 1:])
        return out

    def build_session_context(self) -> tuple[list[Any], Optional[str], Optional[str]]:
        """(messages, model, thinking_level) for the current leaf.

        Messages are LangChain ``BaseMessage`` objects via :func:`convert_to_llm`.
        """
        entries = self.build_context_entries()
        from langchain_core.messages import BaseMessage

        messages: list[BaseMessage] = []
        model: Optional[str] = None
        thinking_level: Optional[str] = None
        for e in entries:
            if isinstance(e, ModelChangeEntry):
                model = e.model_id or model
            elif isinstance(e, ThinkingLevelChangeEntry):
                thinking_level = e.thinking_level
        converted = convert_to_llm(entries)
        messages.extend(converted)
        return messages, model, thinking_level


# ────────────────────────────────────────────────────────────────────────────
# Conversion to LLM messages
# ────────────────────────────────────────────────────────────────────────────


def _entry_at_or_after(entry: SessionEntry, path: list[SessionEntry], target_id: str) -> bool:
    """Whether ``entry`` is ``target_id`` itself or appears after it in ``path``."""
    seen_target = entry.id == target_id
    for e in path:
        if e.id == target_id:
            seen_target = True
        elif e.id == entry.id and seen_target:
            return True
    return seen_target


# ────────────────────────────────────────────────────────────────────────────
# Process-wide session registry
# ────────────────────────────────────────────────────────────────────────────


_session_registry: dict[tuple[str, str], SessionManager] = {}
_session_registry_lock = threading.RLock()


def _list_session_files(session_dir: str) -> list[str]:
    if not os.path.isdir(session_dir):
        return []
    files = [
        os.path.join(session_dir, fn)
        for fn in os.listdir(session_dir)
        if fn.endswith(".jsonl")
    ]
    files.sort(key=lambda p: os.path.getmtime(p), reverse=True)
    return files


def find_recent_session(chat_id: str, workspace: str) -> Optional[str]:
    """Most recently modified session file for a (workspace, chat_id) pair."""
    sdir = session_dir_for(workspace)
    for path in _list_session_files(sdir):
        try:
            mgr = SessionManager.open(path)
        except Exception:
            continue
        if mgr.get_header().chat_id == chat_id:
            return path
    return None


def get_session(
    chat_id: str,
    workspace: str,
    *,
    parent_session: Optional[str] = None,
    delegation_depth: int = 0,
    chat_name: Optional[str] = None,
) -> SessionManager:
    """Get-or-create the session log for a (workspace, chat_id) pair.

    In-process runs reuse the cached manager; a fresh process resumes the most
    recent on-disk session for the pair (continue semantics). Never raises —
    callers must guard their own use if they depend on persistence.
    """
    key = (os.path.normcase(workspace), chat_id)
    with _session_registry_lock:
        mgr = _session_registry.get(key)
        if mgr is not None:
            return mgr
        path = None
        try:
            path = find_recent_session(chat_id, workspace)
        except Exception:
            path = None
        if path:
            mgr = SessionManager.open(path)
        else:
            mgr = SessionManager.create(
                workspace, chat_id=chat_id, name=chat_name,
                parent_session=parent_session, delegation_depth=delegation_depth,
            )
        _session_registry[key] = mgr
        return mgr


def clear_session_registry() -> None:
    with _session_registry_lock:
        _session_registry.clear()


def _block_text(block: ContentBlock) -> str:
    if block.type == "text":
        return block.text
    if block.type == "thinking":
        return block.thinking
    return ""


def _entry_text(content: Union[str, list[ContentBlock]], include_thinking: bool) -> str:
    if isinstance(content, str):
        return content
    parts = []
    for block in content:
        if block.type == "text":
            parts.append(block.text)
        elif block.type == "thinking" and include_thinking:
            parts.append(block.thinking)
    return "\n".join(parts)


def _entry_tool_calls(content: Union[str, list[ContentBlock]]) -> list[dict]:
    """Native-style tool calls from a message's toolCall blocks."""
    if isinstance(content, str):
        return []
    calls = []
    for block in content:
        if block.type == "toolCall":
            calls.append({
                "type": "function",
                "id": block.id,
                "name": block.name,
                "args": block.arguments,
            })
    return calls


def _entry_text_with_tool_blocks(content: Union[str, list[ContentBlock]]) -> str:
    """Text with tool calls rendered as ```tool JSON blocks (text-format)."""
    if isinstance(content, str):
        return content
    parts = []
    for block in content:
        if block.type == "text":
            parts.append(block.text)
        elif block.type == "toolCall":
            try:
                args_json = json.dumps(block.arguments, ensure_ascii=False)
            except Exception:
                args_json = json.dumps({})
            parts.append(
                f"```tool\n{{'tool': '{block.name}', 'args': {args_json}}}\n```"
            )
    return "\n".join(parts)


def convert_to_model_messages(
    entries: list[SessionEntry],
    *,
    include_thinking: bool = False,
    include_custom_messages: bool = True,
) -> list[Any]:
    """Convert session entries into pydantic-ai ModelMessage history (A6b).

    Native counterpart to :func:`convert_to_llm` — produces List[ModelMessage]
    (ModelRequest | ModelResponse) for direct use with Agent.run(message_history=).

    - user / custom_message -> ModelRequest(UserPromptPart)
    - assistant -> ModelResponse(TextPart + ThinkingPart + ToolCallPart)
    - toolResult -> ModelRequest(ToolReturnPart)
    - compaction -> ModelRequest summary + retainedTail messages
    - branch_summary -> ModelRequest summary
    """
    from pydantic_ai.messages import (
        ModelMessage,
        ModelRequest,
        ModelResponse,
        SystemPromptPart,
        TextPart,
        ThinkingPart,
        ToolCallPart,
        ToolReturnPart,
        UserPromptPart,
    )

    out: list[ModelMessage] = []
    for e in entries:
        if isinstance(e, CompactionEntry):
            if e.summary:
                out.append(ModelRequest(parts=[UserPromptPart(content=f"[Context summary]\n{e.summary}")]))
            if e.retained_tail:
                out.extend(convert_to_model_messages(
                    e.retained_tail,
                    include_thinking=include_thinking,
                    include_custom_messages=include_custom_messages,
                ))
            continue
        if isinstance(e, BranchSummaryEntry):
            if e.summary:
                out.append(ModelRequest(parts=[UserPromptPart(content=f"[Abandoned branch summary]\n{e.summary}")]))
            continue
        if isinstance(e, MessageEntry):
            if e.role == "user":
                out.append(ModelRequest(parts=[UserPromptPart(content=_entry_text(e.content, include_thinking))]))
            elif e.role == "custom_message":
                if include_custom_messages:
                    out.append(ModelRequest(parts=[UserPromptPart(content=_entry_text(e.content, include_thinking))]))
            elif e.role == "assistant":
                if e.stop_reason == "error" and not _entry_text(e.content, include_thinking):
                    continue
                parts: list[Any] = []
                text = _entry_text(e.content, include_thinking)
                if text:
                    parts.append(TextPart(content=text))
                # thinking blocks if requested
                if include_thinking and isinstance(e.content, list):
                    for block in e.content:
                        if getattr(block, "type", "") == "thinking":
                            parts.append(ThinkingPart(content=block.thinking))
                # tool calls
                for block in (e.content if isinstance(e.content, list) else []):
                    if getattr(block, "type", "") == "toolCall":
                        parts.append(ToolCallPart(tool_name=block.name, args=block.arguments, tool_call_id=block.id))
                if parts:
                    out.append(ModelResponse(parts=parts))
            elif e.role == "toolResult":
                if not e.tool_call_id:
                    continue
                out.append(ModelRequest(parts=[ToolReturnPart(
                    tool_name=e.tool_name or "",
                    content=_entry_text(e.content, include_thinking),
                    tool_call_id=e.tool_call_id,
                )]))
    return out


def convert_to_llm(
    entries: list[SessionEntry],
    *,
    include_thinking: bool = False,
    native_tool_calls: bool = True,
    include_custom_messages: bool = True,
) -> list[Any]:
    """Convert session entries into LangChain messages (the single projection).

    - user / custom_message -> ``HumanMessage``
    - assistant -> ``AIMessage`` (thinking stripped unless ``include_thinking``;
      toolCall blocks become native ``tool_calls`` when ``native_tool_calls``,
      otherwise rendered as ```tool text blocks so legacy parsers work)
    - toolResult -> ``ToolMessage`` (paired via ``tool_call_id``)
    - compaction -> ``HumanMessage`` summary + its ``retainedTail`` messages
    - branch_summary -> ``HumanMessage`` summary
    - everything else -> skipped
    """
    from langchain_core.messages import AIMessage, HumanMessage, ToolMessage

    out: list[Any] = []
    for e in entries:
        if isinstance(e, CompactionEntry):
            if e.summary:
                out.append(HumanMessage(
                    content=f"[Context summary]\n{e.summary}",
                    additional_kwargs={"role": "compaction"},
                ))
            if e.retained_tail:
                out.extend(convert_to_llm(
                    e.retained_tail,
                    include_thinking=include_thinking,
                    native_tool_calls=native_tool_calls,
                    include_custom_messages=include_custom_messages,
                ))
            continue
        if isinstance(e, BranchSummaryEntry):
            if e.summary:
                out.append(HumanMessage(
                    content=f"[Abandoned branch summary]\n{e.summary}",
                    additional_kwargs={"role": "branch_summary"},
                ))
            continue
        if isinstance(e, MessageEntry):
            if e.role == "user":
                out.append(HumanMessage(content=_entry_text(e.content, include_thinking)))
            elif e.role == "custom_message":
                if include_custom_messages:
                    out.append(HumanMessage(content=_entry_text(e.content, include_thinking)))
            elif e.role == "assistant":
                if e.stop_reason == "error" and not _entry_text(e.content, include_thinking):
                    continue
                tool_calls = _entry_tool_calls(e.content)
                text = _entry_text(e.content, include_thinking)
                if tool_calls and native_tool_calls:
                    out.append(AIMessage(content=text, tool_calls=tool_calls))
                else:
                    rendered = _entry_text_with_tool_blocks(e.content) if tool_calls else text
                    out.append(AIMessage(content=rendered))
            elif e.role == "toolResult":
                if not e.tool_call_id:
                    continue
                out.append(ToolMessage(
                    content=_entry_text(e.content, include_thinking),
                    tool_call_id=e.tool_call_id,
                    additional_kwargs={
                        "tool_name": e.tool_name or "",
                        "is_error": e.is_error,
                    },
                ))
    return out
