"""
LLM Invocation â€” DeepSeek-only with model-level fallback.
Uses the OpenAI-compatible DeepSeek API at https://api.deepseek.com.
"""
import json
import os
import time
import threading
from typing import Any, Optional, Type, TypeVar

from dotenv import load_dotenv
from pydantic import BaseModel
from langchain_openai import ChatOpenAI
from langchain_core.language_models.chat_models import BaseChatModel

from llm.llm_config import TASK_CATEGORIES, get_task_category, DEEPSEEK_MODEL_VISION, supports_vision

from llm.llm_stats import TokenUsageTracker, update_token_stats
# rate_limiter removed â€” no artificial API throttling. The model decides pacing.

load_dotenv(override=True)
T = TypeVar("T", bound=BaseModel)

# Override to route DeepSeek calls through a compatible gateway (e.g. OpenCode Go:
# DEEPSEEK_BASE_URL=https://opencode.ai/zen/go/v1 with OPENCODE_API_KEY set) temporarily.
DEEPSEEK_BASE_URL = os.getenv("DEEPSEEK_BASE_URL", "https://api.deepseek.com")


def _messages_have_images(messages: list) -> bool:
    """True if any message carries an image content part.

    Accepts every common image-part spelling (OpenAI image_url, Anthropic image,
    Responses input_image). Only user-role parts are valid to the API, but the
    presence check is role-agnostic on purpose: an image anywhere means the
    call must go to a vision model or it will hard-400."""
    for msg in messages or []:
        content = getattr(msg, "content", None)
        if isinstance(content, list):
            for part in content:
                if isinstance(part, dict) and part.get("type") in ("image_url", "image", "input_image"):
                    return True
    return False


def _is_off_peak() -> bool:
    """Check if the current time is in the DeepSeek off-peak discount window (16:30-00:30 UTC next day)."""
    import datetime
    now_utc = datetime.datetime.now(datetime.timezone.utc).time()
    start = datetime.time(16, 30)
    end = datetime.time(0, 30)
    if start <= now_utc or now_utc <= end:
        return True
    return False


def log_terminal(msg: str) -> None:
    """Log to shared_state live_terminal_log. Console logging is best-effort."""
    try:
        print(msg, flush=True)
    except (UnicodeEncodeError, OSError):
        try:
            safe = msg.encode("ascii", errors="replace").decode()
            print(safe, flush=True)
        except OSError:
            pass  # console is gone â€” log to shared_state only
    try:
        from sync.state_sync import safe_append_live_log
        safe_append_live_log(msg)
    except Exception:
        pass


def _extract_text(content: Any) -> str:
    """Safely extract text from LLM response content."""
    if isinstance(content, list):
        return "".join(
            p.get("text", str(p)) if isinstance(p, dict) else str(p)
            for p in content
        )
    return str(content)


_PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
_LLM_PROVIDER_FILE = os.path.join(_PROJECT_ROOT, ".deep_agents", "llm_provider.json")


def _load_persisted_provider() -> str:
    """Read the persisted provider choice (set by the UI switcher), so fresh subprocesses
    (e.g. each eval task) and server restarts keep the same selection."""
    try:
        if os.path.isfile(_LLM_PROVIDER_FILE):
            with open(_LLM_PROVIDER_FILE, encoding="utf-8") as f:
                return (json.load(f).get("provider") or "").strip().lower()
    except Exception:
        pass
    return ""


def _save_persisted_provider(provider: str) -> None:
    try:
        os.makedirs(os.path.dirname(_LLM_PROVIDER_FILE), exist_ok=True)
        with open(_LLM_PROVIDER_FILE, "w", encoding="utf-8") as f:
            json.dump({"provider": provider}, f)
    except Exception:
        pass


