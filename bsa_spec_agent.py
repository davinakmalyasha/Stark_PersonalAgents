"""
bsa_spec_agent.py — incremental BSA spec generation routed per the UI dropdown.

Follows the same provider selection as the rest of the harness:

  - Dropdown = OpenCode Zen  -> BSA calls run through OpenCode Go
    (opencode.ai/zen/go/v1, DeepSeek V4 Flash, unlimited with the Go
    subscription), falling back to the free pool on error.
  - Dropdown = DeepSeek API  -> BSA always uses the free pool (zero cost,
    never the paid DeepSeek API).
  - No selection             -> free pool (zero cost).

Free path pipeline (unchanged):

  1. RESEARCH  — harness-driven digest: a reasoning free model suggests which existing
                 files to read; the harness reads them (model never calls tools, which
                 is fragile on free models). Greenfield projects get "design from scratch".
  2. WALK      — an 8-section SPEC checklist, one model call per section, each
                 appended to `specs/SPEC-<slug>.md`. The spec file on disk is the
                 checkpoint: a provider failure loses at most one section, and the next
                 run resumes from the next unwritten section.
  3. ADAPTIVE  — section difficulty maps to free_agent's capability router
                 (reasoning=planning sections, coding=technical, text=mechanical).
                 A too-short/empty section is escalated to a reasoning model before
                 giving up. Providers fall back automatically across the pool (RPM
                 isolation), and 429s get a 30s cooldown inside free_agent.
"""
import json
import os
import re
import time

from free_agent import FreeAgent

# Free providers allowed for BSA. Cloudflare (no reasoning/coding) and Z.ai GLM-Flash
# (no reasoning) are excluded — unreliable for long structured specs. The remaining
# pool is used adaptively by capability through free_agent's existing priority router.
BSA_PROVIDERS = ["groq", "google", "mistral", "nvidia", "openrouter", "kilo", "opencode", "cline"]

# Names that route through this free BSA agent (mirrors the alias map in tools._load_subagents).
_BSA_SPEC_ALIASES = {
    "BSA", "BA", "SA",
    "BSA-Requirements", "BSA-Schema", "BSA-API",
    "BA-GapAnalyzer", "BA-Gherkin", "SA-Database", "SA-API",
    "Analytics", "Designer",
}

_NA_PREFIX = "N/A"
_PLACEHOLDER_MARK = "⚠️"
_MAX_TOKENS = {"text": 6000, "coding": 8000, "reasoning": 10000}
_MAX_SECTIONS = 8

