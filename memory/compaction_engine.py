"""Codex-parity compaction engine with a ledger pass.

Flow (single shared implementation for the sync agent checkpoints and the
async ProcessHistory processor):

* When the transcript fits one chunk, it is a straight Codex single-pass:
  full structured history + the compaction prompt -> final summary.
* When the pinned compactor cannot fit it (e.g. laguna 256k vs a 400k
  transcript), the OLD part becomes an intermediate chunk that is turned into
  a verbose factual LEDGER (never injected) while the RECENT part stays raw.
  The final pass then sees: system + repo map + project.md + ledger(s) +
  recent raw conversation -> one injected summary.
* Ledger chunks are capped so the final pass always has room for the ledger,
  and chunk sizing targets exactly two parts at the 400k trigger.
* Codex overflow behavior is the safety net: on a context-window error the
  oldest input item is dropped and the request retried until it fits.
* project.md and the repo map are re-anchored fresh and never summarized.
"""
from __future__ import annotations

import asyncio
import concurrent.futures
import json
import os
import time
from typing import Any, Awaitable

from pydantic_ai.messages import ModelRequest

from memory.compaction_prompts import (
    SUMMARY_PREFIX,
    chars_per_token,
    chunk_max_tokens,
    get_compaction_prompt,
    get_ledger_prompt,
    last_chunk_max_tokens,
    ledger_max_tokens,
    reserve_tokens,
    retained_user_message_tokens,
    seam_overlap_tokens,
    summary_max_tokens,
)

DEFAULT_COMPACTION_MODEL = "poolside/laguna-s-2.1:free"
DEFAULT_COMPACTION_WINDOW = 262144

_NO_CHANGE = ("NO_CHANGE", "NONE", "NO CHANGE")


# ── Token estimation ───────────────────────────────────────────────────────

def estimate_text_tokens(text: str) -> int:
    return max(1, int(len(text or "") / chars_per_token()))


def estimate_message_tokens(message: Any) -> int:
    """Chars/`chars_per_token` estimate for one ModelMessage (same units as
    the chunk caps, conservative for code/JSON)."""
    total = 0
    for part in getattr(message, "parts", []) or []:
        content = getattr(part, "content", "") or ""
        if isinstance(content, str):
            total += len(content)
        elif isinstance(content, (list, dict)):
            try:
                total += len(json.dumps(content, ensure_ascii=False))
            except Exception:
                total += len(str(content))
        args = getattr(part, "args", None)
        if isinstance(args, dict):
            try:
                total += len(json.dumps(args, ensure_ascii=False))
            except Exception:
                total += len(str(args))
        elif isinstance(args, str):
            total += len(args)
    return max(1, int(total / chars_per_token()))


def _is_system_message(message: Any) -> bool:
    if not isinstance(message, ModelRequest):
        return False
    return any(
        getattr(part, "part_kind", "") == "system-prompt"
        for part in getattr(message, "parts", []) or []
    )


def _has_tool_calls(message: Any) -> bool:
    return any(
        getattr(part, "part_kind", "") == "tool-call"
        for part in getattr(message, "parts", []) or []
    )


def _is_tool_return_only(message: Any) -> bool:
    if not isinstance(message, ModelRequest):
        return False
    parts = list(getattr(message, "parts", []) or [])
    return bool(parts) and all(
        getattr(part, "part_kind", "") == "tool-return" for part in parts
    )


def _drop_oldest(messages: list) -> list:
    """Codex ``remove_first_item()`` with tool-pair integrity.

    The system message (index 0) is never dropped. Dropping a response that
    carries tool calls also drops the immediately-following request when it
    holds only those tool returns, so no orphan return can reach the provider.
    Returns the input unchanged when only the system message remains: callers
    must treat that as "cannot trim further" (a ledger over an empty segment
    must fail, never silently produce an all-NONE ledger).
    """
    if len(messages) <= 1:
        return messages
    idx = 1 if _is_system_message(messages[0]) else 0
    if idx >= len(messages):
        idx = 0
    if len(messages) - 1 <= 1 and _is_system_message(messages[0]):
        return messages
    head = messages[idx]
    remaining = messages[:idx] + messages[idx + 1:]
    # Refuse to leave a conversation with no body at all.
    if _is_system_message(messages[0]) and len(remaining) <= 1:
        return messages
    if _has_tool_calls(head) and len(remaining) > idx:
        nxt = remaining[idx]
        if _is_tool_return_only(nxt):
            remaining = remaining[:idx] + remaining[idx + 1:]
    return remaining