def _resolve_llm_provider() -> dict:
    """Return {base, key, flash, pro} for the active LLM provider.

    Resolution order: shared_state['llm_provider'] (running server / UI) â†’ persisted file
    (survives subprocesses and restarts) â†’ env default. "opencode" routes paid DeepSeek calls
    through OpenCode Go (https://opencode.ai/zen/go/v1, OPENCODE_API_KEY); anything else uses the
    direct DeepSeek API (or DEEPSEEK_BASE_URL if set). Pro maps to flash on OpenCode.
    """
    provider = ""
    try:
        from sync.state_sync import safe_get_state
        provider = (safe_get_state().get("llm_provider") or "").strip().lower()
    except Exception:
        pass
    if not provider:
        provider = _load_persisted_provider()

    if provider == "command":
        # Command Code Provider API â€” OpenAI/Anthropic-compatible gateway.
        # Official endpoint by default; COMMAND_CODE_BASE_URL can point at any
        # compatible gateway/proxy. Model slots are org-scoped catalog slugs;
        # every one is env-overridable without code changes.
        return {
            "base": os.getenv("COMMAND_CODE_BASE_URL", "https://api.commandcode.ai/provider/v1"),
            "key": os.getenv("COMMAND_CODE_API_KEY", ""),
            "flash": os.getenv("COMMAND_CODE_MODEL_FLASH", "deepseek/deepseek-v4-flash"),
            "pro": os.getenv("COMMAND_CODE_MODEL_PRO", "deepseek/deepseek-v4-pro"),
            "vision": os.getenv("COMMAND_CODE_MODEL_VISION", "deepseek/deepseek-v4-flash-vision-exp"),
            "orch": os.getenv("COMMAND_CODE_MODEL_ORCH", "xiaomi/mimo-v2.5"),
            "light": os.getenv("COMMAND_CODE_MODEL_LIGHT", "laguna-s-2.1-free"),
            "audio": os.getenv("COMMAND_CODE_MODEL_AUDIO", "xiaomi/mimo-v2.5"),
        }
    if provider == "opencode":
        # Go subscription endpoint â€” /zen/v1 is the generic/pay-per-use route that has no
        # balance for paid models (returns "Insufficient balance" even with an active Go sub).
        return {
            "base": os.getenv("DEEPSEEK_BASE_URL", "https://opencode.ai/zen/go/v1"),
            "key": os.getenv("OPENCODE_API_KEY", "") or os.getenv("DEEPSEEK_API_KEY", ""),
            "flash": os.getenv("DEEPSEEK_MODEL_FLASH", "deepseek-v4-flash"),
            "pro": os.getenv("DEEPSEEK_MODEL_PRO", "deepseek-v4-flash"),
            "vision": os.getenv("DEEPSEEK_MODEL_VISION", DEEPSEEK_MODEL_VISION),
        }
    # default: direct DeepSeek API (or DEEPSEEK_BASE_URL override from .env)
    return {
        "base": os.getenv("DEEPSEEK_BASE_URL", "https://api.deepseek.com"),
        "key": os.getenv("DEEPSEEK_API_KEY", "") or os.getenv("OPENCODE_API_KEY", ""),
        "flash": os.getenv("DEEPSEEK_MODEL_FLASH", "deepseek-v4-flash"),
        "pro": os.getenv("DEEPSEEK_MODEL_PRO", "deepseek-v4-pro"),
        "vision": os.getenv("DEEPSEEK_MODEL_VISION", DEEPSEEK_MODEL_VISION),
    }


def _map_model(model: str) -> str:
    """Map a canonical model name (deepseek-v4-flash/pro/vision-exp) to the active provider's name.

    Accepts both the colon slug used by agent constants (``deepseek:v4-flash``)
    and the dash canonical (``deepseek-v4-flash``). Org-scoped slugs
    (``deepseek/deepseek-v4-flash``, ``xiaomi/mimo-v2.5``) pass through.
    """
    cfg = _resolve_llm_provider()
    if "/" in model:
        # Org-scoped slug (e.g. "deepseek/deepseek-v4-flash", "laguna-s-2.1-free")
        # is already provider-specific â€” pass through untouched.
        return model
    # Normalize the colon form agent constants use to the canonical dash form.
    if model == "deepseek:v4-flash":
        model = "deepseek-v4-flash"
    elif model == "deepseek:v4-pro":
        model = "deepseek-v4-pro"
    elif model == "deepseek:v4-flash-vision-exp":
        model = "deepseek-v4-flash-vision-exp"
    if model == "deepseek-v4-flash":
        return cfg["flash"]
    if model == "deepseek-v4-pro":
        return cfg["pro"]
    if model in (DEEPSEEK_MODEL_VISION, "deepseek-v4-flash-vision-exp"):
        # Per-provider vision slot: same slug on the DeepSeek API and OpenCode
        # Go listings; the command gateway names its own.
        return cfg.get("vision") or DEEPSEEK_MODEL_VISION
    if model == "deepseek-v4-audio":
        # Per-provider audio slot (MiMo): only the command gateway defines one.
        return cfg.get("audio") or "xiaomi/mimo-v2.5"
    return model


# ── Prefix-cache thinking pins (Phase 7): first-used thinking config per
# (role, model) is frozen for the process so DeepSeek's implicit byte-prefix
# cache is never busted by mid-conversation enabled↔disabled flips.
_THINKING_PINS: dict = {}
_THINKING_PINS_LOCK = threading.Lock()