# Every section must handle both modes: a FEATURE on existing code (base decisions on
# the research digest, match existing conventions) or a GREENFIELD project (design from
# scratch). Sections that don't apply output "N/A: <why>" rather than inventing content.
SECTION_DEFS = [
    {
        "num": 1,
        "title": "Overview & Stakeholders",
        "capability": "text",
        "instructions": (
            "Produce the opening sections of the SPEC:\n"
            "## 1. Executive Summary — one paragraph: what we are building, why, for whom, "
            "and what problem it solves.\n"
            "## 2. Stakeholders & Users — markdown table (Role | Name/Title | Interest | Impact).\n"
            "## 3. Scope — In Scope / Out of Scope / Assumptions / Dependencies bullet lists.\n"
            "Be concrete. If this is a FEATURE on existing code, name the existing project "
            "pieces it touches. If GREENFIELD, describe the new system."
        ),
    },
    {
        "num": 2,
        "title": "Functional Requirements & Business Rules",
        "capability": "reasoning",
        "instructions": (
            "## 4. Functional Requirements — markdown table (ID | Requirement | Priority "
            "(Must/Should/Could) | Owner | Acceptance Criterion). Every requirement needs a "
            "measurable, testable acceptance criterion.\n"
            "## 5. Business Rules — markdown table (ID | Condition | Action | Exception).\n"
            "Enumerate edge cases explicitly: empty, missing, invalid, duplicate, concurrent."
        ),
    },
    {
        "num": 3,
        "title": "Data Model",
        "capability": "reasoning",
        "instructions": (
            "## 6. Data Model — only if the feature stores state; otherwise output N/A.\n"
            "### Entity-Relationship Overview (Mermaid ERD)\n"
            "### Tables — DDL with column types, constraints, defaults, indexes (each index "
            "justified by the query it serves).\n"
            "### Enums & Lookups\n"
            "### Migration Notes — breaking changes, migration plan, rollback.\n"
            "If a FEATURE on existing code, extend the existing schema and reference it from "
            "the research digest. If GREENFIELD, design the schema from scratch."
        ),
    },
    {
        "num": 4,
        "title": "API Contract",
        "capability": "coding",
        "instructions": (
            "## 7. API Contract — only if the feature exposes an API; otherwise output N/A.\n"
            "### Endpoints table (Method | Path | Auth | Rate Limit | Description)\n"
            "### Request/Response DTOs (exact type definitions)\n"
            "### Error Responses (error codes per endpoint)\n"
            "### Pagination, Filtering, Sorting\n"
            "Match existing API conventions if this is a FEATURE on existing code."
        ),
    },
    {
        "num": 5,
        "title": "Architecture & Directory Plan",
        "capability": "reasoning",
        "instructions": (
            "## 8. Architecture\n"
            "### Directory Structure — framework-specific layout with responsibility "
            "annotations; list the exact files to create/modify for this feature.\n"
            "### Key Design Decisions — table (Decision | Rationale | Alternatives Considered)\n"
            "### Resilience Strategy — transaction boundaries, idempotency, circuit breakers, retry policy\n"
            "### Communication Patterns — sync / async / real-time\n"
            "If a FEATURE on existing code, describe how it plugs into the existing "
            "architecture and reference existing files from the research digest."
        ),
    },
    {
        "num": 6,
        "title": "Non-Functional Requirements",
        "capability": "coding",
        "instructions": (
            "## 9. Non-Functional Requirements\n"
            "### Performance — table (Metric | Target | Measurement)\n"
            "### Security — table (Requirement | Implementation): auth, authorization/RBAC, "
            "data classification, PII, rate limiting\n"
            "### Reliability — table (Metric | Target): uptime SLA, RTO, RPO, backups\n"
            "### Observability — logging, metrics, alerting, tracing\n"
            "Right-size to the project — do not design for 1M users when the current count is "
            "100; add a 'Scaling Path' note instead."
        ),
    },
    {
        "num": 7,
        "title": "Acceptance Criteria",
        "capability": "reasoning",
        "instructions": (
            "## 10. Acceptance Criteria\n"
            "### Gherkin Scenarios — organized by feature; happy path, error path, and edge "
            "cases with real concrete data (alice@example.com, 25 orders, $49.99).\n"
            "### Test Cases — table (ID | Scenario | Input | Expected Output | Type "
            "Unit/Integration/E2E).\n"
            "Cover: happy, error, empty, auth failure, not found, conflict, rate limit."
        ),
    },
    {
        "num": 8,
        "title": "Design System & Open Questions",
        "capability": "text",
        "instructions": (
            "## 11. Design System — only if the feature has a UI; otherwise output N/A.\n"
            "### Design Direction (concept, mood, font/color/spatial direction in words, no hex)\n"
            "### Breakpoints (sm/md/lg/xl/2xl)\n"
            "### Component Hierarchy (atoms to molecules to organisms to templates to pages)\n"
            "### Accessibility (WCAG AA checklist)\n"
            "## 12. Open Questions — table (# | Question | Asked To | Status). List every "
            "assumption or unknown that needs clarification.\n"
            "End with the line: **Handoff**: Ready for Dev implementation."
        ),
    },
]

BSA_SYSTEM_PROMPT = """You are BSA, a Business Systems Analyst producing an implementation-ready
specification (SPEC.md) that a Developer can execute without asking clarifying questions.

RULES:
- You are given ONE section of the specification at a time. Write ONLY that section's content.
- Be precise and concrete. No placeholders, no "TBD", no vague language. "Fast" -> "p95 < 200ms".
  "Secure" -> "JWT auth, 15min expiry". "Scalable" -> "Supports 1,000 concurrent users".
- If the section genuinely does not apply to this task, respond with exactly: N/A: <one sentence why>.
- Do not repeat content already in the SPEC SO FAR. Do not add a preamble, reasoning, or commentary.
- If this spec is for a FEATURE added to existing code, base every decision on the existing files
  in the research digest and match existing conventions. If it is a GREENFIELD project, design from scratch.
- Every assumption or unknown goes in the Open Questions section. Never silently guess.
"""


