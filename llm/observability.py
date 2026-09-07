"""llm.observability — Logfire wiring for pydantic-ai agent runs (no-op gated).

Enabled ONLY when ``LOGFIRE_ENABLED=1`` AND a token is available
(``LOGFIRE_TOKEN`` or a configured ``logfire`` project). Without either, every
function is a no-op: no imports of logfire, no behavior change.

When enabled:
  - ``logfire.configure()`` (keyed)
  - ``logfire.instrument_pydantic_ai()`` — per-agent run/tool/model-request spans
  - optional ``logfire.instrument_httpx(capture_all=False)`` behind
    ``LOGFIRE_HTTP_CAPTURE=1`` (never captures payloads/secrets by default)
"""
from __future__ import annotations

import os


def _is_enabled() -> bool:
    flag = os.getenv("LOGFIRE_ENABLED", "0").strip().lower()
    if flag in ("0", "false", "no", "off", ""):
        return False
    token = os.getenv("LOGFIRE_TOKEN", "").strip()
    if token:
        return True
    # Allow a configured local logfire project without an explicit token.
    return os.getenv("LOGFIRE_TOKEN", "").strip() != "" or os.path.isfile(
        os.path.join(
            os.environ.get("HOME", os.path.expanduser("~")),
            ".logfire",
            "config.toml",
        )
    ) if os.getenv("LOGFIRE_NO_TOKEN", "0") == "1" else bool(token)


def instrument_logfire() -> bool:
    """Configure + instrument Logfire. Returns True only if actually enabled."""
    if not _is_enabled():
        return False
    try:
        import logfire

        logfire.configure(token=os.getenv("LOGFIRE_TOKEN", "") or None)
        logfire.instrument_pydantic_ai()
        if os.getenv("LOGFIRE_HTTP_CAPTURE", "0") == "1":
            logfire.instrument_httpx(capture_all=False)
        return True
    except Exception:
        return False