def _get_deepseek_client(model: str, temp: float, role: Optional[str] = None, response_format: Optional[dict] = None, tools: Optional[list[dict]] = None, reasoning_effort: Optional[str] = None, reasoning_budget: Optional[int] = None, messages: Optional[list] = None) -> BaseChatModel:
    """Create a DeepSeek ChatOpenAI client with optional native tools."""
    _cfg = _resolve_llm_provider()
    base_url = _cfg["base"]
    key = _cfg["key"]
    if not key or key.startswith("your_"):
        raise ValueError(
            f"LLM API key not set for base {base_url}. "
            "Set DEEPSEEK_API_KEY (or OPENCODE_API_KEY for the OpenCode Go "
            "gateway / COMMAND_CODE_API_KEY for Command Code)."
        )

    # No per-call max_tokens â€” the model decides how much to output.
    # The context compaction system handles the window, the LLM handles pacing.

    kwargs: dict[str, Any] = {
        "model": model,
        "api_key": key,
        "base_url": base_url,
        "timeout": 120.0,
        "max_retries": 1,
        "temperature": temp,
    }
    if response_format is not None:
        kwargs.setdefault("model_kwargs", {})["response_format"] = response_format
    if tools:
        # Pass tools via model_kwargs for DeepSeek native function calling
        kwargs.setdefault("model_kwargs", {})["tools"] = tools

    # Portable thinking config — mirrors pai_transport._build_model_settings.
    # LangChain path kept for invoke_with_fallback legacy queue; uses same
    # thinking semantics as the native path but without the prefix-cache pin.
    is_reasoning_model = any(x in model.lower() for x in ["r1", "pro", "o1", "o3", "reasoning", "thinking", "fallback", "automatic", "flash"])
    thinking_effort: str | None = None
    thinking_disabled = False
    if reasoning_effort is not None:
        effort = str(reasoning_effort).strip().lower()
        if effort in ("disabled", "none", ""):
            kwargs["extra_body"] = {"thinking": {"type": "disabled"}}
            thinking_disabled = True
        else:
            budget_override = reasoning_budget
            if effort == "max":
                effort = "high"
                if budget_override is None:
                    try:
                        budget_override = int(os.getenv("REASONING_MAX_BUDGET", "32768"))
                    except Exception:
                        budget_override = 32768
            kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
            kwargs["reasoning_effort"] = effort
            thinking_effort = effort
            if budget_override is not None:
                try:
                    kwargs["extra_body"]["thinking"]["budget_tokens"] = int(budget_override)
                except Exception:
                    pass
    elif is_reasoning_model:
        is_orchestrator = (
            role in ["Developer", "DeveloperFixing"]
            or (role and any(x in role.lower() for x in ["developer", "orchestrator", "designer"]))
        )
        is_heavy = is_orchestrator or (
            role in ["Supervisor", "sa", "Coder"]
            or (role and any(x in role.lower() for x in ["fixing", "coder", "architect", "complex"]))
        )
        if is_orchestrator or is_heavy:
            kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
            kwargs["reasoning_effort"] = "high"
            thinking_effort = "high"
        else:
            kwargs["extra_body"] = {"thinking": {"type": "disabled"}}
            thinking_disabled = True

    # NOTE: _THINKING_PINS prefix-cache freeze removed (P0-1). Portable
    # thinking=False vs "high" via isinstance(thinking) eliminates the
    # enabled↔disabled flip that busted DeepSeek byte-prefix cache.

    return ChatOpenAI(**kwargs)



def _clear_failed_models() -> None:
    """Clear the failed models set so each new request starts fresh."""
    try:
        from sync.state_sync import safe_update_state
        safe_update_state({"failed_models": []})
    except Exception:
        pass


def _parse_schema_response(text: str, schema: Type[T]) -> T:
    """Helper to parse a raw JSON string from the LLM into a Pydantic model."""
    import json
    import re

    # Try to find JSON in markdown code block
    json_match = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
    if json_match:
        text = json_match.group(1)
    else:
        # Try to find the first '{' and last '}'
        start = text.find("{")
        end = text.rfind("}")
        if start != -1 and end != -1:
            text = text[start : end + 1]

    text = text.strip()
    
    # Escape invalid single backslashes (like PHP namespaces \Sanctum\) to prevent JSON decode errors
    # but avoid corrupting already double-escaped backslashes.
    pattern = re.compile(r'\\\\|\\"|\\/|\\b|\\f|\\n|\\r|\\t|\\u[0-9a-fA-F]{4}|\\')
    def replace_invalid_escape(match):
        val = match.group(0)
        if val == '\\':
            return '\\\\'
        return val
    text = pattern.sub(replace_invalid_escape, text)
    
    # Parse JSON
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        log_terminal(f"JSON parsing failed. Raw response was: {text}\n")
        raise ValueError(f"Failed to parse JSON response: {e}")

    # Validate schema
    if hasattr(schema, "model_validate"):
        return schema.model_validate(data)
    elif hasattr(schema, "parse_obj"):
        return schema.parse_obj(data)
    else:
        return schema(**data)