def _log(msg: str) -> None:
    try:
        print(msg, flush=True)
    except Exception:
        pass
    try:
        from state_sync import safe_append_live_log
        safe_append_live_log(msg)
    except Exception:
        pass


def _record_free_call(kind: str, provider: str, capability: str, chars: int) -> None:
    """Free-model usage telemetry. Appends one JSON line to BSA_TELEMETRY_FILE if set.

    The paid path records into token_usage via update_token_stats; the free BSA path
    has no cost but should still be observable, so it logs provider/capability/size here.
    """
    path = os.environ.get("BSA_TELEMETRY_FILE", "")
    if not path:
        return
    try:
        rec = {
            "kind": kind,
            "provider": provider or "",
            "capability": capability or "",
            "chars": chars,
            "ts": time.strftime("%Y-%m-%dT%H:%M:%S"),
        }
        with open(path, "a", encoding="utf-8") as f:
            f.write(json.dumps(rec) + "\n")
    except Exception:
        pass


# ----------------------------------------------------------------------
#  FreeAgent instance (restricted to the BSA pool; general chat unaffected)
# ----------------------------------------------------------------------

_bsa_agent = None


def _get_bsa_agent() -> FreeAgent:
    global _bsa_agent
    if _bsa_agent is None:
        _bsa_agent = FreeAgent(enabled_providers=BSA_PROVIDERS)
    return _bsa_agent


def _selected_provider() -> str:
    """Return the UI dropdown selection: 'deepseek' | 'opencode' | ''.

    Same resolution order as llm._resolve_llm_provider: shared_state first,
    then the persisted llm_provider.json (survives restarts/subprocesses).
    """
    try:
        from state_sync import safe_get_state
        p = (safe_get_state().get("llm_provider") or "").strip().lower()
        if p in ("deepseek", "opencode"):
            return p
    except Exception:
        pass
    try:
        from llm import _load_persisted_provider
        p = (_load_persisted_provider() or "").strip().lower()
        if p in ("deepseek", "opencode"):
            return p
    except Exception:
        pass
    return ""


# Provider used by the most recent _chat_any call (for summary telemetry).
_last_used_provider = ""


def _chat_any(
    prompt: str,
    capability: str = "text",
    max_tokens: int = 6000,
    system_prompt: str = BSA_SYSTEM_PROMPT,
) -> tuple:
    """Route one BSA LLM call per the UI dropdown:

    - 'opencode' -> OpenCode Go subscription (unlimited DeepSeek Flash via
      opencode.ai/zen/go/v1), falling back to the free pool on error.
    - 'deepseek' or unset -> free pool (existing zero-cost behavior).

    Returns (content, provider_used).
    """
    global _last_used_provider
    provider = _selected_provider()

    if provider == "opencode":
        try:
            from llm import invoke_messages_with_fallback
            from langchain_core.messages import SystemMessage, HumanMessage
            content = invoke_messages_with_fallback(
                role="BSA",
                messages=[
                    SystemMessage(content=system_prompt),
                    HumanMessage(content=prompt),
                ],
                temp=0.3,
                where="BSA spec section",
                model="deepseek-v4-flash",
            )
            if content:
                _last_used_provider = "opencode-go"
                return str(content), "opencode-go"
            _log("[BSA] OpenCode Go returned empty response - falling back to free pool")
        except Exception as e:
            _log(f"[BSA] OpenCode Go failed ({str(e)[:150]}) - falling back to free pool")

    try:
        agent = _get_bsa_agent()
    except Exception as e:
        _last_used_provider = ""
        _log(f"[BSA] No free providers for fallback: {str(e)[:150]}")
        return "", ""

    try:
        content = agent.chat(
            task=prompt,
            capability=capability,
            max_tokens=max_tokens,
            system_prompt=system_prompt,
        )
        _last_used_provider = getattr(agent, "_last_provider", None) or ""
        return content or "", _last_used_provider
    except Exception as e:
        _last_used_provider = ""
        _log(f"[BSA] Free model failed (capability={capability}): {str(e)[:200]}")
        return "", ""


# ----------------------------------------------------------------------
#  Research phase (harness-driven: model suggests, harness reads)
# ----------------------------------------------------------------------

