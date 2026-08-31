"""
llm/llm.py --- Provider resolution + stream-bus + retry helpers.

P0-LLM: the LangChain transport (ChatOpenAI / invoke_with_fallback /
ConcurrencyThrottle / regex JSON parsing) is deleted. Live surface kept here:

- ``log_terminal`` — console + shared_state live log.
- ``_load/_save_persisted_provider`` — UI provider switcher persistence
  (``.deep_agents/llm_provider.json``).
- ``_resolve_llm_provider`` / ``_map_model`` — gateway resolution + canonical
  model slug mapping used by ``pai_transport`` / ``deps`` / ``health``.
- ``_stream_emit`` / ``_effective_stream_chat_id`` — shared StreamBus delta
  emission for the native agent loops.
- ``_is_retryable_http_error`` / ``_parse_retry_after`` — transient-error
  classification used by the native ``model_request_error`` hook.

No langchain imports in this module (fixed-concept banned list).
"""
import json
import os

from dotenv import load_dotenv

from llm.llm_config import DEEPSEEK_MODEL_VISION

load_dotenv(override=True)

# Override to route DeepSeek calls through a compatible gateway (e.g. OpenCode Go:
# DEEPSEEK_BASE_URL=https://opencode.ai/zen/go/v1 with OPENCODE_API_KEY set) temporarily.
DEEPSEEK_BASE_URL = os.getenv("DEEPSEEK_BASE_URL", "https://api.deepseek.com")


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
            "orch": os.getenv("COMMAND_CODE_MODEL_ORCH", "xiaomi/mimo-v2.5"),
            "light": os.getenv("COMMAND_CODE_MODEL_LIGHT", "laguna-s-2.1-free"),
            "audio": os.getenv("COMMAND_CODE_MODEL_AUDIO", "xiaomi/mimo-v2.5"),
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
    """Map a canonical model name (deepseek-v4-flash/pro/vision-exp) to the active provider's name.

    Accepts both the colon slug used by agent constants (``deepseek:v4-flash``)
    and the dash canonical (``deepseek-v4-flash``). Org-scoped slugs
    (``deepseek/deepseek-v4-flash``, ``xiaomi/mimo-v2.5``) pass through.
    """
    cfg = _resolve_llm_provider()
    if "/" in model:
        # Org-scoped slug (e.g. "deepseek/deepseek-v4-flash", "laguna-s-2.1-free")
        # is already provider-specific — pass through untouched.
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


# ── Retry classification (used by native model_request_error hook) ─────────

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


def _effective_stream_chat_id() -> str:
    """Active chat id for StreamBus emission (global shared_state)."""
    try:
        from sync.state_sync import shared_state
        return str(shared_state.get("chat_id") or "")
    except Exception:
        return ""


def _stream_emit(chat_id: str, kind: str, text: str, agent: str = "", model: str = "") -> None:
    """Push a delta to the shared StreamBus (best-effort)."""
    if not text:
        return
    try:
        from server.realtime.bus import emit as _bus_emit
        _bus_emit(chat_id, kind, text, agent=agent, model=model)
    except Exception:
        pass
