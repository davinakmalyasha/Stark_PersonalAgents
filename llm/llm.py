"""
LLM Invocation — DeepSeek-only with model-level fallback.
Uses the OpenAI-compatible DeepSeek API at https://api.deepseek.com.
"""
import json
import os
import time
from typing import Any, Optional, Type, TypeVar

from dotenv import load_dotenv
from pydantic import BaseModel
from langchain_openai import ChatOpenAI
from langchain_core.language_models.chat_models import BaseChatModel

# Monkeypatch langchain_openai to preserve reasoning_content for DeepSeek prompt caching and response parsing
try:
    import langchain_openai.chat_models.base as base_mod

    # 1. Outgoing messages (for prompt caching)
    orig_convert = base_mod._convert_message_to_dict
    def patched_convert(message, *args, **kwargs):
        res_dict = orig_convert(message, *args, **kwargs)
        if hasattr(message, "additional_kwargs") and "reasoning_content" in message.additional_kwargs:
            res_dict["reasoning_content"] = message.additional_kwargs["reasoning_content"]
        return res_dict
    base_mod._convert_message_to_dict = patched_convert

    # 2. Incoming messages (for parsing reasoning_content in non-streaming responses)
    orig_convert_dict = base_mod._convert_dict_to_message
    def patched_convert_dict(_dict, *args, **kwargs):
        msg = orig_convert_dict(_dict, *args, **kwargs)
        if isinstance(_dict, dict) and "reasoning_content" in _dict:
            msg.additional_kwargs["reasoning_content"] = _dict["reasoning_content"]
        return msg
    base_mod._convert_dict_to_message = patched_convert_dict

    # 3. Incoming streaming chunks (for parsing reasoning_content in streaming responses)
    orig_convert_delta = base_mod._convert_delta_to_message_chunk
    def patched_convert_delta(_dict, *args, **kwargs):
        msg_chunk = orig_convert_delta(_dict, *args, **kwargs)
        if isinstance(_dict, dict) and "reasoning_content" in _dict:
            msg_chunk.additional_kwargs["reasoning_content"] = _dict["reasoning_content"]
        return msg_chunk
    base_mod._convert_delta_to_message_chunk = patched_convert_delta
except Exception:
    pass

from llm.llm_config import TASK_CATEGORIES, get_task_category, DEEPSEEK_MODEL_VISION, supports_vision

class StrWithMetadata(str):
    def __new__(cls, value, reasoning=None):
        obj = super().__new__(cls, value)
        obj.reasoning_content = reasoning
        return obj

from llm.llm_stats import TokenUsageTracker, update_token_stats
# rate_limiter removed — no artificial API throttling. The model decides pacing.

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
            pass  # console is gone — log to shared_state only
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


def _extract_native_tool_calls(res) -> list[dict]:
    """Extract native function calling tool_calls from an LLM response.

    Returns list of {tool, args, id} dicts in our canonical format (id = the
    provider tool_call_id, needed for ToolMessage round-trips).
    Handles both LangChain's parsed tool_calls and raw additional_kwargs.
    """
    tool_calls = []

    # Path 1: LangChain AIMessage.tool_calls (parsed)
    if hasattr(res, 'tool_calls') and res.tool_calls:
        for tc in res.tool_calls:
            try:
                name = tc.get('name', '') if isinstance(tc, dict) else getattr(tc, 'name', '')
                args = tc.get('args', {}) if isinstance(tc, dict) else getattr(tc, 'args', {})
                cid = tc.get('id', '') if isinstance(tc, dict) else getattr(tc, 'id', '')
                if isinstance(args, str):
                    args = json.loads(args)
                if name:
                    tool_calls.append({"tool": name, "args": args, "id": cid})
            except Exception:
                pass

    # Path 2: Raw additional_kwargs from DeepSeek API response
    if not tool_calls:
        raw_tool_calls = res.additional_kwargs.get('tool_calls', []) if hasattr(res, 'additional_kwargs') else []
        for tc in raw_tool_calls:
            try:
                func = tc.get('function', {}) if isinstance(tc, dict) else {}
                name = func.get('name', '')
                args_str = func.get('arguments', '{}')
                args = json.loads(args_str) if isinstance(args_str, str) else args_str
                cid = tc.get('id', '') if isinstance(tc, dict) else ''
                if name:
                    tool_calls.append({"tool": name, "args": args, "id": cid})
            except Exception:
                pass

    return tool_calls