_SKIP_DIRS = {
    ".git", ".venv", "venv", "node_modules", "__pycache__",
    ".deep_agents", ".antigravity", ".claude", ".codex",
    ".pytest_cache", ".agents", ".agent-memory", "dist", "build",
}


def _list_project_files(project_path: str, cap: int = 300) -> str:
    if not project_path or not os.path.isdir(project_path):
        return ""
    lines = []
    for root, dirs, files in os.walk(project_path):
        dirs[:] = [d for d in dirs if d not in _SKIP_DIRS]
        for fname in sorted(files):
            full = os.path.join(root, fname)
            if os.path.getsize(full) > 300_000:  # don't list huge binaries
                continue
            lines.append(os.path.relpath(full, project_path))
            if len(lines) >= cap:
                return "\n".join(lines)
    return "\n".join(lines)


def _parse_json_list(text: str) -> list:
    """Best-effort extraction of a JSON array from an LLM response."""
    if not text:
        return []
    m = re.search(r"\[[\s\S]*\]", text)
    if m:
        try:
            data = json.loads(m.group(0))
            if isinstance(data, list):
                return data
        except json.JSONDecodeError:
            pass
    # Fallback: loose scan for {"type": "...", "target": "..."} pairs.
    items = []
    for t, tg in re.findall(r'"type"\s*:\s*"([^"]+)"\s*,\s*"target"\s*:\s*"([^"]+)"', text):
        items.append({"type": t, "target": tg})
    return items[:8]


def _safe_read(path: str, project_path: str, max_chars: int = 4000) -> str:
    try:
        full = path if os.path.isabs(path) else os.path.join(project_path, path)
        full = os.path.normpath(full)
        if not os.path.isfile(full):
            return ""
        if os.path.getsize(full) > 200_000:
            return ""
        with open(full, "r", encoding="utf-8", errors="replace") as f:
            return f.read(max_chars)
    except Exception:
        return ""


def _safe_fetch(url: str, max_chars: int = 2000) -> str:
    try:
        import urllib.request
        req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=10) as r:
            data = r.read(max_chars + 500).decode("utf-8", errors="replace")
        return data[:max_chars]
    except Exception:
        return ""


def _research_digest(task: str, project_path: str, agent: FreeAgent) -> str:
    tree = _list_project_files(project_path, cap=300)
    if not tree:
        return "Greenfield project — no existing code found. Design the system from scratch."

    suggest = (
        "You are preparing to write a technical specification for the task below, against the "
        "existing project file tree. Which existing files MUST be read so the spec matches the "
        "current code? Respond with ONLY a JSON array, each item "
        '{"type": "read_file", "target": "<relative path>"} or '
        '{"type": "web_fetch", "target": "<url>"}. Pick at most 6. If none are needed, respond [].\n\n'
        f"## TASK\n{task}\n\n## PROJECT FILE TREE\n{tree}"
    )
    resp, _prov = _chat_any(
        suggest,
        capability="reasoning",
        max_tokens=1200,
        system_prompt="You are a research planner. Output only JSON.",
    )
    _record_free_call("research", _prov, "reasoning", len(resp or ""))

    items = _parse_json_list(resp)
    _log(f"[BSA] Research requested {len(items)} item(s)")

    chunks = []
    for item in items[:8]:
        typ = (item or {}).get("type", "")
        target = (item or {}).get("target", "")
        if not target:
            continue
        if typ == "read_file":
            content = _safe_read(target, project_path, max_chars=4000)
            if content:
                chunks.append(f"--- FILE: {target} ---\n{content}")
        elif typ == "web_fetch":
            text = _safe_fetch(target, max_chars=2000)
            if text:
                chunks.append(f"--- WEB: {target} ---\n{text}")

    if not chunks:
        return "No additional research needed — task is self-contained."
    return "\n\n".join(chunks)


# ----------------------------------------------------------------------
#  Section walk
# ----------------------------------------------------------------------