# ── Chunk allocation (two parts at the 400k trigger) ──────────────────────

def allocate_chunks(
    messages: list,
    window: int,
    *,
    ledger_prompt_tokens: int = 0,
    final_prompt_tokens: int = 0,
    map_tokens: int = 0,
    project_doc_tokens: int = 0,
) -> tuple[Any, list[list]]:
    """Split structured history into ``[intermediate chunks..., last chunk]``.

    The last (raw) chunk is filled backwards to its own cap first; the older
    remainder is packed greedily into ledgered chunks. Caps are clamped to the
    compactor window so a smaller model simply yields more chunks.
    """
    if not messages:
        return None, []
    system = messages[0] if _is_system_message(messages[0]) else None
    system_tokens = estimate_message_tokens(system) if system is not None else 0
    body = list(messages[1:] if system is not None else messages)
    if not body:
        return system, []

    reserve = reserve_tokens()
    max1 = min(
        chunk_max_tokens(),
        max(1024, window - system_tokens - ledger_prompt_tokens - ledger_max_tokens() - reserve),
    )
    max_last = min(
        last_chunk_max_tokens(),
        max(1024, window - system_tokens - final_prompt_tokens - map_tokens - project_doc_tokens - reserve),
    )

    last: list = []
    last_tokens = 0
    idx = len(body) - 1
    while idx >= 0:
        tokens = estimate_message_tokens(body[idx])
        if last and last_tokens + tokens > max_last:
            break
        last.insert(0, body[idx])
        last_tokens += tokens
        idx -= 1
    if not last:
        # A single message exceeds the final cap; keep it and let the
        # trim-retry safety net shrink it at the provider boundary.
        last = [body[-1]]
        idx = len(body) - 2
    head = body[: idx + 1]

    chunks: list[list] = []
    current: list = []
    current_tokens = 0
    for message in head:
        tokens = estimate_message_tokens(message)
        if current and current_tokens + tokens > max1:
            chunks.append(current)
            current, current_tokens = [], 0
        current.append(message)
        current_tokens += tokens
    if current:
        chunks.append(current)
    chunks.append(last)
    return system, chunks