_PROJECT_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
# Single persisted location: root .deep_agents (shared with every other settings file).
_LLM_PROVIDER_FILE = os.path.join(_PROJECT_ROOT, ".deep_agents", "llm_provider.json")
# Pre-unification twin written beside this module by older builds; adopted once, then retired.
_LEGACY_LLM_PROVIDER_FILE = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".deep_agents", "llm_provider.json")


def _load_persisted_provider() -> str:
    """Read the persisted provider choice (set by the UI switcher), so fresh subprocesses
    (e.g. each eval task) and server restarts keep the same selection."""
    try:
        if os.path.isfile(_LLM_PROVIDER_FILE):
            with open(_LLM_PROVIDER_FILE, encoding="utf-8") as f:
                return (json.load(f).get("provider") or "").strip().lower()
    except Exception:
        pass
    # Legacy twin: migrate its value forward exactly once.
    try:
        if os.path.isfile(_LEGACY_LLM_PROVIDER_FILE):
            with open(_LEGACY_LLM_PROVIDER_FILE, encoding="utf-8") as f:
                legacy = (json.load(f).get("provider") or "").strip().lower()
            if legacy and not os.path.isfile(_LLM_PROVIDER_FILE):
                _save_persisted_provider(legacy)
                try:
                    os.replace(_LEGACY_LLM_PROVIDER_FILE, _LEGACY_LLM_PROVIDER_FILE + ".migrated")
                except OSError:
                    pass
            return legacy
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

    Resolution order: shared_state['llm_provider'] (running server / UI) → persisted file
    (survives subprocesses and restarts) → env default. "opencode" routes paid DeepSeek calls
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
        # Command Code Provider API — OpenAI/Anthropic-compatible gateway.
        # Official endpoint by default; COMMAND_CODE_BASE_URL can point at any
        # compatible gateway/proxy. Model slots are org-scoped catalog slugs;
        # every one is env-overridable without code changes.
        return {
            "base": os.getenv("COMMAND_CODE_BASE_URL", "https://api.commandcode.ai/provider/v1"),
            "key": os.getenv("COMMAND_CODE_API_KEY", ""),
            "flash": os.getenv("COMMAND_CODE_MODEL_FLASH", "deepseek/deepseek-v4-flash"),
            "pro": os.getenv("COMMAND_CODE_MODEL_PRO", "deepseek/deepseek-v4-pro"),
            "vision": os.getenv("COMMAND_CODE_MODEL_VISION", "deepseek/deepseek-v4-flash-vision-exp"),
            "orch": os.getenv("COMMAND_CODE_MODEL_ORCH", "laguna-s-2.1-free"),
            "light": os.getenv("COMMAND_CODE_MODEL_LIGHT", "laguna-s-2.1-free"),
        }
    if provider == "opencode":
        # Go subscription endpoint — /zen/v1 is the generic/pay-per-use route that has no
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
    """Map a canonical model name (deepseek-v4-flash/pro/vision-exp) to the active provider's name."""
    cfg = _resolve_llm_provider()
    if "/" in model:
        # Org-scoped slug (e.g. "deepseek/deepseek-v4-flash", "laguna-s-2.1-free")
        # is already provider-specific — pass through untouched.
        return model
    if model == "deepseek-v4-flash":
        return cfg["flash"]
    if model == "deepseek-v4-pro":
        return cfg["pro"]
    if model in (DEEPSEEK_MODEL_VISION, "deepseek-v4-flash-vision-exp"):
        # Per-provider vision slot: same slug on the DeepSeek API and OpenCode
        # Go listings; the command gateway names its own.
        return cfg.get("vision") or DEEPSEEK_MODEL_VISION
    return model


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

    # No per-call max_tokens — the model decides how much to output.
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

    # Configure native thinking mode dynamically by role/model.
    # Only send thinking parameters if the model supports it or is a fallback model.
    is_reasoning_model = any(x in model.lower() for x in ["r1", "pro", "o1", "o3", "reasoning", "thinking", "fallback", "automatic", "flash"])

    if reasoning_effort is not None:
        # Explicit caller intent always wins and is always sent — the
        # mode-aware ladders know the target model better than this heuristic.
        effort = str(reasoning_effort).strip().lower()
        if effort in ("disabled", "none", ""):
            kwargs["extra_body"] = {"thinking": {"type": "disabled"}}
        else:
            budget_override = reasoning_budget
            if effort == "max":
                # Portable ceiling: effort tops out at "high" plus an explicit
                # token budget the vendor can honor (REASONING_MAX_BUDGET).
                effort = "high"
                if budget_override is None:
                    budget_override = int(os.getenv("REASONING_MAX_BUDGET", "32768"))
            kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
            kwargs["reasoning_effort"] = effort
            if budget_override is not None:
                kwargs["extra_body"]["thinking"]["budget_tokens"] = int(budget_override)
    elif is_reasoning_model:
        # No explicit effort: role-based defaults. Developer-like/heavy roles
        # think at high; everyone else runs with thinking disabled.
        is_orchestrator = (
            role in ["Developer", "DeveloperFixing"]
            or (role and any(x in role.lower() for x in ["developer", "orchestrator", "designer"]))
        )
        is_heavy = is_orchestrator or (
            role in ["Supervisor", "sa", "Coder"]
            or (role and any(x in role.lower() for x in ["fixing", "coder", "architect", "complex"]))
        )

        if is_orchestrator:
            kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
            kwargs["reasoning_effort"] = "high"
        elif is_heavy:
            kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
            kwargs["reasoning_effort"] = "high"
        else:
            kwargs["extra_body"] = {"thinking": {"type": "disabled"}}

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
    def __init__(self):
        self.lock = threading.Lock()
        self.active_requests = 0
        self.max_concurrency = 4
        self.cooldown_until = 0.0
        
    def acquire(self):
        # Spin/wait if active requests exceed limit
        while True:
            with self.lock:
                now = time.time()
                limit = 1 if now < self.cooldown_until else self.max_concurrency
                if self.active_requests < limit:
                    self.active_requests += 1
                    break
            time.sleep(0.1)
            
    def release(self):
        with self.lock:
            self.active_requests = max(0, self.active_requests - 1)
            
    def trigger_cooldown(self, duration=30):
        with self.lock:
            self.cooldown_until = time.time() + duration

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
    """Context-window errors can never be fixed by retrying — report and move on."""
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
    # Permanent client errors — never retry
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
    """CF3: CACHE_DEBUG=1 → log a payload-content hash + cache tokens per call.

    DeepSeek prefix caching is a byte-prefix match. If cache_hit stays 0 across
    repeated calls with identical prefixes, a silent invalidator is at work —
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

            _throttle.acquire()
            try:
                res = llm.invoke(
                    messages,
                    config={"callbacks": [tracker]},
                )
            finally:
                _throttle.release()
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
                        safe_append_live_log(f"  → Tools: {', '.join(tools_called)}")
                    elif len(val_str) < 200:
                        safe_append_live_log(val_str)
                    else:
                        safe_append_live_log(f"  → Response: {len(val_str)} chars")
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
                        f"[{model}] Transient error, retrying (attempt {attempt+1}/3) — waiting {wait}s...\n"
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
                        _throttle.acquire()
                        try:
                            res = llm.invoke(
                                retry_msgs,
                                config={"callbacks": [tracker]},
                            )
                        finally:
                            _throttle.release()
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

    # ── Vision auto-switch ────────────────────────────────────────────────────
    # If any message carries an image part and the queued head model cannot see
    # images, swap the ENTIRE queue to the vision model for THIS call only —
    # text models would hard-400 on images, so falling back to them after a
    # vision failure is pointless. Next text-only turn reverts to the normal
    # queue automatically. Single vision model by design (premium/experimental);
    # extend VISION_MODELS in llm_config when more arrive at normal prices.
    try:
        if _messages_have_images(trimmed_messages) and not (queue and supports_vision(queue[0]["model"])):
            vision_model = _map_model(DEEPSEEK_MODEL_VISION)
            if supports_vision(vision_model):
                queue = [{"provider": "deepseek", "model": vision_model}]
                log_terminal(f"[LLM] Images present → routed to {vision_model}")
    except Exception:
        pass

    return queue, trimmed_messages, selected_temp


def _stream_emit(chat_id: str, kind: str, text: str, agent: str = "", model: str = "") -> None:
    """Push a delta to the shared StreamBus (best-effort)."""
    if not text:
        return
    try:
        from stream_bus import emit as _bus_emit
        _bus_emit(chat_id, kind, text, agent=agent, model=model)
    except Exception:
        pass


def invoke_messages_with_fallback_stream(
    role: str,
    messages: list,
    temp: float = 0.7,
    where: Optional[str] = None,
    schema: Optional[Any] = None,
    tools: Optional[list[dict]] = None,
    reasoning_effort: Optional[str] = None,
    reasoning_budget: Optional[int] = None,
    model: Optional[str] = None,
) -> Any:
    """Streaming variant of invoke_messages_with_fallback.

    Emits text/reasoning deltas into the shared StreamBus (chat-scoped) so the
    API server can forward them as SSE ``delta`` frames — the frontend shows the
    reply as it is generated (P10). Returns the EXACT same result shape as the
    non-streaming variant; if streaming fails for any reason, this falls back to
    the non-streaming path for that call (zero behavior change when disabled).
    """
    try:
        from stream_bus import enabled as _stream_bus_enabled
        if not _stream_bus_enabled():
            return invoke_messages_with_fallback(
                role, messages, temp=temp, where=where, schema=schema, tools=tools,
                reasoning_effort=reasoning_effort, reasoning_budget=reasoning_budget, model=model,
            )
    except Exception:
        return invoke_messages_with_fallback(
            role, messages, temp=temp, where=where, schema=schema, tools=tools,
            reasoning_effort=reasoning_effort, reasoning_budget=reasoning_budget, model=model,
        )

    queue, trimmed_messages, selected_temp = _prepare_llm_call(
        role, messages, temp, schema, tools, reasoning_effort, reasoning_budget, model
    )
    errors_by_model: dict[str, str] = {}
    chat_id = _effective_stream_chat_id()

    def _emit(kind: str, text: str) -> None:
        _stream_emit(chat_id, kind, text, agent=role, model="")

    def _build_native_tool_calls(chunks: list[dict]) -> list[dict]:
        out = []
        for tc in chunks:
            name = tc.get("name") or ""
            args_str = tc.get("args") or "{}"
            cid = tc.get("id") or ""
            try:
                args = json.loads(args_str) if isinstance(args_str, str) else args_str
            except Exception:
                args = {}
            if name:
                out.append({"tool": name, "args": args, "id": cid})
        return out

    def _stream_once(m: str, response_format: Optional[dict]):
        """One streaming attempt; returns (text, reasoning, tool_chunks, tracker)."""
        llm = _get_deepseek_client(
            m, selected_temp, role,
            response_format=response_format,
            tools=tools, reasoning_effort=reasoning_effort,
            reasoning_budget=reasoning_budget, messages=trimmed_messages,
        )
        try:
            # Ask the provider to include usage in the final streamed chunk.
            llm.stream_usage = True
        except Exception as _usage_e:
            # stream_usage unsupported on this client — usage falls back to
            # the token tracker callbacks.
            print(f"[llm] stream_usage not supported ({_usage_e}) — using callback usage.")
        tracker = TokenUsageTracker()
        text_parts: list[str] = []
        reasoning_parts: list[str] = []
        tool_chunks: list[dict] = []
        _throttle.acquire()
        try:
            for chunk in llm.stream(trimmed_messages, config={"callbacks": [tracker]}):
                delta = getattr(chunk, "content", None)
                if isinstance(delta, list):
                    delta = "".join(
                        str(p.get("text", p)) if isinstance(p, dict) else str(p)
                        for p in delta
                    )
                if delta:
                    text_parts.append(delta)
                    _emit("text", delta)
                reasoning = getattr(chunk, "additional_kwargs", {}).get("reasoning_content") if hasattr(chunk, "additional_kwargs") else None
                if reasoning:
                    reasoning_parts.append(reasoning)
                    _emit("reasoning", reasoning)
                for tc in (getattr(chunk, "tool_call_chunks", None) or []):
                    idx = tc.get("index", len(tool_chunks))
                    while len(tool_chunks) <= idx:
                        tool_chunks.append({"name": "", "args": "", "id": ""})
                    if tc.get("name"):
                        tool_chunks[idx]["name"] += tc["name"]
                    if tc.get("id"):
                        tool_chunks[idx]["id"] += tc["id"]
                    if tc.get("args"):
                        tool_chunks[idx]["args"] += tc["args"]
        finally:
            _throttle.release()
        text = "".join(text_parts)
        reasoning = "".join(reasoning_parts) or None
        if not text and reasoning:
            text = reasoning
        return text, reasoning, tool_chunks, tracker

    for item in queue:
        m = item["model"]
        try:
            if role != "MemoryArchivist":
                action = where[:120].replace('\n', ' ').strip() if where else "processing..."
                log_terminal(f"[{role}] {action}")
            response_format = {"type": "json_object"} if schema is not None else None
            text, reasoning, tool_chunks, tracker = _stream_once(m, response_format)

            input_tok = tracker.input_tokens or 0
            output_tok = tracker.output_tokens or 0
            cache_hit = tracker.cache_hit_tokens or 0
            update_token_stats(
                role, m, input_tok, output_tok, cache_hit, where,
                reasoning_chars=len(reasoning) if reasoning else 0,
                reasoning_effort=reasoning_effort,
                reasoning_budget=reasoning_budget,
            )
            _log_cache_debug(
                trimmed_messages, tracker, role, m,
                tools=tools, reasoning_effort=reasoning_effort,
                reasoning_budget=reasoning_budget,
            )
            _emit("end", "\n")

            native_tool_calls = _build_native_tool_calls(tool_chunks)
            if tools is not None:
                if native_tool_calls:
                    return (text or "", native_tool_calls, reasoning)
                elif text:
                    return (text, [], reasoning)  # Text response, no tool calls — agent is done
                else:
                    log_terminal(f"[{m}] Empty response, trying next...\n")
            else:
                if text:
                    if schema is not None:
                        text = _parse_schema_response(text, schema)
                    else:
                        text = StrWithMetadata(text, reasoning=reasoning)
                    return text
                log_terminal(f"[{m}] Empty response, trying next...\n")
        except Exception as e:
            import traceback
            tb = traceback.format_exc()
            err_str = str(e)
            errors_by_model[m] = err_str[:300]
            log_terminal(f"[{m}] Failed: {err_str}\n{tb}\n")

            # Rate-limit / transient server error retry (same policy as non-streaming)
            if _is_retryable_http_error(err_str):
                _throttle.trigger_cooldown(30)
                for attempt in range(3):
                    wait = _handle_rate_limit(err_str, attempt)
                    log_terminal(
                        f"[{m}] Transient error, retrying (attempt {attempt+1}/3) — waiting {wait}s...\n"
                    )
                    time.sleep(wait)
                    try:
                        response_format = {"type": "json_object"} if schema is not None else None
                        text, reasoning, tool_chunks, tracker = _stream_once(m, response_format)
                        input_tok = tracker.input_tokens or 0
                        output_tok = tracker.output_tokens or 0
                        cache_hit = tracker.cache_hit_tokens or 0
                        update_token_stats(
                            role, m, input_tok, output_tok, cache_hit, where,
                            reasoning_chars=len(reasoning) if reasoning else 0,
                            reasoning_effort=reasoning_effort,
                            reasoning_budget=reasoning_budget,
                        )
                        _log_cache_debug(
                            trimmed_messages, tracker, role, m,
                            tools=tools, reasoning_effort=reasoning_effort,
                            reasoning_budget=reasoning_budget,
                        )
                        _emit("end", "\n")
                        native_tool_calls = _build_native_tool_calls(tool_chunks)
                        if tools is not None:
                            if native_tool_calls:
                                return (text or "", native_tool_calls, reasoning)
                            elif text:
                                return (text, [], reasoning)
                        elif text:
                            if schema is not None:
                                text = _parse_schema_response(text, schema)
                            else:
                                text = StrWithMetadata(text, reasoning=reasoning)
                            return text
                    except Exception as retry_e:
                        err_str = str(retry_e)
                        errors_by_model[m] = f"Rate-limit retry {attempt+1} failed: {err_str[:200]}"
                        log_terminal(f"[{m}] Retry {attempt+1} failed: {retry_e}\n")
                        if not _is_retryable_http_error(err_str):
                            break

            # P10: streaming failed (non-transient) — fall back to the plain
            # non-streaming path for THIS call. If the API is genuinely down,
            # the fallback raises the same exhaustion error.
            log_terminal(f"[{m}] Streaming failed ({err_str[:120]}) — falling back to non-streaming.")
            return invoke_messages_with_fallback(
                role, messages, temp=temp, where=where, schema=schema, tools=tools,
                reasoning_effort=reasoning_effort, reasoning_budget=reasoning_budget, model=model,
            )

    detail = "; ".join(f"{m}: {e}" for m, e in errors_by_model.items())
    raise RuntimeError(
        f"All DeepSeek models exhausted for role '{role}'. "
        f"Errors: {detail}. "
        "Check your DEEPSEEK_API_KEY and account balance at https://platform.deepseek.com."
    )


def invoke_messages_with_fallback(
    role: str,
    messages: list,
    temp: float = 0.7,
    where: Optional[str] = None,
    schema: Optional[Any] = None,
    tools: Optional[list[dict]] = None,
    reasoning_effort: Optional[str] = None,
    reasoning_budget: Optional[int] = None,
    model: Optional[str] = None,
) -> Any:
    """
    Invoke LLM with multi-turn messages and optional native function calling.
    Returns (content_text, tool_calls) tuple when tools are provided,
    or just content_text string when tools=None (legacy mode).
    """

    queue, trimmed_messages, selected_temp = _prepare_llm_call(
        role, messages, temp, schema, tools, reasoning_effort, reasoning_budget, model
    )
    errors_by_model: dict[str, str] = {}

    for item in queue:
        model = item["model"]
        try:
            if role != "MemoryArchivist":
                # Show what the agent is working on, not just the model name
                action = where[:120].replace('\n', ' ').strip() if where else "processing..."
                log_terminal(f"[{role}] {action}")
            response_format = {"type": "json_object"} if schema is not None else None
            llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, tools=tools, reasoning_effort=reasoning_effort, reasoning_budget=reasoning_budget, messages=trimmed_messages)
            tracker = TokenUsageTracker()
            _throttle.acquire()
            try:
                res = llm.invoke(trimmed_messages, config={"callbacks": [tracker]})
            finally:
                _throttle.release()
            reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
            if reasoning:
                log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
            # Extract native tool_calls if present (structured function calling)
            native_tool_calls = _extract_native_tool_calls(res)
            # DeepSeek with thinking enabled may return reasoning_content but empty content.
            # Fall back to reasoning_content if content is empty.
            val = _extract_text(getattr(res, "content", None))
            if not val and reasoning:
                val = reasoning
            update_token_stats(
                role, model, tracker.input_tokens, tracker.output_tokens, tracker.cache_hit_tokens, where,
                reasoning_chars=len(reasoning) if reasoning else 0,
                reasoning_effort=reasoning_effort,
                reasoning_budget=reasoning_budget,
            )
            _log_cache_debug(
                trimmed_messages, tracker, role, model,
                tools=tools, reasoning_effort=reasoning_effort,
                reasoning_budget=reasoning_budget,
            )
            # Return structured result: tuple with (text, tool_calls, reasoning) if tools active
            if tools is not None:
                if native_tool_calls:
                    return (val or "", native_tool_calls, reasoning)
                elif val:
                    return (val, [], reasoning)  # Text response, no tool calls — agent is done
                else:
                    log_terminal(f"[{model}] Empty response, trying next...\n")
            else:
                if val:
                    if schema is not None:
                        val = _parse_schema_response(val, schema)
                    else:
                        val = StrWithMetadata(val, reasoning=reasoning)
                    return val
                log_terminal(f"[{model}] Empty response, trying next...\n")
        except Exception as e:
            import traceback
            tb = traceback.format_exc()
            err_str = str(e)
            errors_by_model[model] = err_str[:300]
            log_terminal(f"[{model}] Failed: {err_str}\n{tb}\n")

            # Rate-limit / transient server error retry
            if _is_retryable_http_error(err_str):
                _throttle.trigger_cooldown(30)
                for attempt in range(3):
                    wait = _handle_rate_limit(err_str, attempt)
                    log_terminal(
                        f"[{model}] Transient error, retrying (attempt {attempt+1}/3) — waiting {wait}s...\n"
                    )
                    time.sleep(wait)
                    try:
                        response_format = {"type": "json_object"} if schema is not None else None
                        llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, tools=tools, reasoning_effort=reasoning_effort, reasoning_budget=reasoning_budget, messages=trimmed_messages)
                        tracker = TokenUsageTracker()
                        _throttle.acquire()
                        try:
                            res = llm.invoke(
                                trimmed_messages, config={"callbacks": [tracker]}
                            )
                        finally:
                            _throttle.release()
                        reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
                        if reasoning:
                            log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
                        native_tool_calls = _extract_native_tool_calls(res)
                        val = _extract_text(getattr(res, "content", None))
                        if not val and reasoning:
                            val = reasoning
                        update_token_stats(
                            role, model,
                            tracker.input_tokens, tracker.output_tokens,
                            tracker.cache_hit_tokens,
                            where,
                            reasoning_chars=len(reasoning) if reasoning else 0,
                            reasoning_effort=reasoning_effort,
                            reasoning_budget=reasoning_budget,
                        )
                        _log_cache_debug(
                            trimmed_messages, tracker, role, model,
                            tools=tools, reasoning_effort=reasoning_effort,
                            reasoning_budget=reasoning_budget,
                        )
                        if tools is not None:
                            if native_tool_calls:
                                return (val or "", native_tool_calls, reasoning)
                            elif val:
                                return (val, [], reasoning)
                        elif val:
                            if schema is not None:
                                val = _parse_schema_response(val, schema)
                            else:
                                val = StrWithMetadata(val, reasoning=reasoning)
                            return val
                    except Exception as retry_e:
                        err_str = str(retry_e)
                        errors_by_model[model] = f"Rate-limit retry {attempt+1} failed: {err_str[:200]}"
                        log_terminal(f"[{model}] Retry {attempt+1} failed: {retry_e}\n")
                        if not _is_retryable_http_error(err_str):
                            break

    detail = "; ".join(f"{m}: {e}" for m, e in errors_by_model.items())
    raise RuntimeError(
        f"All DeepSeek models exhausted for role '{role}'. "
        f"Errors: {detail}. "
        "Check your DEEPSEEK_API_KEY and account balance at https://platform.deepseek.com."
    )