import threading
import contextvars

#: Per-thread StreamBus target override. Parallel delegation threads set this
#: to their own chat id so child deltas never interleave into the parent's
#: stream (the bus key otherwise comes from the GLOBAL shared_state chat_id).
stream_chat_override: contextvars.ContextVar[str] = contextvars.ContextVar(
    "stream_chat_override", default=""
)


def _effective_stream_chat_id() -> str:
    override = stream_chat_override.get()
    if override:
        return override
    try:
        from sync.state_sync import shared_state
        return str(shared_state.get("chat_id") or "")
    except Exception:
        return ""


class ConcurrencyThrottle:
    """Async-native concurrency gate (Phase 7): asyncio.Semaphore instead of
    the spin-lock. Sync callers (deepagents graph, legacy invoke paths) use
    `.acquire_sync()` which parks the thread on a threading.Event rather than
    burning CPU with time.sleep(0.1)."""
    def __init__(self, max_concurrency: int = 4):
        import asyncio
        self.max_concurrency = max_concurrency
        self._sem = asyncio.Semaphore(max_concurrency)
        self._sync_sem = threading.Semaphore(max_concurrency)
        self._cooldown_until = 0.0
        self._cooldown_lock = threading.Lock()

    def _in_cooldown(self) -> bool:
        with self._cooldown_lock:
            return time.time() < self._cooldown_until

    async def acquire(self):
        # Cooldown after a rate-limit: only one request may run for its duration.
        import asyncio
        while self._in_cooldown():
            await asyncio.sleep(0.1)
        await self._sem.acquire()

    def release(self):
        import asyncio
        self._sem.release()

    def acquire_sync(self):
        if self._in_cooldown():
            # Park the thread briefly instead of a hot spin.
            import time as _t
            while self._in_cooldown():
                _t.sleep(0.1)
        self._sync_sem.acquire()

    def release_sync(self):
        self._sync_sem.release()

    def trigger_cooldown(self, duration=30):
        with self._cooldown_lock:
            self._cooldown_until = time.time() + duration

_throttle = ConcurrencyThrottle()


def trim_prompt(text: str) -> str:
    """Pre-processes prompts to strip markdown comments, trailing spaces, and redundant newlines in a single pass."""
    import re
    if not isinstance(text, str) or not text:
        return text
    # 1. Strip markdown HTML comments: <!-- comment -->
    text = re.sub(r"<!--[\s\S]*?-->", "", text)
    # 2. Strip trailing spaces from each line
    lines = [line.rstrip() for line in text.splitlines()]
    # 3. Collapse multiple consecutive empty lines to a single empty line
    collapsed_lines = []
    for line in lines:
        if not line:
            if collapsed_lines and collapsed_lines[-1]:  # only keep one empty line
                collapsed_lines.append("")
        else:
            collapsed_lines.append(line)
    return "\n".join(collapsed_lines).strip()


def _parse_retry_after(err_str: str) -> int:
    """Parse Retry-After from headers/body: integer seconds, retry-after-ms float, or HTTP-date."""
    import re
    import email.utils
    from datetime import datetime, timezone
    low = err_str.lower()
    # retry-after-ms: <float milliseconds>
    m = re.search(r"retry[-_]after[-_]ms[:\s]+(\d+(?:\.\d+)?)", low)
    if m:
        try:
            return max(1, int(float(m.group(1)) / 1000))
        except Exception:
            pass
    # retry-after: <integer seconds>
    m = re.search(r"retry[-_]after[:\s]+(\d+)", low)
    if m:
        try:
            return max(1, int(m.group(1)))
        except Exception:
            pass
    # retry-after: <HTTP-date> e.g. "Wed, 21 Oct 2015 07:28:00 GMT"
    m = re.search(
        r"retry[-_]after[:\s]+([A-Za-z]{3}, \d{2} [A-Za-z]{3} \d{4} \d{2}:\d{2}:\d{2} [A-Za-z]+)",
        low,
    )
    if m:
        try:
            parsed = email.utils.parsedate_to_datetime(m.group(1))
            return max(1, int((parsed - datetime.now(timezone.utc)).total_seconds()))
        except Exception:
            pass
    # "after N seconds" phrasing
    m = re.search(r"after\s+(\d+)\s+seconds", low)
    if m:
        try:
            return max(1, int(m.group(1)))
        except Exception:
            pass
    return 0


