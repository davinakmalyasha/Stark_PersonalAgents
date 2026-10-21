"""Compaction-time LLM one-liners for projectMap Key files.

Generated ONLY in the compaction re-anchor path (llm/compaction_native.py),
never on fresh runs and never in the per-edit delta path — fresh runs stay
strictly deterministic. The sidecar is a generation cache keyed by body hash;
``RepoMapGenerator.generate_map(include_summaries=True)`` overlays an entry
only when its stored hash still matches the file exactly, so a stale summary
can never be injected as fact.

Failure is always silent: free-pool exhaustion, timeouts, or unparsable
output simply leave the deterministic role/docstring label in place.
"""
import asyncio
import hashlib
import json
import os
import re

_SIDECAR_NAME = "file_summaries.json"
_MAX_SUMMARY_CHARS = 150
_MAX_HEAD_CHARS = 400
_MAX_PROMPT_CHARS = 16000


def _sidecar_path(project_path: str) -> str:
    return os.path.join(project_path, ".agent-memory", _SIDECAR_NAME)


def _file_body(project_path: str, rel_path: str) -> tuple[str, str]:
    """(content, sha1-body-hash) for one repo-relative file."""
    try:
        full = os.path.join(project_path, rel_path.replace("/", os.sep))
        with open(full, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
    except Exception:
        return "", ""
    digest = hashlib.sha1(content.encode("utf-8", "replace")).hexdigest()[:16]
    return content, digest


def load_summaries(project_path: str) -> dict:
    """Read the sidecar as {path: {hash, summary}}; never raises."""
    try:
        with open(_sidecar_path(project_path), "r", encoding="utf-8") as f:
            value = json.loads(f.read())
        return value if isinstance(value, dict) else {}
    except Exception:
        return {}


def _write_summaries(project_path: str, data: dict) -> bool:
    try:
        os.makedirs(os.path.dirname(_sidecar_path(project_path)), exist_ok=True)
        tmp = _sidecar_path(project_path) + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            json.dump(data, f, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
        os.replace(tmp, _sidecar_path(project_path))
        return True
    except Exception:
        return False


def _needing_summary(project_path: str, key_files: list[str], sidecar: dict) -> list[tuple[str, str, str]]:
    """Key files whose body changed (or never had) a matching summary."""
    out: list[tuple[str, str, str]] = []
    for rel in key_files:
        content, digest = _file_body(project_path, rel)
        if not content or not digest:
            continue
        entry = sidecar.get(rel)
        if isinstance(entry, dict) and entry.get("hash") == digest and entry.get("summary"):
            continue
        out.append((rel, content, digest))
    return out


def _build_prompt(pending: list[tuple[str, str, str]]) -> str:
    blocks: list[str] = []
    used = 0
    for rel, content, _digest in pending:
        head = content[:_MAX_HEAD_CHARS]
        block = f"### {rel}\n{head}"
        if used + len(block) > _MAX_PROMPT_CHARS:
            break
        blocks.append(block)
        used += len(block)
    return (
        "For each source file below, write ONE plain line describing what the "
        "file does and when an engineer would open it. Output format, exactly "
        "one line per file, no markdown, no bullets, max 20 words:\n"
        "<path> — <summary>\n\n" + "\n\n".join(blocks)
    )


def _parse_summaries(text: str, wanted: set[str]) -> dict[str, str]:
    """Parse `path — summary` / `path: summary` lines; tolerate list prefixes."""
    parsed: dict[str, str] = {}
    by_basename: dict[str, str] = {}
    for line in str(text or "").splitlines():
        line = line.strip().lstrip("-*# ").strip()
        m = re.match(r"^`?([\w./\\-]+)`?\s*[—:\-]\s*(.+)$", line)
        if not m:
            continue
        path = m.group(1).replace("\\", "/").lstrip("./")
        summary = " ".join(m.group(2).split())[:_MAX_SUMMARY_CHARS]
        if not summary:
            continue
        if path in wanted:
            parsed[path] = summary
        else:
            by_basename[os.path.basename(path)] = summary
    if len(parsed) < len(wanted):
        for path in wanted:
            if path in parsed:
                continue
            found = by_basename.get(os.path.basename(path))
            if found:
                parsed[path] = found
    return parsed


async def ensure_file_summaries(
    project_path: str,
    key_files: list[str],
    timeout: float = 8.0,
) -> dict:
    """Refresh the sidecar for hash-changed Key files with ONE batched
    free-pool request. Returns the (possibly unchanged) sidecar; never raises
    and never blocks compaction beyond ``timeout``."""
    sidecar = load_summaries(project_path)
    if not project_path or not key_files:
        return sidecar
    pending = _needing_summary(project_path, key_files, sidecar)
    if not pending:
        return sidecar
    try:
        from llm.free_provider import build_free_fallback
        # The catalog has no "compaction" capability; route through "text".
        fallback = build_free_fallback(
            capability="text", prefer_reasoning=False,
            max_context_tokens=1000000,
        )
    except Exception:
        fallback = None
    if fallback is None:
        return sidecar

    prompt = _build_prompt(pending)
    try:
        from pydantic_ai import direct as _direct
        from pydantic_ai.messages import ModelRequest, UserPromptPart
        request = [ModelRequest(parts=[UserPromptPart(content=prompt)])]
        resp = await asyncio.wait_for(
            _direct.model_request(fallback, request, model_settings={"temperature": 0.2}),
            timeout=timeout,
        )
        text = ""
        for part in getattr(resp, "parts", []) or []:
            if getattr(part, "part_kind", "") == "text":
                text += str(getattr(part, "content", "") or "")
    except Exception:
        return sidecar

    parsed = _parse_summaries(text, {rel for rel, _c, _h in pending})
    if not parsed:
        return sidecar
    for rel, _content, digest in pending:
        summary = parsed.get(rel)
        if summary:
            sidecar[rel] = {"hash": digest, "summary": summary}
    # Bound the sidecar to the current Key-file vocabulary.
    keep = set(key_files)
    sidecar = {path: entry for path, entry in sidecar.items() if path in keep}
    _write_summaries(project_path, sidecar)
    return sidecar