def _section_prompt(sec: dict, task: str, digest: str, prior_content: str) -> str:
    parts = [f"## TASK\n{task}", f"\n## SECTION TO WRITE\n{sec['instructions']}"]
    if digest:
        parts.append(f"\n## EXISTING PROJECT CONTEXT (research digest)\n{digest}")
    if prior_content:
        # Stable reference instead of the raw growing tail: pass only the section
        # HEADINGS (fixed-length, never changes between calls) plus a short index.
        # The old `prior_content[-12000:]` tail shifted on every new section, which
        # broke DeepSeek prefix-cache continuity and re-missed ~12K tokens per call.
        headings = re.findall(r"^## (\d+\. .+)$", prior_content, re.MULTILINE)
        if headings:
            index = "\n".join(f"- {h}" for h in headings)
            parts.append(
                f"\n## SPEC SO FAR (section index — reference, do not repeat)\n{index}"
            )
        else:
            parts.append(f"\n## SPEC SO FAR (already-written content)\n{prior_content[:4000]}")
    parts.append("\nOutput ONLY this section's content now. No preamble, no heading, no commentary.")
    return "\n\n".join(parts)


def _chat_safe(agent: FreeAgent, prompt: str, capability: str, max_tokens: int) -> str:
    content, provider = _chat_any(
        prompt,
        capability=capability,
        max_tokens=max_tokens,
        system_prompt=BSA_SYSTEM_PROMPT,
    )
    _record_free_call("llm", provider, capability, len(content or ""))
    return content


def _is_na(content: str) -> bool:
    return bool(content) and content.strip().upper().startswith(_NA_PREFIX)


def _needs_escalation(content: str) -> bool:
    if not content or not content.strip():
        return True
    s = content.strip()
    if len(s) < 100 and not _is_na(s):
        return True
    return False


def _trim_oversized(content: str, cap: int = 12000) -> str:
    """Guard against a model dumping the whole spec in one section: truncate."""
    if len(content) > cap:
        return content[:cap] + "\n\n<!-- [BSA] output truncated — wrote more than this section -->"
    return content


def _generate_section(agent: FreeAgent, sec: dict, task: str, digest: str, prior_content: str) -> str:
    prompt = _section_prompt(sec, task, digest, prior_content)
    cap = sec["capability"]
    content = _chat_safe(agent, prompt, capability=cap, max_tokens=_MAX_TOKENS.get(cap, 6000))
    if _needs_escalation(content):
        _log(f"[BSA] Section {sec['num']} too short ({len(content or '')} chars) — escalating to reasoning model")
        content = _chat_safe(agent, prompt, capability="reasoning", max_tokens=_MAX_TOKENS["reasoning"])
    if _needs_escalation(content):
        return _PLACEHOLDER_MARK
    return _trim_oversized(content.strip())


# ----------------------------------------------------------------------
#  Spec file checkpointing
# ----------------------------------------------------------------------

def _read_spec_state(spec_path: str) -> tuple:
    """Return (set of section titles already in the file, full file content)."""
    if not os.path.isfile(spec_path):
        return set(), ""
    try:
        with open(spec_path, "r", encoding="utf-8", errors="replace") as f:
            text = f.read()
    except Exception:
        return set(), ""
    # Only section headings that match a known SECTION_DEFS title count as
    # checkpoints — model content can contain "## 2. ..." sub-headings which
    # would otherwise be mistaken for completed sections.
    known_titles = {s["title"] for s in SECTION_DEFS}
    done = set(re.findall(r"^## \d+\. (.+)$", text, re.MULTILINE)) & known_titles
    return done, text


def _append_section(spec_path: str, sec: dict, content: str, is_first: bool) -> None:
    content = content.strip()
    block = f"\n## {sec['num']}. {sec['title']}\n{content}\n"
    if is_first or not os.path.isfile(spec_path):
        header = (
            f"# Specification: {sec['title']}\n"
            "**Version**: 1.0\n"
            f"**Date**: {time.strftime('%Y-%m-%d')}\n"
            "**Author**: BSA (free spec agent)\n"
            "**Status**: Draft\n"
            "**Handoff**: Ready for Dev implementation\n\n---\n"
        )
        block = header + block
    try:
        with open(spec_path, "a", encoding="utf-8") as f:
            f.write(block)
    except Exception as e:
        _log(f"[BSA] Failed to write spec section: {e}")


def _make_slug(task: str) -> str:
    stop = {
        "a", "an", "the", "and", "or", "for", "of", "to", "with", "in", "on", "at",
        "build", "create", "make", "add", "implement", "feature", "new", "page", "module",
    }
    words = [w for w in re.findall(r"[A-Za-z0-9]+", task.lower()) if w not in stop]
    slug = "-".join(words[:3]) if words else "spec"
    return slug[:60]