def ledger_allowance(
    chunks: list[list],
    window: int,
    *,
    system_tokens: int = 0,
    final_prompt_tokens: int = 0,
    map_tokens: int = 0,
    project_doc_tokens: int = 0,
    overlap_tokens: int = 0,
) -> int:
    """Per-ledger output cap: whatever the final pass can still hold after
    the raw last chunk and the seam-overlap carryover."""
    count = len(chunks) - 1
    if count <= 0:
        return 0
    last_tokens = sum(estimate_message_tokens(m) for m in chunks[-1])
    total = (
        window - system_tokens - final_prompt_tokens - map_tokens
        - project_doc_tokens - reserve_tokens() - last_tokens - overlap_tokens
    )
    return max(2048, min(ledger_max_tokens(), total // count))


def seam_overlap(chunk: list, max_tokens: int | None = None) -> list:
    """Raw tail of the previous chunk repeated for the final pass.

    Set to a budget that bridges the chunk boundary without re-carrying the
    whole segment. The selected suffix never starts with an orphaned tool
    return: if the head is a tool-return-only request, its preceding tool-call
    response is pulled in too.
    """
    if not chunk:
        return []
    if max_tokens is None:
        max_tokens = seam_overlap_tokens()
    if max_tokens <= 0:
        return []
    idx = len(chunk)
    total = 0
    while idx > 0:
        tokens = estimate_message_tokens(chunk[idx - 1])
        if total and total + tokens > max_tokens:
            break
        idx -= 1
        total += tokens
    while 0 < idx < len(chunk) and _is_tool_return_only(chunk[idx]):
        previous = chunk[idx - 1]
        if not _has_tool_calls(previous):
            break
        idx -= 1
        total += estimate_message_tokens(previous)
    return list(chunk[idx:])


# ── Prompts / requests ─────────────────────────────────────────────────────

def build_final_prompt(ledgers: list[str], project_doc: str, has_overlap: bool = False) -> str:
    parts = [get_compaction_prompt()]
    ledger_text = "\n\n---\n\n".join(text for text in ledgers if text)
    if ledger_text:
        parts.append(
            "AUTHORITATIVE LEDGER(S) from earlier parts of this same "
            "conversation (the raw later conversation follows):\n" + ledger_text
        )
        parts.append(
            "If a ledger conflicts with the raw later conversation, the later "
            "conversation wins: reconcile and keep only the final state. "
            "Preserve exact file paths, symbol names, commands, and error strings."
        )
    if has_overlap:
        parts.append(
            "The first conversation messages above repeat the end of the "
            "previous segment verbatim for continuity; they are already "
            "covered by the ledgers."
        )
    parts.append("CURRENT PROJECT.MD:\n" + (project_doc or "(not authored yet)"))
    parts.append("Return the [SUMMARY] section first, then the [PROJECT.MD] section.")
    return "\n\n".join(parts)


def _is_overflow(exc: BaseException) -> bool:
    try:
        from llm.llm import _is_context_overflow
        if _is_context_overflow(str(exc)):
            return True
    except Exception:
        pass
    low = str(exc).lower()
    return any(key in low for key in (
        "context length", "context window", "maximum context", "too many tokens",
        "prompt is too long", "input is too long", "reduce the length",
        "context_length_exceeded",
    ))


async def _request(model: Any, messages: list, max_tokens: int) -> tuple[str, str]:
    """Return ``(text, served_by)`` for one provider call."""
    from pydantic_ai import direct as _direct

    settings: dict = {"temperature": 0.2}
    if max_tokens and int(max_tokens) > 0:
        settings["max_tokens"] = int(max_tokens)
    response = await _direct.model_request(model, list(messages), model_settings=settings)
    text = ""
    for part in getattr(response, "parts", []) or []:
        if getattr(part, "part_kind", "") == "text":
            text += str(getattr(part, "content", "") or "")
    served = ""
    try:
        served = str(getattr(response, "model_name", "") or "")
        if not served:
            served = str(getattr(getattr(response, "model", None), "model_name", "") or "")
    except Exception:
        served = ""
    return text.strip(), served


async def _request_with_trim(
    model: Any, messages: list, prompt_text: str, max_tokens: int
) -> tuple[str, int, str]:
    """Send one pass; on a context-window error drop the oldest input item
    (Codex behavior) and retry. Returns ``(text, trimmed_count, served_by)``.

    Trimming stops before the conversation body disappears: a pass with no
    content left must fail (and be skipped) rather than answer from nothing.
    """
    from pydantic_ai.messages import ModelRequest, UserPromptPart

    prompt = ModelRequest(parts=[UserPromptPart(content=prompt_text)])
    work = list(messages)
    trimmed = 0
    retried = False
    while True:
        try:
            text, served = await _request(model, work + [prompt], max_tokens)
            return text, trimmed, served
        except Exception as exc:
            if _is_overflow(exc):
                if len(work) <= 1 or trimmed >= 500:
                    raise
                smaller = _drop_oldest(work)
                if len(smaller) == len(work):
                    raise  # cannot trim further without emptying the pass
                work = smaller
                trimmed += 1
                continue
            # A flaky free provider can answer with a malformed body after the
            # FallbackModel exhausted its chain; one delayed full-chain retry
            # is cheaper than losing the whole pass.
            if not retried:
                retried = True
                await asyncio.sleep(1.5)
                continue
            raise


def _looks_empty_ledger(text: str) -> bool:
    """A ledger that echoes the template with NONE everywhere is a failure."""
    stripped = (text or "").strip()
    if len(stripped) < 120:
        return True
    body = [
        line.strip().upper() for line in stripped.splitlines()
        if line.strip() and not line.strip().startswith("#")
    ]
    return bool(body) and all(line in ("NONE", "- NONE") for line in body)


async def _ledger_pass(
    model: Any, system: Any, chunk: list, prompt: str, max_tokens: int
) -> tuple[str, int, str]:
    messages = ([system] if system is not None else []) + list(chunk)
    effective_prompt = (
        prompt
        + f"\n\nBudget: up to ~{int(max_tokens):,} tokens. A dense ledger beats "
        "a short one; use the room rather than skimming, and do not repeat yourself."
    )
    text, trimmed, served = await _request_with_trim(
        model, messages, effective_prompt, max_tokens
    )
    if _looks_empty_ledger(text):
        # One fresh attempt; a different provider often serves it properly.
        try:
            retry_text, retry_trimmed, retry_served = await _request_with_trim(
                model, messages, effective_prompt, max_tokens
            )
            if not _looks_empty_ledger(retry_text):
                return retry_text, trimmed + retry_trimmed, retry_served
        except Exception:
            pass
    return text, trimmed, served


def _final_messages(system: Any, full_map: str, chunk: list) -> list:
    from pydantic_ai.messages import ModelRequest, UserPromptPart

    out: list = []
    if system is not None:
        out.append(system)
    if full_map:
        out.append(ModelRequest(parts=[UserPromptPart(
            content=f"<current-context>\n{full_map}\n</current-context>"
        )]))
    out.extend(chunk)
    return out


# ── Post-compaction retention (Codex: user messages only + summary last) ───

def collect_retained_user_messages(
    messages: list, max_tokens: int | None = None
) -> list[str]:
    """Exact user asks from the pre-compaction history, newest-first budgeted.

    Injected context blocks (map/project-memory/old summaries) are excluded -
    they are re-anchored fresh by the caller. A msg that crosses the budget is
    truncated at the start, mirroring Codex's retained-user-message policy.
    """
    from pydantic_ai.messages import ModelRequest, UserPromptPart

    if max_tokens is None:
        max_tokens = retained_user_message_tokens()
    texts: list[str] = []
    for message in messages:
        if not isinstance(message, ModelRequest):
            continue
        for part in getattr(message, "parts", []) or []:
            if not isinstance(part, UserPromptPart):
                continue
            content = part.content
            if not isinstance(content, str):
                continue
            text = content.strip()
            if not text:
                continue
            if text.startswith(("<current-context>", "<project-memory>", "[COMPACTION SUMMARY]")):
                continue
            if text.startswith(SUMMARY_PREFIX[:60]):
                continue
            texts.append(text)
    if max_tokens <= 0:
        return []
    kept: list[str] = []
    remaining = max_tokens
    for text in reversed(texts):
        tokens = estimate_text_tokens(text)
        if tokens <= remaining:
            kept.append(text)
            remaining -= tokens
        else:
            if remaining > 0:
                chars = max(1, int(remaining * chars_per_token()))
                kept.append(text[:chars] + "\n...[older part of this message trimmed]...")
            break
    kept.reverse()
    return kept


# ── Compactor model / project refresh ─────────────────────────────────────

def build_compactor() -> tuple[Any, int, str]:
    """Return ``(model, context_window, name)`` for the compaction model.

    ``COMPACTION_MODEL`` pins a free-pool catalog model (default laguna
    s-2.1, 256k). The remaining free pool stays as a fallback chain. When the
    pinned slug is unknown the router falls back to the largest-context model.
    """
    from llm.free_provider import build_free_fallback
    from llm.free_providers import model_context_window

    pinned = (os.getenv("COMPACTION_MODEL") or DEFAULT_COMPACTION_MODEL).strip()
    try:
        window = int(os.getenv("COMPACTION_CONTEXT_WINDOW", "0") or 0)
    except (TypeError, ValueError):
        window = 0
    if window <= 0:
        window = model_context_window(pinned, 0) if pinned else 0
    # NOTE: the free catalog has no "compaction" capability — the old code
    # queried it and silently got None, disabling the LLM compaction pass.
    # Route through "text" (every catalog model has it) with the pinned model
    # first, then the largest-context fallback.
    model = build_free_fallback(
        capability="text", prefer_reasoning=False, preferred_model=pinned
    )
    if model is None:
        model = build_free_fallback(
            capability="text", prefer_reasoning=False, prefer_context=True
        )
    if window <= 0:
        try:
            from llm.free_providers import _get_models_for_capability
            models = _get_models_for_capability("compaction", prefer_context=True)
            if models:
                window = int(models[0].get("context", 0) or 0)
        except Exception:
            window = 0
    return model, (window or DEFAULT_COMPACTION_WINDOW), (pinned or "auto")


async def _map_summary_pass(project_path: str) -> None:
    if os.getenv("PAI_MAP_SUMMARIES", "1") == "0" or not project_path:
        return
    try:
        from memory.repo_map_generator import RepoMapGenerator
        from memory.repo_summaries import ensure_file_summaries
        generator = RepoMapGenerator(project_path)
        await ensure_file_summaries(project_path, generator.key_files())
    except Exception:
        return


async def run_compaction_summary(messages: list, project_path: str) -> dict:
    """Full Codex-parity compaction preparation.

    Returns ``{summary, full_map, project_memory, user_messages, chunks,
    ledgers, trimmed, model, window}``. ``summary`` already carries the
    handoff prefix ("" when the LLM pass is disabled or failed). Never raises.
    """
    started = time.perf_counter()
    result: dict = {
        "summary": "", "full_map": "", "project_memory": "",
        "user_messages": [], "chunks": 0, "ledgers": 0, "trimmed": 0,
        "overlap": 0, "overlap_tokens": 0, "model": "", "window": 0,
        "served_models": [],
        "ledger_tokens": 0, "summary_tokens": 0, "retained_user_tokens": 0,
        "pre_tokens": 0, "post_context_tokens_est": 0, "duration_ms": 0,
        "failed_passes": 0,
    }
    try:
        result["pre_tokens"] = sum(estimate_message_tokens(m) for m in messages)
    except Exception:
        result["pre_tokens"] = 0
    try:
        result["user_messages"] = collect_retained_user_messages(messages)
    except Exception:
        result["user_messages"] = []

    project_doc = ""
    if project_path:
        try:
            from memory.memory_manager import read_project_doc
            project_doc = read_project_doc(project_path)
        except Exception:
            project_doc = ""

    # Repo map + authored memory are re-anchored fresh and are never
    # summarized. Refresh them first so the final pass can use the map and the
    # budget math sees real sizes.
    if project_path:
        await _map_summary_pass(project_path)
        try:
            from memory.memory_manager import fresh_full_map
            result["full_map"] = fresh_full_map(project_path, include_summaries=True)
        except Exception:
            result["full_map"] = ""
        try:
            from memory.memory_manager import load_project_memory
            result["project_memory"] = load_project_memory(project_path)
        except Exception:
            result["project_memory"] = ""

    if os.getenv("PAI_COMPACTION_LLM", "1") != "0" and messages:
        try:
            model, window, name = build_compactor()
            result["model"], result["window"] = name, window

            ledger_prompt = get_ledger_prompt()
            final_prompt_base = get_compaction_prompt()
            map_text = str(result.get("full_map") or "")
            system, chunks = allocate_chunks(
                messages,
                window,
                ledger_prompt_tokens=estimate_text_tokens(ledger_prompt),
                final_prompt_tokens=estimate_text_tokens(final_prompt_base),
                map_tokens=estimate_text_tokens(map_text),
                project_doc_tokens=estimate_text_tokens(project_doc),
            )
            result["chunks"] = len(chunks)
            result["ledgers"] = max(0, len(chunks) - 1)

            if chunks:
                system_tokens = estimate_message_tokens(system) if system is not None else 0
                ledgers: list[str] = []
                served_models: list[str] = []
                trimmed_total = 0
                failed_passes = 0
                overlap: list = []
                overlap_tokens = 0
                if len(chunks) > 1:
                    # Seam overlap: repeat the raw tail of the last old chunk at
                    # the start of the final pass so the boundary is visible,
                    # not just described by a ledger.
                    overlap = seam_overlap(chunks[-2])
                    overlap_tokens = sum(estimate_message_tokens(m) for m in overlap)
                    per_ledger = ledger_allowance(
                        chunks, window,
                        system_tokens=system_tokens,
                        final_prompt_tokens=estimate_text_tokens(final_prompt_base),
                        map_tokens=estimate_text_tokens(map_text),
                        project_doc_tokens=estimate_text_tokens(project_doc),
                        overlap_tokens=overlap_tokens,
                    )
                    # Ledger passes run sequentially: the compactor is a single
                    # pinned free model, and concurrent calls trip its rate
                    # limits / flaky fallbacks. One failed ledger must not kill
                    # the summary - skip it and keep the rest.
                    for chunk in chunks[:-1]:
                        try:
                            text, trimmed, served = await _ledger_pass(
                                model, system, chunk, ledger_prompt, per_ledger
                            )
                        except Exception as exc:
                            failed_passes += 1
                            print(f"[COMPACTION] ledger pass failed: {type(exc).__name__}")
                            continue
                        if served:
                            served_models.append(served)
                        if text and not _looks_empty_ledger(text):
                            ledgers.append(text)
                            trimmed_total += trimmed
                        else:
                            failed_passes += 1
                            print("[COMPACTION] ledger pass returned an empty ledger")
                result["overlap"] = len(overlap)
                result["overlap_tokens"] = overlap_tokens
                result["ledger_tokens"] = sum(estimate_text_tokens(t) for t in ledgers)
                # Diagnostic only (never injected, excluded from metrics JSONL).
                result["ledger_texts"] = list(ledgers)

                final_prompt = build_final_prompt(ledgers, project_doc, has_overlap=bool(overlap))
                final_input = _final_messages(system, map_text, overlap + list(chunks[-1]))
                try:
                    text, trimmed, served = await _request_with_trim(
                        model, final_input, final_prompt, summary_max_tokens()
                    )
                    if served:
                        served_models.append(served)
                except Exception as exc:
                    failed_passes += 1
                    print(f"[COMPACTION] final pass failed: {type(exc).__name__}")
                    text, trimmed = "", 0
                trimmed_total += trimmed
                result["trimmed"] = trimmed_total
                result["failed_passes"] = failed_passes
                result["served_models"] = served_models

                summary, project_update = split_compaction_blocks(text)
                if not summary:
                    summary = text.strip()
                if summary:
                    result["summary"] = SUMMARY_PREFIX + "\n" + summary
                if (
                    project_update
                    and project_path
                    and project_update.strip().upper() not in _NO_CHANGE
                ):
                    try:
                        from memory.memory_manager import update_project_memory
                        update_project_memory(project_path, project_update)
                    except Exception:
                        pass
        except Exception:
            pass

    # Re-read authored memory AFTER any checker update so the post-compaction
    # context is fresh (changed or not).
    if project_path:
        try:
            from memory.memory_manager import load_project_memory
            result["project_memory"] = load_project_memory(project_path)
        except Exception:
            pass

    # Metrics: sizes + duration, appended to a per-project JSONL so compaction
    # quality can be tracked/regressed over time.
    try:
        result["summary_tokens"] = estimate_text_tokens(result["summary"])
        result["retained_user_tokens"] = sum(
            estimate_text_tokens(t) for t in (result.get("user_messages") or [])
        )
        result["post_context_tokens_est"] = (
            result["summary_tokens"] + result["retained_user_tokens"]
            + estimate_text_tokens(str(result.get("full_map") or ""))
            + estimate_text_tokens(str(result.get("project_memory") or ""))
        )
    except Exception:
        pass
    result["duration_ms"] = int((time.perf_counter() - started) * 1000)
    _emit_metrics(project_path, result)
    return result


def _emit_metrics(project_path: str, result: dict) -> None:
    """Append one compaction metrics line per project (best-effort)."""
    if not project_path:
        return
    try:
        import datetime as _dt

        record = {k: v for k, v in result.items() if k not in ("full_map", "ledger_texts", "summary")}
        record["ts"] = _dt.datetime.now().isoformat(timespec="seconds")
        path = os.path.join(project_path, ".agent-memory", "compaction_metrics.jsonl")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False, default=str) + "\n")
    except Exception:
        return