def _is_context_overflow(err_str: str) -> bool:
    """Context-window errors can never be fixed by retrying â€” report and move on."""
    low = err_str.lower()
    markers = (
        "context length", "context window", "maximum context", "max context",
        "context is too long", "context too long", "token limit", "input is too long",
        "too many tokens", "exceeded the context", "context overflow",
        "context_length_exceeded", "prompt is too long", "reduce the length",
    )
    return any(m in low for m in markers)


_RETRYABLE_PATTERNS = (
    r"429|500|502|503|504|524",
    r"rate limit|rate-limit|rate_limit|too many requests",
    r"overloaded|service unavailable|service_unavailable|internal error|internal_error|server error",
    r"terminated|fetch failed|failed to fetch|network error|connection error|connection refused|connection reset|socket hang up|reset before headers|getaddrinfo|econnreset|econnrefused|etimedout|eai_again|enotfound",
    r"request (?:timeout|timed out)|response (?:timeout|timed out)|connection (?:timeout|timed out)|read (?:timeout|timed out)|stream (?:timeout|timed out)",
    r"resource exhausted|resource_exhausted|try your request again|retry your request",
)

_NON_RETRYABLE_PATTERNS = (
    "bad request", "invalid request", "invalid api key", "invalid parameter",
    "unauthorized", "authentication", "insufficient_quota", "permission denied",
    "not found", "does not exist", "is not a valid", "validation error",
)


def _is_retryable_http_error(err_str: str) -> bool:
    """True for transient server/rate/network errors; False for permanent client errors."""
    if _is_context_overflow(err_str):
        return False
    low = err_str.lower()
    # Permanent client errors â€” never retry
    if any(p in low for p in _NON_RETRYABLE_PATTERNS):
        return False
    import re
    return any(re.search(p, low) for p in _RETRYABLE_PATTERNS)


def _handle_rate_limit(err_str: str, attempt: int) -> int:
    """Backoff wait time, honoring server Retry-After when present (seconds, ms, or HTTP-date)."""
    retry_after = _parse_retry_after(err_str)
    # Exponential backoff: 2 * (2 ** attempt)
    exp_backoff = 2 * (2 ** attempt)
    # Server-provided wait wins when present; backoff is the fallback
    wait = retry_after if retry_after > 0 else exp_backoff
    return min(max(wait, 1), 60)  # 1s floor, 60s cap


def _log_cache_debug(
    messages,
    tracker,
    role: str,
    model: str,
    tools: Optional[list[dict]] = None,
    reasoning_effort: Optional[str] = None,
    reasoning_budget: Optional[int] = None,
) -> None:
    """CF3: CACHE_DEBUG=1 â†’ log a payload-content hash + cache tokens per call.

    DeepSeek prefix caching is a byte-prefix match. If cache_hit stays 0 across
    repeated calls with identical prefixes, a silent invalidator is at work â€”
    compare payload_sha between two requests to find the changing bytes.
    """
    if os.environ.get("CACHE_DEBUG", "0") != "1":
        return
    try:
        import hashlib
        message_payload = []
        for message in messages:
            item = {
                "type": type(message).__name__,
                "content": getattr(message, "content", ""),
            }
            for field in ("name", "tool_call_id", "tool_calls", "additional_kwargs"):
                if hasattr(message, field):
                    item[field] = getattr(message, field)
            message_payload.append(item)
        payload = json.dumps(
            {
                "messages": message_payload,
                # Tool schemas and thinking configuration are request inputs
                # even though they are not LangChain message objects.
                "tools": tools or [],
                "reasoning_effort": reasoning_effort,
                "reasoning_budget": reasoning_budget,
            },
            sort_keys=True,
            ensure_ascii=False,
            separators=(",", ":"),
            default=str,
        )
        digest = hashlib.sha256(payload.encode("utf-8", "replace")).hexdigest()[:16]
        log_terminal(
            f"[CACHE-DEBUG] role={role} model={model} payload_sha={digest} len={len(payload)} "
            f"tools={len(tools or [])} reasoning={reasoning_effort or 'default'} "
            f"in={getattr(tracker, 'input_tokens', 0)} out={getattr(tracker, 'output_tokens', 0)} "
            f"cache_hit={getattr(tracker, 'cache_hit_tokens', 0)}\n"
        )
    except Exception:
        pass