def _build_summary(spec_path: str, project_path: str, written: list, n_na: int,
                   placeholders: int, providers: set) -> str:
    try:
        rel = os.path.relpath(spec_path, project_path).replace("\\", "/")
    except Exception:
        rel = os.path.basename(spec_path)
    status = "PARTIAL" if placeholders else "COMPLETE"
    prov = ", ".join(sorted(p for p in providers if p)) if providers else "none"
    return (
        f"[BSA] Spec {status}: {rel} — {len(written)}/{_MAX_SECTIONS} sections "
        f"({n_na} N/A, {placeholders} failed). Providers used: {prov}. "
        f"Full path: {spec_path}"
    )


# ----------------------------------------------------------------------
#  Entry point
# ----------------------------------------------------------------------

def run_bsa_spec(task: str, project_path: str, chat_id: str = "", max_sections: int = None) -> str:
    """BSA spec generation. Routes per the UI dropdown:
    'opencode' -> OpenCode Go subscription; 'deepseek'/unset -> free pool.
    Returns a summary string for the orchestrator."""
    if not task or not task.strip():
        return "[BSA] Error: empty task."

    mode = _selected_provider()
    if mode == "opencode":
        try:
            agent = _get_bsa_agent()
        except Exception:
            agent = None  # Go subscription path only; free fallback unavailable
    else:
        try:
            agent = _get_bsa_agent()
        except Exception as e:
            return (
                f"[BSA] Error: no free providers configured ({e}). "
                "Set at least one of GROQ_API_KEY / GEMINI_API_KEY / MISTRAL_API_KEY / "
                "OPENROUTER_API_KEY / NVIDIA_API_KEY / ZAI_API_KEY / OPENCODE_API_KEY / "
                "CLINE_API_KEY in .env, or switch the LLM provider dropdown to OpenCode Zen."
            )

    slug = _make_slug(task)
    specs_dir = os.path.join(project_path, "specs")
    try:
        os.makedirs(specs_dir, exist_ok=True)
    except Exception as e:
        return f"[BSA] Error: cannot create specs dir under {project_path}: {e}"
    spec_path = os.path.join(specs_dir, f"SPEC-{slug}.md")

    mode_label = "opencode-go" if mode == "opencode" else "free-pool"
    _log(f"[BSA] Spec agent — task={task[:80]!r} slug={slug} mode={mode_label} providers={BSA_PROVIDERS}")

    digest = _research_digest(task, project_path, agent)
    _log(f"[BSA] Research digest: {len(digest)} chars")

    done, prior = _read_spec_state(spec_path)
    written = list(done)
    placeholders = 0
    n_na = 0
    providers = set()
    # BSA_MAX_SECTIONS env override lets tests bound the walk (default: full 8).
    try:
        limit = max_sections or int(os.environ.get("BSA_MAX_SECTIONS") or _MAX_SECTIONS)
    except ValueError:
        limit = _MAX_SECTIONS
    limit = max(1, min(limit, _MAX_SECTIONS))

    for sec in SECTION_DEFS[:limit]:
        if sec["title"] in done:
            continue  # resume from checkpoint
        _log(f"[BSA] Section {sec['num']}/{_MAX_SECTIONS}: {sec['title']} (capability={sec['capability']})")
        is_first = not os.path.isfile(spec_path)

        content = _generate_section(agent, sec, task, digest, prior)
        if _last_used_provider:
            providers.add(_last_used_provider)

        if content.startswith(_PLACEHOLDER_MARK):
            placeholders += 1
            content = _PLACEHOLDER_MARK + " unable to generate (all providers exhausted)"
        elif _is_na(content):
            n_na += 1

        _append_section(spec_path, sec, content, is_first)
        written.append(sec["title"])
        try:
            _, prior = _read_spec_state(spec_path)
        except Exception:
            pass
        time.sleep(0.5)  # small RPM-friendly gap between provider calls

    # Persist the spec path so the orchestrator / Dev can find it.
    try:
        from state_sync import safe_get_state, safe_update_state
        state = safe_get_state()
        safe_update_state({
            "outputs": {
                **state.get("outputs", {}),
                "spec_file": spec_path,
            }
        })
    except Exception:
        pass

    return _build_summary(spec_path, project_path, written, n_na, placeholders, providers)