def split_compaction_blocks(text: str) -> tuple[str, str]:
    """Split a final-pass reply into (summary, project.md update).

    Tolerant: a reply without tags is treated as the summary; the project
    block is empty when absent.
    """
    up = (text or "").upper()
    i_sum = up.find("[SUMMARY]")
    i_pm = up.find("[PROJECT.MD]")
    if i_sum >= 0:
        start = i_sum + len("[SUMMARY]")
        end = i_pm if i_pm > start else len(text)
        summary = text[start:end].strip()
    elif i_pm > 0:
        summary = text[:i_pm].strip()
    else:
        summary = (text or "").strip()
    project = text[i_pm + len("[PROJECT.MD]"):].strip() if i_pm >= 0 else ""
    return summary, project


def compact_history_with_summary(
    history: list, *, project_path: str = "", policy: Any = None
) -> tuple[list, dict]:
    """Sync checkpoint entry for the agent loops: summarize + re-anchor.

    Returns ``(new_history, engine_info)``. Never raises; on engine failure the
    caller still gets a valid history with the deterministic placeholder.
    """
    from llm.model_history import compact_history_native

    try:
        result = run_sync(run_compaction_summary(history, project_path))
    except Exception:
        result = {}
    summary = str(result.get("summary") or "")
    new_history = compact_history_native(
        history,
        policy=policy,
        full_project_map=(result.get("full_map") or None),
        summary_text=(summary or None),
        project_memory_text=(result.get("project_memory") or None),
        user_messages=(result.get("user_messages") or None),
    )
    return new_history, result


def run_sync(coro: Awaitable) -> Any:
    """Run an async compaction engine call from sync callers safely.

    Uses ``asyncio.run`` when no loop is active; inside a running loop the
    coroutine executes in a dedicated thread with its own loop, so the sync
    Dev checkpoint path cannot deadlock the caller.
    """
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return asyncio.run(coro)
    with concurrent.futures.ThreadPoolExecutor(
        max_workers=1, thread_name_prefix="compaction"
    ) as pool:
        return pool.submit(asyncio.run, coro).result()