def invoke_with_fallback(
    role: str,
    sys_inst: str,
    prompt: str,
    schema: Optional[Type[T]] = None,
    temp: float = 0.7,
    where: Optional[str] = None,
) -> Any:
    """Invoke LLM with DeepSeek fallback chain. Supports structured output."""
    

    category = get_task_category(role)
    queue = TASK_CATEGORIES[category].copy()

    # Map canonical model names to the active provider's models (deepseek | opencode)
    queue = [{"provider": item["provider"], "model": _map_model(item["model"])} for item in queue]

    selected_temp = temp

    try:
        from sync.state_sync import safe_get_state
        state = safe_get_state()
        selected_temp = state.get("selected_temp", temp)
    except Exception:
        pass

    # Use direct messages instead of ChatPromptTemplate to avoid
    # curly braces in JSON content being parsed as template variables.
    from langchain_core.messages import SystemMessage, HumanMessage

    errors_by_model: dict[str, str] = {}

    for item in queue:
        model = item["model"]
        try:
            if role != "MemoryArchivist":
                # Show what the agent is working on, not just the model name
                action = where[:120].replace('\n', ' ').strip() if where else "processing..."
                log_terminal(f"[{role}] {action}")
            response_format = {"type": "json_object"} if schema else None

            # If schema requested, add JSON guidelines to prompt and enable native response_format
            effective_sys = trim_prompt(sys_inst)
            effective_prompt = trim_prompt(prompt)
            if schema:
                fields = schema.model_fields
                field_desc = ", ".join(
                    f'"{k}": <{f.annotation.__name__ if f.annotation else "str"}>'
                    for k, f in fields.items()
                )
                effective_sys += (
                    f"\n\nYou MUST respond with ONLY a valid JSON object "
                    f"matching this schema: {{{field_desc}}}. "
                    f"No markdown, no explanation, just the raw JSON."
                )

            messages = [
                SystemMessage(content=effective_sys),
                HumanMessage(content=effective_prompt),
            ]

            llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, messages=messages)
            tracker = TokenUsageTracker()

            _throttle.acquire_sync()
            try:
                res = llm.invoke(
                    messages,
                    config={"callbacks": [tracker]},
                )
            finally:
                _throttle.release_sync()
            reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
            if reasoning:
                log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
            raw_text = _extract_text(getattr(res, "content", None))
            if not raw_text and reasoning:
                raw_text = reasoning

            if schema:
                val = _parse_schema_response(raw_text, schema)
            else:
                val = raw_text

            update_token_stats(
                role, model, tracker.input_tokens, tracker.output_tokens, tracker.cache_hit_tokens, where,
                reasoning_chars=len(reasoning) if reasoning else 0,
            )
            _log_cache_debug(messages, tracker, role, model)

            # Log a smart summary instead of dumping the entire response
            try:
                from sync.state_sync import safe_append_live_log
                if not schema:
                    val_str = str(val)
                    # Extract tool calls for a concise summary
                    import re as _re
                    tools_called = _re.findall(r'"tool":\s*"(\w+)"', val_str)
                    if tools_called:
                        safe_append_live_log(f"  â†’ Tools: {', '.join(tools_called)}")
                    elif len(val_str) < 200:
                        safe_append_live_log(val_str)
                    else:
                        safe_append_live_log(f"  â†’ Response: {len(val_str)} chars")
            except Exception:
                pass

            return val

        except Exception as e:
            err_str = str(e)
            errors_by_model[model] = err_str[:300]
            log_terminal(f"[{model}] Failed: {err_str}\n")

            # If rate-limited / transient server error, wait and retry
            if _is_retryable_http_error(err_str):
                _throttle.trigger_cooldown(30)
                for attempt in range(3):
                    wait = _handle_rate_limit(err_str, attempt)
                    log_terminal(
                        f"[{model}] Transient error, retrying (attempt {attempt+1}/3) â€” waiting {wait}s...\n"
                    )
                    time.sleep(wait)
                    try:
                        effective_sys = trim_prompt(sys_inst)
                        if schema:
                            fields = schema.model_fields
                            field_desc = ", ".join(
                                f'"{k}": <{f.annotation.__name__ if f.annotation else "str"}>'
                                for k, f in fields.items()
                            )
                            effective_sys += (
                                f"\n\nYou MUST respond with ONLY a valid JSON object "
                                f"matching this schema: {{{field_desc}}}. "
                                f"No markdown, no explanation, just the raw JSON."
                            )
                        retry_msgs = [
                            SystemMessage(content=effective_sys),
                            HumanMessage(content=trim_prompt(prompt)),
                        ]
                        llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, messages=retry_msgs)
                        tracker = TokenUsageTracker()
                        _throttle.acquire_sync()
                        try:
                            res = llm.invoke(
                                retry_msgs,
                                config={"callbacks": [tracker]},
                            )
                        finally:
                            _throttle.release_sync()
                        reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
                        if reasoning:
                            log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
                        raw_text = _extract_text(getattr(res, "content", res))
                        if schema:
                            val = _parse_schema_response(raw_text, schema)
                        else:
                            val = raw_text
                        update_token_stats(
                            role, model,
                            tracker.input_tokens, tracker.output_tokens,
                            tracker.cache_hit_tokens,
                            where,
                            reasoning_chars=len(reasoning) if reasoning else 0,
                        )
                        _log_cache_debug(retry_msgs, tracker, role, model)
                        return val
                    except Exception as retry_e:
                        err_str = str(retry_e)
                        errors_by_model[model] = f"Rate-limit retry {attempt+1} failed: {err_str[:200]}"
                        log_terminal(f"[{model}] Retry {attempt+1} failed: {retry_e}\n")
                        if not _is_retryable_http_error(err_str):
                            break

    # Build detailed error
    detail = "; ".join(f"{m}: {e}" for m, e in errors_by_model.items())
    raise RuntimeError(
        f"All DeepSeek models exhausted for role '{role}'. "
        f"Errors: {detail}. "
        "Check your DEEPSEEK_API_KEY and account balance at https://platform.deepseek.com."
    )





