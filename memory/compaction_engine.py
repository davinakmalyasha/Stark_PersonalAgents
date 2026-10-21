"""Single-pass compaction engine using the chat model.

Codex/OpenCode-style behavior:

* The full transcript is sent once to the same model that is driving the chat.
* The model returns one handoff summary.
* History is rebuilt deterministically as
  ``system -> project memory -> current map -> retained user asks -> summary``.
* project.md and the repo map are re-anchored fresh and are never summarized.

There is no separate compactor model, ledger pass, chunk allocation, or
seam-overlap pass.
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
    get_compaction_prompt,
    retained_user_message_tokens,
    summary_max_tokens,
)

_NO_CHANGE = ("NO_CHANGE", "NONE", "NO CHANGE")


# ── Token estimation ───────────────────────────────────────────────────────

def estimate_text_tokens(text: str) -> int:
    return max(1, int(len(text or "") / chars_per_token()))


def estimate_message_tokens(message: Any) -> int:
    """Chars/`chars_per_token` estimate for one ModelMessage."""
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


# ── Model resolution ──────────────────────────────────────────────────────

def _window_for_model(model_id: str) -> int:
    try:
        from llm.model_history import window_for_model

        return int(window_for_model(model_id) or 0)
    except Exception:
        return 0


def _resolve_model(
    role: str,
    deps: Any,
    model: Any,
    model_id: str = "",
    backend: str = "",
) -> tuple[Any, str, int]:
    """Return ``(model, model_id, window)`` for the summary request.

    A concrete model object (the one used by the chat) always wins. When only
    a role is available, resolve the same role plan and build that model.
    """
    if model is not None and not isinstance(model, str):
        mid = str(model_id or getattr(model, "model_name", "") or "")
        window = int(getattr(deps, "context_window", 0) or 0) or _window_for_model(mid)
        return model, mid, window

    ref = model if isinstance(model, str) else ""
    plan: dict = {}
    chosen_backend = str(backend or "").strip().lower()
    if not chosen_backend:
        try:
            from llm.provider_modes import get_role_plan

            plan = get_role_plan(role or "developer")
            chosen_backend = str(plan.get("backend") or "paid").strip().lower()
        except Exception:
            chosen_backend = "paid"

    if chosen_backend == "free":
        try:
            from llm.free_provider import build_free_fallback

            prefer = plan.get("think_bias")
            if prefer is None:
                try:
                    from agents.workspace_registry import thinking_prefer_reasoning

                    prefer = thinking_prefer_reasoning(
                        str(plan.get("thinking") or "inherit")
                    )
                except Exception:
                    prefer = None
            free_model = build_free_fallback(
                capability="text",
                prefer_reasoning=prefer,
            )
            mid = ref or str(plan.get("model") or "free-pool")
            return free_model, mid, _window_for_model(mid)
        except Exception:
            return None, ref or "free-pool", 0

    from llm.native_tools import build_native_model

    ref = ref or str(plan.get("model") or "")
    if not ref:
        ref = "deepseek:v4.1-flash"
    return build_native_model(
        role=role or "developer",
        model=ref,
        deps=deps,
        temp=0.2,
    )


# ── One request ───────────────────────────────────────────────────────────

async def _request(
    model: Any, messages: list, prompt_text: str, max_tokens: int
) -> tuple[str, str]:
    """Return ``(text, served_by)`` for the single summary request."""
    from pydantic_ai import direct as _direct
    from pydantic_ai.messages import UserPromptPart

    settings: dict = {"temperature": 0.2}
    if max_tokens and int(max_tokens) > 0:
        settings["max_tokens"] = int(max_tokens)
    request = ModelRequest(parts=[UserPromptPart(content=prompt_text)])
    response = await _direct.model_request(
        model,
        list(messages) + [request],
        model_settings=settings,
    )
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


# ── Post-compaction retention ────────────────────────────────────────────

def collect_retained_user_messages(
    messages: list, max_tokens: int | None = None
) -> list[str]:
    """Exact user asks from the pre-compaction history, newest-first budgeted.

    Injected context blocks (map/project-memory/old summaries) are excluded;
    they are re-anchored fresh by the caller.
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


# ── Project refresh ───────────────────────────────────────────────────────

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


async def run_compaction_summary(
    messages: list,
    project_path: str,
    *,
    policy: Any = None,
    role: str = "",
    deps: Any = None,
    model: Any = None,
    model_id: str = "",
    backend: str = "",
) -> dict:
    """Run one same-model handoff summary.

    Returns ``{summary, full_map, project_memory, user_messages, model,
    window, ...}``. ``summary`` already carries the handoff prefix. Never
    raises.
    """
    started = time.perf_counter()
    result: dict = {
        "summary": "", "full_map": "", "project_memory": "",
        "user_messages": [], "chunks": 0, "ledgers": 0, "trimmed": 0,
        "overlap": 0, "overlap_tokens": 0, "model": "", "window": 0,
        "served_models": [], "ledger_tokens": 0, "summary_tokens": 0,
        "retained_user_tokens": 0, "pre_tokens": 0,
        "post_context_tokens_est": 0, "duration_ms": 0, "failed_passes": 0,
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
            model_obj, name, window = _resolve_model(
                role, deps, model, model_id, backend=backend
            )
            result["model"], result["window"] = name, window
            if model_obj is None:
                raise RuntimeError("no compaction model available")
            prompt = get_compaction_prompt()
            if project_doc:
                prompt += "\n\nCURRENT PROJECT.MD:\n" + project_doc
            text, served = await _request(
                model_obj, messages, prompt, summary_max_tokens()
            )
            result["chunks"] = 1 if text else 0
            result["served_models"] = [served] if served else []
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
        except Exception as exc:
            result["failed_passes"] = 1
            result["error"] = f"{type(exc).__name__}: {exc}"
            print(f"[COMPACTION] summary pass failed: {type(exc).__name__}")

    if project_path:
        try:
            from memory.memory_manager import load_project_memory

            result["project_memory"] = load_project_memory(project_path)
        except Exception:
            pass

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

        record = {
            k: v for k, v in result.items()
            if k not in ("full_map", "summary")
        }
        record["ts"] = _dt.datetime.now().isoformat(timespec="seconds")
        path = os.path.join(project_path, ".agent-memory", "compaction_metrics.jsonl")
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "a", encoding="utf-8") as f:
            f.write(json.dumps(record, ensure_ascii=False, default=str) + "\n")
    except Exception:
        return


def split_compaction_blocks(text: str) -> tuple[str, str]:
    """Split a summary reply into (summary, project.md update)."""
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
    history: list,
    *,
    project_path: str = "",
    policy: Any = None,
    role: str = "",
    deps: Any = None,
    model: Any = None,
    model_id: str = "",
    backend: str = "",
) -> tuple[list, dict]:
    """Sync checkpoint entry for the agent loops: summarize + re-anchor.

    Returns ``(new_history, engine_info)``. Never raises; on engine failure the
    caller still gets a valid history with the deterministic placeholder.
    """
    from llm.model_history import compact_history_native

    try:
        result = run_sync(run_compaction_summary(
            history,
            project_path,
            policy=policy,
            role=role,
            deps=deps,
            model=model,
            model_id=model_id,
            backend=backend,
        ))
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
    """Run an async compaction engine call from sync callers safely."""
    try:
        asyncio.get_running_loop()
    except RuntimeError:
        return asyncio.run(coro)
    with concurrent.futures.ThreadPoolExecutor(
        max_workers=1, thread_name_prefix="compaction"
    ) as pool:
        return pool.submit(asyncio.run, coro).result()