def _compact_prompt(text: str) -> str:
    if not text:
        return text
    import re
    lines = text.splitlines()
    compacted_lines = []
    in_code_block = False
    for line in lines:
        trimmed = line.strip()
        if trimmed.startswith("```"):
            in_code_block = not in_code_block
            compacted_lines.append(line)
            continue
        
        if in_code_block:
            compacted_lines.append(line)
        else:
            collapsed = re.sub(r'[ \t]+', ' ', line).strip()
            if collapsed:
                compacted_lines.append(collapsed)
            else:
                if compacted_lines and compacted_lines[-1] != "":
                    compacted_lines.append("")
                    
    while compacted_lines and compacted_lines[-1] == "":
        compacted_lines.pop()
        
    return "\n".join(compacted_lines)


def _message_has_tool_calls(message: Any) -> bool:
    """Return whether an AI message participates in a native tool-call round trip.

    LangChain normally exposes calls through ``AIMessage.tool_calls``. Some
    OpenAI-compatible responses keep the raw calls in ``additional_kwargs``
    instead, so check both representations before deciding whether DeepSeek's
    ``reasoning_content`` may be removed.
    """
    try:
        if getattr(message, "tool_calls", None):
            return True
    except Exception:
        pass
    try:
        additional_kwargs = getattr(message, "additional_kwargs", None)
        return bool(
            isinstance(additional_kwargs, dict)
            and additional_kwargs.get("tool_calls")
        )
    except Exception:
        return False


def strip_thinking_from_messages(messages: list, strip: bool = True) -> list:
    """Remove only discardable previous DeepSeek reasoning.

    DeepSeek allows ``reasoning_content`` to be omitted from an assistant turn
    that did not call a tool. It requires the reasoning to be replayed for an
    assistant turn that did call a tool, including all later requests in that
    tool-call conversation. Preserve the latter while stripping the former to
    reduce prompt size without breaking the native tool protocol.

    When ``strip`` is False the message list is returned unchanged.
    """
    if not strip:
        return messages
    out = []
    for msg in messages:
        if (
            type(msg).__name__ == "AIMessage"
            and hasattr(msg, "additional_kwargs")
            and isinstance(msg.additional_kwargs, dict)
            and "reasoning_content" in msg.additional_kwargs
            and not _message_has_tool_calls(msg)
        ):
            try:
                new_msg = msg.model_copy(deep=False)
                new_msg.additional_kwargs = {k: v for k, v in msg.additional_kwargs.items() if k != "reasoning_content"}
                out.append(new_msg)
                continue
            except Exception:
                pass
            try:
                kwargs = {}
                for field in ["content", "id", "name", "tool_calls", "additional_kwargs", "tool_call_id"]:
                    if hasattr(msg, field):
                        kwargs[field] = getattr(msg, field)
                kwargs["additional_kwargs"] = {k: v for k, v in msg.additional_kwargs.items() if k != "reasoning_content"}
                out.append(type(msg)(**kwargs))
                continue
            except Exception:
                pass
        out.append(msg)
    return out


def _prepare_llm_call(
    role: str,
    messages: list,
    temp: float,
    schema: Optional[Any],
    tools: Optional[list[dict]],
    reasoning_effort: Optional[str],
    reasoning_budget: Optional[int],
    model: Optional[str],
) -> tuple:
    """Shared prep for invoke + streaming variants: model queue, temperature,
    thinking-strip trimming, schema guideline. Returns (queue, trimmed_messages,
    selected_temp)."""
    category = get_task_category(role)
    queue = TASK_CATEGORIES[category].copy()
    if model:
        normalized_model = model.replace(":", "-")
        queue = [{"provider": "deepseek", "model": normalized_model}] + [x for x in queue if x["model"] != normalized_model]
    # Map canonical model names to the active provider's models (deepseek | opencode)
    queue = [{"provider": item["provider"], "model": _map_model(item["model"])} for item in queue]
    selected_temp = temp

    try:
        from sync.state_sync import safe_get_state
        state = safe_get_state()
        selected_temp = state.get("selected_temp", temp)
    except Exception:
        pass

    trimmed_messages = []
    # Thinking Block Stripping: reasoning from previous text-only turns is NOT
    # needed for the next completion and only re-sends tokens as input. DeepSeek
    # requires reasoning_content to remain on every previous assistant turn that
    # made a tool call, so strip_thinking_from_messages preserves those turns.
    # STRIP_THINKING=0 restores the old echo behavior for A/B comparison.
    strip_thinking = os.environ.get("STRIP_THINKING", "1").lower() not in ("0", "false", "no", "off")
    if strip_thinking:
        messages = strip_thinking_from_messages(messages)
    for msg in messages:
        if hasattr(msg, "content") and isinstance(msg.content, str):
            content = trim_prompt(msg.content)
            if type(msg).__name__ == "SystemMessage":
                content = _compact_prompt(content)

            # Recreate preserving all fields like id, name, tool_calls, additional_kwargs
            kwargs = {}
            for field in ["id", "name", "tool_calls", "additional_kwargs", "tool_call_id"]:
                if hasattr(msg, field):
                    kwargs[field] = getattr(msg, field)

            if (
                strip_thinking
                and type(msg).__name__ == "AIMessage"
                and isinstance(kwargs.get("additional_kwargs"), dict)
                and not _message_has_tool_calls(msg)
            ):
                if "reasoning_content" in kwargs["additional_kwargs"]:
                    kwargs["additional_kwargs"] = {
                        k: v for k, v in kwargs["additional_kwargs"].items() if k != "reasoning_content"
                    }

            trimmed_messages.append(type(msg)(content=content, **kwargs))
        else:
            trimmed_messages.append(msg)

    if schema is not None:
        import json
        schema_json = ""
        if hasattr(schema, "schema_json"):
            schema_json = schema.schema_json()
        elif hasattr(schema, "model_json_schema"):
            schema_json = json.dumps(schema.model_json_schema())
        else:
            schema_json = str(schema)

        guideline = f"\n\nCRITICAL: You MUST respond with a JSON object matching this schema:\n{schema_json}"
        if trimmed_messages:
            last_msg = trimmed_messages[-1]
            if hasattr(last_msg, "content"):
                kwargs = {}
                for field in ["id", "name", "tool_calls", "additional_kwargs", "tool_call_id"]:
                    if hasattr(last_msg, field):
                        kwargs[field] = getattr(last_msg, field)
                last_content = last_msg.content
                if isinstance(last_content, list):
                    new_content = list(last_content) + [{"type": "text", "text": guideline}]
                else:
                    new_content = str(last_content) + guideline
                trimmed_messages[-1] = type(last_msg)(content=new_content, **kwargs)


    # â”€â”€ Vision auto-switch â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€
    # If any message carries an image part and the queued head model cannot see
    # images, swap the ENTIRE queue to the vision model for THIS call only â€”
    # text models would hard-400 on images, so falling back to them after a
    # vision failure is pointless. Next text-only turn reverts to the normal
    # queue automatically. Single vision model by design (premium/experimental);
    # extend VISION_MODELS in llm_config when more arrive at normal prices.
    try:
        if _messages_have_images(trimmed_messages) and not (queue and supports_vision(queue[0]["model"])):
            vision_model = _map_model(DEEPSEEK_MODEL_VISION)
            if supports_vision(vision_model):
                queue = [{"provider": "deepseek", "model": vision_model}]
                log_terminal(f"[LLM] Images present â†’ routed to {vision_model}")
    except Exception:
        pass


    return queue, trimmed_messages, selected_temp


def _stream_emit(chat_id: str, kind: str, text: str, agent: str = "", model: str = "") -> None:
    """Push a delta to the shared StreamBus (best-effort)."""
    if not text:
        return
    try:
        from server.realtime.bus import emit as _bus_emit
        _bus_emit(chat_id, kind, text, agent=agent, model=model)
    except Exception:
        pass
