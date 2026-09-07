"""
free_providers.py --- Multi-provider free LLM transport (zero cost).

Engine extracted from the retired FreeProviders agent: provider configs, rate
limits, rotation/retry routing, native OpenAI-compatible tool_calls parsing,
and a live model-catalog refresher. Transport ONLY - no prompts, no agent
loop, no CLI.

Consumers:
    from llm.free_providers import FreeProviders, complete, chat, configure
    from llm.free_providers import list_models, refresh_catalog
    from llm.free_providers import (
        FreeProviderError, AllProvidersExhaustedError, ContextWindowExceededError,
    )

Env knobs: FREE_PROVIDER_REQUEST_TIMEOUT / FREE_AGENT_REQUEST_TIMEOUT,
FREE_PROVIDER_MAX_RETRY_CYCLES / FREE_AGENT_MAX_RETRY_CYCLES.
Dependencies: none beyond stdlib (urllib).
"""

import base64
import itertools
import json
import os
import re
import sys
import time
from dataclasses import dataclass, field
from typing import Optional

PROVIDER_REQUEST_TIMEOUT = float(os.getenv(
    "FREE_PROVIDER_REQUEST_TIMEOUT",
    os.getenv("FREE_AGENT_REQUEST_TIMEOUT", "30"),
))
# Idle ceiling for NON-streaming completions: seconds without ANY response
# byte before the attempt is aborted and the next provider gets its turn.
# A call that is actively producing is never cut by this - only true silence.
IDLE_TIMEOUT = float(os.getenv(
    "FREE_PROVIDER_IDLE_TIMEOUT",
    os.getenv("FREE_PROVIDER_DEADLINE", "90"),
))
MAX_PROVIDER_RETRY_CYCLES = int(os.getenv(
    "FREE_PROVIDER_MAX_RETRY_CYCLES",
    os.getenv("FREE_AGENT_MAX_RETRY_CYCLES", "2"),
))

# Load .env file if python-dotenv is available
try:
    from dotenv import load_dotenv
    load_dotenv()
except ImportError:
    pass

# ----------------------------------------------------------------------
#  PHASE 1+2: PROVIDER CONFIGS + MODEL CATALOG (externalized config)
# ----------------------------------------------------------------------
# Single source of truth: config/free_catalog.json — providers, rate limits,
# priorities, and the model catalog. Edit that file to change what the free
# pool offers; this module loads it once at import time. If the file is
# missing or unreadable, the module degrades to an empty catalog (free pool
# simply routes nothing) rather than crashing import.

_FREE_CATALOG_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "config",
    "free_catalog.json",
)


def _load_free_catalog() -> tuple[dict, list]:
    """Load {providers, models} from config/free_catalog.json (read-only)."""
    try:
        with open(_FREE_CATALOG_PATH, "r", encoding="utf-8") as _f:
            _data = json.load(_f)
        _providers = _data.get("providers") or {}
        _models = _data.get("models") or []
        if not isinstance(_providers, dict) or not isinstance(_models, list):
            return {}, []
        return _providers, _models
    except Exception:
        return {}, []


PROVIDER_CONFIGS, MODEL_CATALOG = _load_free_catalog()
# ----------------------------------------------------------------------
#  PHASE 3: CORE BUILDING BLOCKS
# ----------------------------------------------------------------------

class FreeProviderError(Exception):
    """Base error for FreeProviders."""
    def __init__(self, message: str, provider: str = "", status: int = 0):
        self.provider = provider
        self.status = status
        super().__init__(f"[{provider}] {message}" if provider else message)


class AllProvidersExhaustedError(FreeProviderError):
    """All providers failed for this request."""


class ContextWindowExceededError(FreeProviderError):
    """A provider rejected a request because its real context is too small."""


def _is_context_window_error(error: BaseException) -> bool:
    text = str(error).lower()
    markers = (
        "context length", "context window", "maximum context", "max context",
        "too many tokens", "prompt is too long", "request too large",
        "token limit", "input token",
    )
    return any(marker in text for marker in markers)


def _sleep_with_abort(seconds: float, abort_check=None) -> None:
    """Back off without making a retry-until-success request uncancellable."""
    deadline = time.monotonic() + max(0.0, float(seconds))
    while True:
        if callable(abort_check) and abort_check():
            raise AllProvidersExhaustedError("Free-agent request cancelled.")
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            return
        time.sleep(min(1.0, remaining))


class RateLimitTracker:
    """Simple sliding-window rate limiter per provider.

    Tracks RPM (requests per minute) and RPD (requests per day).
    """

    def __init__(self, rpm: int, rpd: int = 0, tpm: int = 0):
        self.max_rpm = rpm
        self.max_rpd = rpd
        self.max_tpm = tpm
        self._window: list[float] = []  # timestamps in current minute
        self._daily: list[float] = []   # timestamps in current day
        self._cooldown_until: float = 0.0

    def can_request(self) -> bool:
        now = time.time()
        # Cooldown check
        if now < self._cooldown_until:
            return False
        # Clear old entries
        self._window = [t for t in self._window if now - t < 60]
        self._daily = [t for t in self._daily if now - t < 86400]
        # Check limits
        if self.max_rpm and len(self._window) >= self.max_rpm:
            return False
        if self.max_rpd and len(self._daily) >= self.max_rpd:
            return False
        return True

    def record_request(self):
        now = time.time()
        self._window.append(now)
        self._daily.append(now)

    def trigger_cooldown(self, seconds: float = 30.0):
        self._cooldown_until = time.time() + seconds


# ----------------------------------------------------------------------
#  PHASE 4: PROVIDER CLIENT
# ----------------------------------------------------------------------

class ProviderClient:
    """Unified client for any free LLM provider.

    Handles every OpenAI-compatible provider listed in PROVIDER_CONFIGS.
    All share the OpenAI-compatible /chat/completions endpoint.
    """

    def __init__(self, provider_name: str):
        if provider_name not in PROVIDER_CONFIGS:
            raise ValueError(f"Unknown provider: {provider_name}. Options: {list(PROVIDER_CONFIGS.keys())}")

        self.name = provider_name
        self.cfg = dict(PROVIDER_CONFIGS[provider_name])
        self.api_keys = self._load_keys()
        if not self.api_keys:
            raise ValueError(
                f"No API keys for {self.cfg['name']}. "
                f"Set {self.cfg['env_key']} (comma-separated) "
                f"or {self.cfg['env_key_fallback']} env var."
            )
        self._key_cycle = itertools.cycle(self.api_keys)
        self.rate_limiter = RateLimitTracker(
            rpm=self.cfg.get("rpm", 30),
            rpd=self.cfg.get("rpd", 0),
            tpm=self.cfg.get("tpm", 0),
        )
        self.default_model = self.cfg["default_model"]
        # Health memory: auth-dead providers are skipped for the pool's
        # lifetime; transport failures escalate their cooldown.
        self.auth_dead = False
        self.fail_streak = 0

    # â”€â”€ key loading â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    def _load_keys(self) -> list[str]:
        keys_str = os.getenv(self.cfg["env_key"], "")
        fallback = os.getenv(self.cfg["env_key_fallback"], "")
        if not keys_str and fallback:
            return [fallback.strip()]
        if not keys_str and not fallback:
            # Some providers (e.g. Kilo) work without an API key
            if self.cfg.get("api_key_optional", False):
                return [""]
            return []
        keys = [k.strip() for k in keys_str.replace(";", ",").split(",") if k.strip()]
        return keys

    def _next_key(self) -> str:
        return next(self._key_cycle)

    # â”€â”€ URL building â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    def _build_url(self) -> str:
        base = self.cfg["base_url"]
        # Handle Cloudflare account ID interpolation
        if "{account_id}" in base:
            account_id = os.getenv("CLOUDFLARE_ACCOUNT_ID", "")
            if not account_id:
                raise FreeProviderError("CLOUDFLARE_ACCOUNT_ID not set", provider=self.name)
            base = base.replace("{account_id}", account_id)
        # Ensure no double slash
        base = base.rstrip("/")
        return base + "/chat/completions"

    # â”€â”€ header / body building â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    def _build_headers(self, extra: Optional[dict] = None) -> dict:
        key = self._next_key()
        headers = {
            "Content-Type": "application/json",
        }
        if key:
            headers["Authorization"] = f"Bearer {key}"
        if self.name == "openrouter":
            headers["X-Title"] = os.getenv("OPENROUTER_APP_NAME", "free_providers")
            referer = os.getenv("OPENROUTER_APP_URL", "")
            if referer:
                headers["HTTP-Referer"] = referer
        if self.name == "opencode":
            # Cloudflare blocks non-browser User-Agents (error 1010)
            headers["User-Agent"] = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"
        if extra:
            headers.update(extra)
        return headers

    @staticmethod
    def _image_to_data_uri(image_path: str) -> str:
        ext = os.path.splitext(image_path)[1].lower().lstrip(".")
        mime_map = {
            "jpg": "image/jpeg", "jpeg": "image/jpeg",
            "png": "image/png", "gif": "image/gif",
            "webp": "image/webp", "bmp": "image/bmp",
        }
        mime = mime_map.get(ext, "image/png")
        with open(image_path, "rb") as f:
            data = base64.b64encode(f.read()).decode("utf-8")
        return f"data:{mime};base64,{data}"

    def _build_messages(
        self,
        prompt: str,
        system_prompt: Optional[str] = None,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        messages: Optional[list[dict]] = None,
    ) -> list[dict]:
        if messages is not None:
            # Native tool turns must preserve the exact assistant/tool message
            # sequence.  Do not collapse it back into a prompt string.
            return [dict(message) for message in messages]

        msgs = []
        if system_prompt:
            msgs.append({"role": "system", "content": system_prompt})

        has_images = (images and len(images) > 0) or (image_urls and len(image_urls) > 0)
        if has_images:
            content_parts: list[dict] = [{"type": "text", "text": prompt}]
            if images:
                for path in images:
                    content_parts.append({
                        "type": "image_url",
                        "image_url": {"url": self._image_to_data_uri(path)},
                    })
            if image_urls:
                for url in image_urls:
                    content_parts.append({
                        "type": "image_url",
                        "image_url": {"url": url},
                    })
            msgs.append({"role": "user", "content": content_parts})
        else:
            msgs.append({"role": "user", "content": prompt})

        return msgs

    def _build_body(
        self,
        prompt: str,
        stream: bool = False,
        system_prompt: Optional[str] = None,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        messages: Optional[list[dict]] = None,
        tools: Optional[list[dict]] = None,
        tool_choice=None,
        **overrides,
    ) -> dict:
        model = overrides.pop("model", self.default_model)
        params = {
            "temperature": 0.7,
            "max_tokens": 4096,
            "top_p": 0.9,
        }
        params.update(overrides)

        body: dict = {
            "model": model,
            "messages": self._build_messages(
                prompt, system_prompt, images, image_urls, messages
            ),
            "stream": stream,
            **params,
        }
        if tools is not None:
            body["tools"] = tools
        if tool_choice is not None:
            body["tool_choice"] = tool_choice
        return body

    # â”€â”€ HTTP â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    def _post(self, body: dict, timeout: Optional[float] = None) -> dict:
        import urllib.error
        import urllib.request

        url = self._build_url()
        headers = self._build_headers()
        payload = json.dumps(body).encode()

        req = urllib.request.Request(url, data=payload, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=timeout or IDLE_TIMEOUT) as resp:
                body = json.loads(resp.read().decode())
            if self.cfg.get("response_wrapped") and isinstance(body, dict) and isinstance(body.get("data"), dict):
                body = body["data"]
            return body
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode()
            try:
                detail = json.loads(raw)
                msg = detail.get("error", {}).get("message", raw)
            except json.JSONDecodeError:
                msg = raw
            raise FreeProviderError(msg, provider=self.name, status=exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise FreeProviderError(f"{self.name} request failed or timed out: {exc}", provider=self.name, status=0) from exc

    def _parse_reply(self, body: dict) -> str:
        choices = body.get("choices") or [{}]
        choice = choices[0] if isinstance(choices, list) else {}
        msg = choice.get("message", {})
        content = msg.get("content", "")
        if isinstance(content, list):
            content = "".join(
                part.get("text", "") if isinstance(part, dict) else str(part)
                for part in content
            )
        return content.strip() if content else ""

    @staticmethod
    def _parse_tool_calls(body: dict) -> list[dict]:
        """Normalize OpenAI-compatible native function calls.

        The model-facing protocol is the provider's structured ``tool_calls``
        field.  Invalid argument payloads are ignored as malformed calls rather
        than reinterpreted as text instructions.
        """
        choices = body.get("choices") or []
        if not isinstance(choices, list) or not choices:
            return []
        message = choices[0].get("message") or {}
        raw_calls = message.get("tool_calls") or []
        if not isinstance(raw_calls, list):
            return []

        calls = []
        for index, call in enumerate(raw_calls):
            if not isinstance(call, dict):
                continue
            function = call.get("function") or {}
            name = str(function.get("name") or "").strip()
            if not name:
                continue
            raw_args = function.get("arguments", {})
            if isinstance(raw_args, str):
                try:
                    raw_args = json.loads(raw_args or "{}")
                except json.JSONDecodeError:
                    continue
            if not isinstance(raw_args, dict):
                continue
            calls.append({
                "id": str(call.get("id") or f"call_{index + 1}"),
                "name": name,
                "args": raw_args,
            })
        return calls

    def _parse_model_used(self, body: dict) -> str:
        return body.get("model", self.default_model)

    # â”€â”€ public API â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    def chat(self, prompt: str, **overrides) -> str:
        """Text-only chat. Returns reply string."""
        body = self._build_body(prompt, stream=False, **overrides)
        resp = self._post(body)
        return self._parse_reply(resp)

    @staticmethod
    def _extract_think_tags(content: str) -> tuple[str, str]:
        """Pull inline <think>...</think> blocks out of content.

        Some reasoning models (GLM, Qwen) embed their chain-of-thought in the
        content itself instead of a reasoning field. Returns
        ``(clean_content, extracted_reasoning)``."""
        reasoning_parts: list[str] = []

        def _repl(m: "re.Match[str]") -> str:
            reasoning_parts.append(m.group(1).strip())
            return ""

        cleaned = re.sub(r"<think>(.*?)</think>", _repl, content,
                         flags=re.DOTALL | re.IGNORECASE)
        return cleaned.strip(), "\n\n".join(p for p in reasoning_parts if p)

    def chat_with_info(
        self,
        prompt: str = "",
        messages: Optional[list[dict]] = None,
        tools: Optional[list[dict]] = None,
        tool_choice=None,
        idle_timeout: Optional[float] = None,
        **overrides,
    ) -> dict:
        """Chat with metadata and optional native tool calls.

        ``idle_timeout`` caps seconds without ANY response byte (the call is
        otherwise never cut while it is producing). ``reasoning`` in the
        result carries the model's chain-of-thought from either the provider's
        reasoning field or inline <think> tags."""
        raw = self._post(self._build_body(
            prompt,
            stream=False,
            messages=messages,
            tools=tools,
            tool_choice=tool_choice,
            **overrides,
        ), timeout=idle_timeout)
        reply = self._parse_reply(raw)
        choices = raw.get("choices") or [{}]
        message = choices[0].get("message", {}) if isinstance(choices, list) else {}
        reasoning = str(message.get("reasoning_content")
                        or message.get("reasoning") or "")
        inline_clean, inline_reasoning = self._extract_think_tags(reply)
        if inline_reasoning:
            reasoning = (reasoning + "\n\n" + inline_reasoning).strip()
            reply = inline_clean
        return {
            "reply": reply,
            "reasoning": reasoning,
            "tool_calls": self._parse_tool_calls(raw),
            "model": self._parse_model_used(raw),
            "provider": self.name,
            "usage": raw.get("usage", {}),
            "raw": raw,
        }

    def stream_chat_with_info(
        self,
        prompt: str = "",
        messages: Optional[list[dict]] = None,
        tools: Optional[list[dict]] = None,
        tool_choice=None,
        on_token=None,
        on_reasoning=None,
        **overrides,
    ) -> dict:
        """SSE variant of chat_with_info with live callbacks.

        Content tokens fire ``on_token(token)``; reasoning tokens (delta
        .reasoning_content / .reasoning, plus any trailing <think> block) fire
        ``on_reasoning(token)``. Tool-call deltas are accumulated by index so
        fragmented argument JSON reassembles. Returns the chat_with_info
        shape; per-attempt silence beyond the socket timeout raises like any
        other failure so the pool can fall over immediately."""
        import urllib.error
        import urllib.request

        body = self._build_body(
            prompt, stream=True, messages=messages, tools=tools,
            tool_choice=tool_choice, **overrides,
        )
        req = urllib.request.Request(
            self._build_url(), data=json.dumps(body).encode(),
            headers=self._build_headers(), method="POST",
        )
        try:
            resp = urllib.request.urlopen(req, timeout=PROVIDER_REQUEST_TIMEOUT)
        except urllib.error.HTTPError as exc:
            raw_err = exc.read().decode()
            try:
                detail = json.loads(raw_err)
                msg = detail.get("error", {}).get("message", raw_err)
            except json.JSONDecodeError:
                msg = raw_err
            raise FreeProviderError(msg, provider=self.name, status=exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise FreeProviderError(f"{self.name} stream failed or timed out: {exc}",
                                    provider=self.name, status=0) from exc

        reply_parts: list[str] = []
        reasoning_parts: list[str] = []
        tool_acc: dict[int, dict] = {}
        model_used = self.default_model
        finish_reason = ""
        with resp:
            for raw_line in resp:
                line = raw_line.decode("utf-8", errors="replace").strip()
                if not line or line.startswith(":"):
                    continue
                if not line.startswith("data: "):
                    continue
                data_str = line[6:]
                if data_str == "[DONE]":
                    break
                try:
                    chunk = json.loads(data_str)
                except json.JSONDecodeError:
                    continue
                if self.cfg.get("response_wrapped") and isinstance(chunk, dict) \
                        and isinstance(chunk.get("data"), dict):
                    chunk = chunk["data"]
                if chunk.get("model"):
                    model_used = chunk["model"]
                choices = chunk.get("choices") or [{}]
                choice = choices[0] if isinstance(choices, list) else {}
                delta = choice.get("delta") or {}

                rsn = delta.get("reasoning_content") or delta.get("reasoning") or ""
                if rsn:
                    reasoning_parts.append(rsn)
                    if on_reasoning:
                        try:
                            on_reasoning(rsn)
                        except Exception:
                            pass
                tok = delta.get("content") or ""
                if tok:
                    reply_parts.append(tok)
                    if on_token:
                        try:
                            on_token(tok)
                        except Exception:
                            pass
                for frag in delta.get("tool_calls") or []:
                    if not isinstance(frag, dict):
                        continue
                    idx = frag.get("index", 0)
                    acc = tool_acc.setdefault(idx, {"id": "", "name": "", "args": ""})
                    if frag.get("id"):
                        acc["id"] = frag["id"]
                    fn = frag.get("function") or {}
                    fname = fn.get("name") or ""
                    if fname:
                        # Providers differ: OpenAI streams the name once and
                        # omits it afterwards; others repeat the full name in
                        # every fragment. Handle split AND repeated styles.
                        if fname.startswith(acc["name"]) and len(fname) > len(acc["name"]):
                            acc["name"] = fname
                        elif not acc["name"].startswith(fname):
                            acc["name"] += fname
                    if fn.get("arguments"):
                        acc["args"] += fn["arguments"]
                if choice.get("finish_reason"):
                    finish_reason = choice["finish_reason"]

        reply = "".join(reply_parts)
        clean, inline_r = self._extract_think_tags(reply)
        if inline_r:
            reasoning_parts.append(inline_r)
            reply = clean
        tool_calls = []
        for idx in sorted(tool_acc):
            acc = tool_acc[idx]
            name = acc["name"].strip()
            if not name:
                continue
            try:
                args = json.loads(acc["args"] or "{}")
            except json.JSONDecodeError:
                continue
            if not isinstance(args, dict):
                continue
            tool_calls.append({
                "id": acc["id"] or f"call_{idx + 1}",
                "name": name,
                "args": args,
            })
        return {
            "reply": reply,
            "reasoning": "\n".join(p for p in reasoning_parts if p),
            "tool_calls": tool_calls,
            "model": model_used,
            "provider": self.name,
            "usage": {},
            "finish_reason": finish_reason,
        }

    def chat_multimodal(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ) -> str:
        """Text + image(s) chat."""
        body = self._build_body(prompt, stream=False, images=images, image_urls=image_urls, **overrides)
        resp = self._post(body)
        return self._parse_reply(resp)

    def stream(self, prompt: str, **overrides):
        """Stream a text-only response token-by-token."""
        body = self._build_body(prompt, stream=True, **overrides)
        yield from self._stream_request(body)

    def _stream_request(self, body: dict):
        import urllib.error
        import urllib.request

        url = self._build_url()
        headers = self._build_headers()
        payload = json.dumps(body).encode()
        req = urllib.request.Request(url, data=payload, headers=headers, method="POST")

        try:
            resp = urllib.request.urlopen(req, timeout=PROVIDER_REQUEST_TIMEOUT)
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode()
            try:
                detail = json.loads(raw)
                msg = detail.get("error", {}).get("message", raw)
            except json.JSONDecodeError:
                msg = raw
            raise FreeProviderError(msg, provider=self.name, status=exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise FreeProviderError(f"{self.name} stream failed or timed out: {exc}", provider=self.name, status=0) from exc

        for raw_line in resp:
            line = raw_line.decode("utf-8", errors="replace").strip()
            if not line or line.startswith(":"):
                continue
            if line.startswith("data: "):
                data_str = line[6:]
            else:
                continue
            if data_str == "[DONE]":
                break
            try:
                chunk = json.loads(data_str)
            except json.JSONDecodeError:
                continue
            if self.cfg.get("response_wrapped") and isinstance(chunk, dict) and isinstance(chunk.get("data"), dict):
                chunk = chunk["data"]
            delta = chunk.get("choices", [{}])[0].get("delta", {})
            token = delta.get("content", "")
            if token:
                yield token

    def supports(self, capability: str) -> bool:
        """Check if this provider supports a capability."""
        key = f"supports_{capability}"
        return self.cfg.get(key, False)


# ----------------------------------------------------------------------
#  PHASE 5: FREE AGENT (multi-provider router)
# ----------------------------------------------------------------------

def _detect_capability(prompt: str, images=None, image_urls=None) -> str:
    """Auto-detect what capability is needed."""
    if images or image_urls:
        return "vision"
    prompt_lower = prompt.lower()
    # Audio? (checking for whisper-like patterns or audio file refs)
    if any(prompt_lower.endswith(ext) for ext in [".mp3", ".wav", ".ogg", ".m4a", ".flac"]):
        return "audio"
    # Coding keywords
    coding_keywords = [
        "write code", "function", "class ", "implement", "debug", "bug",
        "refactor", "algorithm", "syntax", "compile", "error",
    ]
    if any(kw in prompt_lower for kw in coding_keywords):
        return "coding"
    # Reasoning keywords
    reasoning_keywords = [
        "explain", "why", "how does", "reason", "analyze", "compare",
        "what is the difference", "pros and cons", "evaluate",
    ]
    if any(kw in prompt_lower for kw in reasoning_keywords):
        return "reasoning"
    return "text"


def _get_models_for_capability(
    capability: str,
    min_context_tokens: int | None = None,
    prefer_context: bool = False,
    prefer_reasoning: bool | None = None,
    max_context_tokens: int | None = None,
) -> list[dict]:
    """Get eligible models, optionally preferring the largest context first.

    ``prefer_reasoning`` nudges ranking toward (True) or away from (False)
    reasoning-capable models without overriding provider priority.
    ``max_context_tokens`` hard-caps the selectable window (e.g. ``<1M`` for
    compaction specialists that must avoid 1M-context models)."""
    # Â±30 stays below the provider-priority quantum (100): routing bias
    # reorders models, never providers.
    reasoning_bias = -30 if prefer_reasoning is True else 30 if prefer_reasoning is False else 0
    matches = []
    for model in MODEL_CATALOG:
        if capability in model.get("capabilities", []):
            context = int(model.get("context", 0) or 0)
            if min_context_tokens is not None and context < int(min_context_tokens):
                continue
            if max_context_tokens is not None and context >= int(max_context_tokens):
                continue
            provider = model["provider"]
            pcfg = PROVIDER_CONFIGS.get(provider, {})
            prov_priority = pcfg.get("priority", {}).get(capability, 99)

            # Combined score: provider priority + model priority (+ bias)
            score = prov_priority * 100 + model.get("priority", 99)
            if reasoning_bias and "reasoning" in model.get("capabilities", []):
                score += reasoning_bias
            matches.append((score, prov_priority, context, model))

    if prefer_context:
        matches.sort(key=lambda x: (-x[2], x[0], x[1], x[3].get("priority", 99)))
    else:
        matches.sort(key=lambda x: (x[0], x[1], x[3].get("priority", 99)))
    return [m[3] for m in matches]
class FreeProviders:
    """Multi-provider free LLM agent with smart routing and fallback.

    Routes requests to the best available provider based on capability,
    rate limit status, and priority.

    Usage:
        agent = FreeProviders()
        reply = agent.chat("Hello!")
        vision = agent.chat("Describe this", images=["photo.jpg"])
    """

    def __init__(self, enabled_providers: Optional[list[str]] = None):
        self.providers: dict[str, ProviderClient] = {}
        provider_names = enabled_providers or list(PROVIDER_CONFIGS.keys())
        for name in provider_names:
            try:
                self.providers[name] = ProviderClient(name)
            except (ValueError, ImportError) as e:
                pass  # Provider not configured, skip silently

        if not self.providers:
            raise ValueError(
                "No providers configured. Set at least one API key env var:\n"
                "  OPENROUTER_API_KEY, GROQ_API_KEY, GEMINI_API_KEY,\n"
                "  MISTRAL_API_KEY, NVIDIA_API_KEY, CLOUDFLARE_API_TOKEN\n"
                "  (+ CLOUDFLARE_ACCOUNT_ID for Cloudflare)\n"
                "  ZAI_API_KEY for Z.ai (GLM)\n"
                "  or OPENCODE_API_KEY for OpenCode\n"
                "  or CLINE_API_KEY for Cline (usage-billing)\n"
                "Note: Kilo Gateway works without any API key."
            )

        self._last_provider: Optional[str] = None

    def _route(
        self,
        capability: str,
        preferred_provider: Optional[str] = None,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        min_context_tokens: int | None = None,
        prefer_context: bool = False,
        prefer_reasoning: bool | None = None,
        max_context_tokens: int | None = None,
    ) -> list[tuple[ProviderClient, str]]:
        """Build an ordered list of (client, model_id) to try.

        Filters by capability, checks rate limits, sorts by priority.
        """
        # Auto-detect capability if images present
        actual_capability = _detect_capability("", images, image_urls)
        if actual_capability != "text":
            capability = actual_capability

        models = _get_models_for_capability(
            capability,
            min_context_tokens=min_context_tokens,
            prefer_context=prefer_context,
            prefer_reasoning=prefer_reasoning,
            max_context_tokens=max_context_tokens,
        )

        # Build route list
        route_list = []
        for model in models:
            client = self.providers.get(model["provider"])
            if client is None or getattr(client, "auth_dead", False):
                continue
            if not client.rate_limiter.can_request():
                continue
            route_list.append((client, model["id"]))

        # Deduplicate: keep first occurrence of each provider
        seen_providers = set()
        deduped = []
        for client, model_id in route_list:
            if client.name not in seen_providers:
                seen_providers.add(client.name)
                deduped.append((client, model_id))

        # If preferred provider is set, move it to front
        if preferred_provider:
            preferred = [(c, m) for c, m in deduped if c.name == preferred_provider]
            rest = [(c, m) for c, m in deduped if c.name != preferred_provider]
            deduped = preferred + rest

        return deduped

    def available_model_contexts(
        self, capability: str = "text", max_context_tokens: int | None = None
    ) -> list[int]:
        """Return context capacities for models backed by configured providers."""
        configured = set(self.providers)
        capacities = {
            int(model.get("context", 0) or 0)
            for model in MODEL_CATALOG
            if model.get("provider") in configured
            and capability in model.get("capabilities", [])
            and int(model.get("context", 0) or 0) > 0
            and (max_context_tokens is None or int(model.get("context", 0) or 0) < int(max_context_tokens))
        }
        return sorted(capacities, reverse=True)

    def _get_shortest_cooldown(self) -> float:
        """Find the shortest cooldown time among all rate-limited providers."""
        now = time.time()
        shortest = 60.0  # max wait
        for client in self.providers.values():
            remaining = client.rate_limiter._cooldown_until - now
            if 0 < remaining < shortest:
                shortest = remaining
        return max(shortest, 2.0)  # at least 2s â€” fast fallback beats patience

    def chat(
        self,
        task: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        capability: str = "text",
        preferred_provider: Optional[str] = None,
        **overrides,
    ) -> str:
        """Send a request to the best available provider.

        Falls back through providers on failure. Normal requests retain the
        bounded retry policy. Dedicated compaction requests can opt into
        ``retry_forever`` and context-aware routing without changing normal
        Developer/FreeProviders behavior.
        """
        min_context_tokens = overrides.pop("min_context_tokens", None)
        prefer_context = bool(overrides.pop("prefer_context", False))
        prefer_reasoning = overrides.pop("prefer_reasoning", None)
        max_context_tokens = overrides.pop("max_context_tokens", None)
        retry_forever = bool(overrides.pop("retry_forever", False))
        context_sensitive = bool(overrides.pop("context_sensitive", False))
        abort_check = overrides.pop("abort_check", None)
        native_messages = overrides.pop("messages", None)
        native_tools = overrides.pop("tools", None)
        native_tool_choice = overrides.pop("tool_choice", None)
        return_info = bool(overrides.pop("_return_info", False))
        max_loops = MAX_PROVIDER_RETRY_CYCLES
        loop = 0
        last_error: Optional[Exception] = None

        # A context-aware request must fail over to chunking when no
        # configured model can fit the requested payload. Without this guard,
        # ``retry_forever`` would wait forever for a model that cannot exist.
        if min_context_tokens is not None:
            actual_capability = _detect_capability("", images, image_urls)
            if actual_capability == "text":
                actual_capability = capability
            fitting_models = [
                model for model in MODEL_CATALOG
                if model.get("provider") in self.providers
                and (max_context_tokens is None or int(model.get("context", 0) or 0) < int(max_context_tokens))
                and actual_capability in model.get("capabilities", [])
                and int(model.get("context", 0) or 0) >= int(min_context_tokens)
            ]
            if not fitting_models:
                raise ContextWindowExceededError(
                    f"No configured {actual_capability} model fits {int(min_context_tokens):,} tokens."
                )

        while retry_forever or loop < max_loops:
            if callable(abort_check) and abort_check():
                raise AllProvidersExhaustedError("Free-agent request cancelled.")

            routes = self._route(
                capability,
                preferred_provider,
                images,
                image_urls,
                min_context_tokens=min_context_tokens,
                prefer_context=prefer_context,
                prefer_reasoning=prefer_reasoning,
                max_context_tokens=max_context_tokens,
            )

            # All providers rate-limited â€” wait and loop back
            if not routes:
                if not retry_forever and loop >= max_loops - 1:
                    raise AllProvidersExhaustedError(
                        f"No eligible providers after {max_loops} retry cycles."
                    ) from last_error
                wait = self._get_shortest_cooldown() if not min_context_tokens else min(60.0, 5.0 * (2 ** min(loop, 4)))
                _sleep_with_abort(wait, abort_check)
                loop += 1
                continue

            context_error: Optional[Exception] = None
            for route_pos, (client, model_id) in enumerate(routes):
                try:
                    overrides["model"] = model_id
                    has_media = (images and len(images) > 0) or (image_urls and len(image_urls) > 0)

                    if return_info or native_messages is not None or native_tools is not None:
                        result = client.chat_with_info(
                            task,
                            messages=native_messages,
                            tools=native_tools,
                            tool_choice=native_tool_choice,
                            **overrides,
                        )
                    elif has_media:
                        result = client.chat_multimodal(
                            task, images=images, image_urls=image_urls, **overrides
                        )
                    else:
                        result = client.chat(task, **overrides)

                    client.rate_limiter.record_request()
                    client.fail_streak = 0
                    if return_info:
                        result_text = str(result.get("reply", "") if isinstance(result, dict) else result or "")
                        has_tool_calls = bool(result.get("tool_calls")) if isinstance(result, dict) else False
                    else:
                        result_text = str(result or "")
                        has_tool_calls = False
                    if retry_forever and not result_text.strip() and not has_tool_calls:
                        last_error = FreeProviderError(
                            "provider returned an empty response",
                            provider=client.name,
                        )
                        continue
                    if (
                        not retry_forever
                        and (native_messages is not None or native_tools is not None)
                        and not result_text.strip()
                        and not has_tool_calls
                        and route_pos < len(routes) - 1
                    ):
                        # An empty turn with tools still available is a soft
                        # failure: fall through to the next provider instead
                        # of ending the call with nothing.
                        last_error = FreeProviderError(
                            "provider returned an empty response",
                            provider=client.name,
                        )
                        continue
                    self._last_provider = client.name
                    return result

                except FreeProviderError as e:
                    last_error = e
                    if context_sensitive and _is_context_window_error(e):
                        context_error = e
                        continue
                    if e.status in (401, 403):
                        # Auth error â€” this key will not heal; disable the
                        # provider for the lifetime of the pool.
                        client.auth_dead = True
                        continue
                    elif e.status == 429:
                        # Rate limited â€” cooldown and try next
                        client.rate_limiter.trigger_cooldown(30)
                        continue
                    elif e.status == 0 or e.status >= 500:
                        # Transport/server failure â€” escalate cooldown so
                        # repeated dead providers stop costing us seconds.
                        client.fail_streak += 1
                        client.rate_limiter.trigger_cooldown(
                            min(90.0, 30.0 * client.fail_streak)
                        )
                        continue
                    else:
                        # Other error â€” try next
                        continue
                except Exception as e:
                    last_error = e
                    if context_sensitive and _is_context_window_error(e):
                        context_error = e
                        continue
                    continue

            if context_sensitive and context_error is not None:
                # Do not retry an impossible full request forever. The
                # compactor catches this and switches to chronological chunks.
                raise ContextWindowExceededError(
                    str(context_error),
                    provider=getattr(context_error, "provider", ""),
                    status=getattr(context_error, "status", 0),
                ) from context_error

            # All providers in this round failed â€” wait, then loop back to start
            if retry_forever or loop < max_loops - 1:
                wait = self._get_shortest_cooldown()
                _sleep_with_abort(wait, abort_check)
                loop += 1
                continue

        raise AllProvidersExhaustedError(
            f"All providers failed after {max_loops} retry cycles. Last error: {last_error}"
        ) from last_error

    def chat_with_info(
        self,
        messages: list[dict],
        capability: str = "text",
        preferred_provider: Optional[str] = None,
        **overrides,
    ) -> dict:
        """Send an exact native-message conversation and return tool metadata."""
        return self.chat(
            "",
            capability=capability,
            preferred_provider=preferred_provider,
            messages=messages,
            _return_info=True,
            **overrides,
        )

    def stream_complete(
        self,
        messages: list[dict],
        task: str = "",
        capability: str = "coding",
        preferred_provider: Optional[str] = None,
        on_token=None,
        on_reasoning=None,
        temperature: Optional[float] = None,
        abort_check=None,
        **overrides,
    ) -> dict:
        """Routed streaming turn â€” same fallback ladder as chat(), but the
        winning attempt streams via SSE with live callbacks.

        Returns the chat_with_info shape. An empty streamed turn falls through
        to the next provider; a mid-stream transport failure raises like any
        other failure so the caller can decide on a non-streaming retry.
        """
        min_context_tokens = overrides.pop("min_context_tokens", None)
        prefer_context = bool(overrides.pop("prefer_context", False))
        prefer_reasoning = overrides.pop("prefer_reasoning", None)
        max_context_tokens = overrides.pop("max_context_tokens", None)
        retry_forever = bool(overrides.pop("retry_forever", False))
        context_sensitive = bool(overrides.pop("context_sensitive", False))
        native_tools = overrides.pop("tools", None)
        native_tool_choice = overrides.pop("tool_choice", None)
        if temperature is not None:
            overrides["temperature"] = temperature

        last_error: Optional[Exception] = None
        max_loops = MAX_PROVIDER_RETRY_CYCLES
        loop = 0
        while retry_forever or loop < max_loops:
            if callable(abort_check) and abort_check():
                raise AllProvidersExhaustedError("Free-provider request cancelled.")
            routes = self._route(
                capability, preferred_provider,
                min_context_tokens=min_context_tokens,
                prefer_context=prefer_context,
                prefer_reasoning=prefer_reasoning,
                max_context_tokens=max_context_tokens,
            )
            if not routes:
                if not retry_forever and loop >= max_loops - 1:
                    raise AllProvidersExhaustedError(
                        f"No eligible providers after {max_loops} retry cycles."
                    ) from last_error
                time.sleep(self._get_shortest_cooldown())
                loop += 1
                continue

            for route_pos, (client, model_id) in enumerate(routes):
                try:
                    overrides["model"] = model_id
                    result = client.stream_chat_with_info(
                        task,
                        messages=messages,
                        tools=native_tools,
                        tool_choice=native_tool_choice,
                        on_token=on_token,
                        on_reasoning=on_reasoning,
                        **overrides,
                    )
                    client.rate_limiter.record_request()
                    client.fail_streak = 0
                    if (
                        not retry_forever
                        and not str(result.get("reply", "")).strip()
                        and not result.get("tool_calls")
                        and route_pos < len(routes) - 1
                    ):
                        last_error = FreeProviderError(
                            "provider returned an empty streamed response",
                            provider=client.name,
                        )
                        continue
                    self._last_provider = client.name
                    return result
                except FreeProviderError as e:
                    last_error = e
                    if getattr(client, "auth_dead", False):
                        continue
                    if e.status in (401, 403):
                        client.auth_dead = True
                        continue
                    if e.status == 429:
                        client.rate_limiter.trigger_cooldown(30)
                        continue
                    if e.status == 0 or e.status >= 500:
                        client.fail_streak += 1
                        client.rate_limiter.trigger_cooldown(
                            min(90.0, 30.0 * client.fail_streak)
                        )
                        continue
                    continue
                except Exception as e:
                    last_error = e
                    continue

            if retry_forever or loop < max_loops - 1:
                _sleep_with_abort(self._get_shortest_cooldown(), abort_check)
                loop += 1
                continue

        raise AllProvidersExhaustedError(
            f"All providers failed streaming after {max_loops} retry cycles. "
            f"Last error: {last_error}"
        ) from last_error

    def stream(
        self,
        task: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        capability: str = "text",
        preferred_provider: Optional[str] = None,
        **overrides,
    ):
        """Stream a response from the best available provider.

        Tries the configured provider routes within the bounded retry policy.
        """
        max_loops = MAX_PROVIDER_RETRY_CYCLES
        for loop in range(max_loops):
            routes = self._route(capability, preferred_provider, images, image_urls)

            if not routes:
                if loop >= max_loops - 1:
                    raise AllProvidersExhaustedError(
                        f"No available providers for streaming after {max_loops} retry cycles."
                    )
                time.sleep(self._get_shortest_cooldown())
                continue

            last_error: Optional[Exception] = None
            for client, model_id in routes:
                try:
                    overrides["model"] = model_id
                    for token in client.stream(task, **overrides):
                        yield token
                    client.rate_limiter.record_request()
                    self._last_provider = client.name
                    return
                except FreeProviderError as e:
                    last_error = e
                    if e.status in (401, 429):
                        continue
                    continue
                except Exception as e:
                    last_error = e
                    continue

            if loop < max_loops - 1:
                time.sleep(self._get_shortest_cooldown())
                continue

        raise AllProvidersExhaustedError(
            f"All providers failed streaming after {max_loops} retries. Last error: {last_error}"
        ) from last_error

    def list_configured_providers(self) -> list[dict]:
        """List which providers are configured and their status."""
        result = []
        for name, client in self.providers.items():
            cfg = PROVIDER_CONFIGS[name]
            can_req = client.rate_limiter.can_request()
            result.append({
                "name": cfg["name"],
                "key": name,
                "models": len([m for m in MODEL_CATALOG if m["provider"] == name]),
                "default_model": client.default_model,
                "rate_limited": not can_req,
                "supports_vision": cfg.get("supports_vision", False),
                "supports_audio": cfg.get("supports_audio", False),
            })
        return result


# ----------------------------------------------------------------------
#  MODULE API (singleton pool + convenience wrappers)
# ----------------------------------------------------------------------

_default_pool = None


def get_pool() -> FreeProviders:
    global _default_pool
    if _default_pool is None:
        _default_pool = FreeProviders()
    return _default_pool


def configure(enabled_providers=None) -> None:
    """Rebuild the shared pool with an explicit provider allow-list."""
    global _default_pool
    _default_pool = FreeProviders(enabled_providers=enabled_providers)


def complete(messages=None, prompt="", tools=None, tool_choice=None,
             capability="coding", preferred_provider=None, temperature=None,
             abort_check=None, **overrides) -> dict:
    """One routed completion across free providers.

    Returns the chat_with_info shape: {"reply", "tool_calls", "model",
    "provider", "usage", ...}. Raises AllProvidersExhaustedError or
    FreeProviderError subclasses on total failure.
    """
    if temperature is not None:
        overrides["temperature"] = temperature
    return get_pool().chat(
        task=prompt or "",
        capability=capability,
        preferred_provider=preferred_provider,
        messages=messages,
        tools=tools,
        tool_choice=tool_choice,
        abort_check=abort_check,
        _return_info=True,
        **overrides,
    )


def chat(prompt, **overrides) -> str:
    """One-string convenience: routed reply text."""
    return get_pool().chat(prompt, **overrides)


def stream_complete(messages=None, prompt="", tools=None, tool_choice=None,
                    capability="coding", preferred_provider=None,
                    temperature=None, on_token=None, on_reasoning=None,
                    abort_check=None, **overrides) -> dict:
    """Routed streaming completion (see FreeProviders.stream_complete).

    Fires ``on_token``/``on_reasoning`` live while the winning provider
    generates; returns the full info dict when done."""
    if temperature is not None:
        overrides["temperature"] = temperature
    return get_pool().stream_complete(
        messages=messages or [],
        capability=capability,
        preferred_provider=preferred_provider,
        tools=tools,
        tool_choice=tool_choice,
        on_token=on_token,
        on_reasoning=on_reasoning,
        abort_check=abort_check,
        task=prompt or "",
        **overrides,
    )


def to_native_messages(messages: list) -> list[dict]:
    """LangChain transcript -> OpenAI-native dicts (free transport).
    Image content-block lists pass through untouched."""
    from llm.native_conv import lc_to_native

    return lc_to_native(messages)


def list_remote_models(provider: str, timeout: float = 15.0) -> list[dict]:
    """GET {base_url}/models for a configured provider.

    Most catalogs are public; if a key exists in the environment it is sent.
    Returns [{"id", "context"}]. Freshness endpoint per provider - run this
    before pinning new model ids into MODEL_CATALOG."""
    cfg = PROVIDER_CONFIGS.get(provider)
    if not cfg:
        raise ValueError(f"Unknown provider '{provider}'. Options: {sorted(PROVIDER_CONFIGS)}")
    import urllib.request

    url = cfg["base_url"].rstrip("/") + "/models"
    headers = {"User-Agent": "free_providers"}
    key = os.getenv(cfg["env_key"], "") or os.getenv(cfg["env_key_fallback"], "")
    if key:
        headers["Authorization"] = "Bearer " + key.replace(";", ",").split(",")[0].strip()
    req = urllib.request.Request(url, headers=headers)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read().decode())
    items = data.get("data") if isinstance(data, dict) else data
    out = []
    for m in items if isinstance(items, list) else []:
        if isinstance(m, dict) and m.get("id"):
            try:
                ctx = int(m.get("context_length") or 0)
            except Exception:
                ctx = 0
            out.append({"id": m["id"], "context": ctx})
    return out


def list_models(capability: str = "", provider: str = "") -> list[dict]:
    """Static catalog entries, optionally filtered."""
    models = MODEL_CATALOG
    if provider:
        models = [m for m in models if m.get("provider") == provider]
    if capability:
        models = [m for m in models if capability in m.get("capabilities", [])]
    return models


def refresh_catalog(timeout: float = 15.0) -> list[dict]:
    """Live-refresh OpenRouter's catalog; returns currently-free model ids.

    Freshness endpoint: run this whenever you want to see what the free tier
    offers today, then pin ids via preferred_provider/model overrides.
    """
    import urllib.request

    req = urllib.request.Request(
        "https://openrouter.ai/api/v1/models",
        headers={"User-Agent": "free_providers"},
    )
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        data = json.loads(resp.read().decode())
    out: list[dict] = []
    for m in data.get("data", []):
        mid = str(m.get("id") or "")
        pricing = m.get("pricing") or {}
        try:
            free = float(pricing.get("prompt", "1") or 1) == 0 and float(
                pricing.get("completion", "1") or 1
            ) == 0
        except Exception:
            free = ":free" in mid
        if mid and free:
            out.append({
                "id": mid,
                "name": m.get("name", ""),
                "context": int(m.get("context_length") or 0),
            })
    return out


def _free_turn_emitters(name: str, chat_id: str):
    """Live stream-bus emitters for an agent turn (content + reasoning)."""
    if not chat_id:
        try:
            from sync.state_sync import safe_get_state

            chat_id = str(safe_get_state().get("chat_id") or "")
        except Exception:
            chat_id = ""
    if not chat_id:
        return None, None
    try:
        from server.realtime.bus import emit as _bus_emit

        def on_token(t, _c=chat_id, _n=name, _e=_bus_emit):
            try:
                _e(_c, "text", t, agent=_n)
            except Exception:
                pass

        def on_reasoning(t, _c=chat_id, _n=name, _e=_bus_emit):
            try:
                _e(_c, "thinking", t, agent=_n)
            except Exception:
                pass

        return on_token, on_reasoning
    except Exception:
        return None, None


def _model_messages_to_native(history: list) -> list[dict]:
    """ModelMessage -> OpenAI-native dicts for free pool (A6b)."""
    out: list[dict] = []
    try:
        from pydantic_ai.messages import (
            ModelRequest,
            ModelResponse,
            SystemPromptPart,
            ToolCallPart,
            ToolReturnPart,
            UserPromptPart,
            TextPart,
            ThinkingPart,
            BinaryContent,
            ImageUrl,
        )
    except Exception:
        return out
    for m in history or []:
        try:
            if isinstance(m, ModelRequest):
                for p in getattr(m, "parts", []) or []:
                    if isinstance(p, SystemPromptPart):
                        out.append({"role": "system", "content": p.content})
                    elif isinstance(p, UserPromptPart):
                        c = p.content
                        if isinstance(c, str):
                            out.append({"role": "user", "content": c})
                        elif isinstance(c, list):
                            text_parts: list[str] = []
                            image_parts: list[dict] = []
                            for item in c:
                                if isinstance(item, str):
                                    text_parts.append(item)
                                elif isinstance(item, BinaryContent):
                                    try:
                                        import base64
                                        b64 = base64.b64encode(item.data).decode("ascii")
                                        mime = getattr(item, "media_type", "image/png") or "image/png"
                                        url = f"data:{mime};base64,{b64}"
                                        image_parts.append({"type": "image_url", "image_url": {"url": url}})
                                    except Exception:
                                        text_parts.append(str(item))
                                elif isinstance(item, ImageUrl):
                                    try:
                                        image_parts.append({"type": "image_url", "image_url": {"url": item.url}})
                                    except Exception:
                                        text_parts.append(str(item))
                                else:
                                    text_parts.append(str(item))
                            if text_parts and image_parts:
                                out.append({"role": "user", "content": [{"type": "text", "text": "\n".join(text_parts)}] + image_parts})
                            elif image_parts:
                                out.append({"role": "user", "content": image_parts})
                            elif text_parts:
                                out.append({"role": "user", "content": "\n".join(text_parts)})
                            else:
                                out.append({"role": "user", "content": ""})
                        else:
                            out.append({"role": "user", "content": str(c)})
                    elif isinstance(p, ToolReturnPart):
                        out.append({"role": "tool", "tool_call_id": p.tool_call_id or "", "name": p.tool_name or "", "content": str(p.content or "")})
            elif isinstance(m, ModelResponse):
                text = ""
                tool_calls: list[dict] = []
                for p in getattr(m, "parts", []) or []:
                    if isinstance(p, TextPart):
                        text += p.content or ""
                    elif isinstance(p, ToolCallPart):
                        args = p.args if isinstance(p.args, dict) else {}
                        if isinstance(args, str):
                            try:
                                args = json.loads(args)
                            except Exception:
                                args = {}
                        tool_calls.append({"id": p.tool_call_id or "", "type": "function", "function": {"name": p.tool_name or "", "arguments": json.dumps(args, separators=(",", ":"))}})
                    elif isinstance(p, ThinkingPart):
                        pass
                entry: dict = {"role": "assistant", "content": text or ""}
                if tool_calls:
                    entry["tool_calls"] = tool_calls
                out.append(entry)
        except Exception:
            continue
    return out


def free_native_turn(
    name: str,
    messages: list,
    tools: Optional[list[dict]] = None,
    capability: str = "coding",
    temperature: Optional[float] = None,
    prefer_reasoning: Optional[bool] = None,
    max_context_tokens: int | None = None,
) -> tuple[str, list[dict], Optional[str]]:
    """One workspace-agent LLM turn on the free pool.

    Streams live to the shared chat bus (content + thinking), falls back to a
    single bounded non-streaming attempt on mid-stream failure, and returns the
    canonical ``(text, tool_calls, reasoning)`` triple every agent loop
    consumes. Raises loudly when the whole pool fails â€” no silent paid
    fallback by design.
    """
    pool = FreeProviders()
    # A6b: native ModelMessage support
    if messages and isinstance(messages[0], dict):
        native_msgs = messages
    else:
        try:
            from pydantic_ai.messages import ModelRequest, ModelResponse

            if messages and isinstance(messages[0], (ModelRequest, ModelResponse)):
                native_msgs = _model_messages_to_native(messages)
            else:
                native_msgs = to_native_messages(messages or [])
        except Exception:
            native_msgs = to_native_messages(messages or [])
    on_token, on_reasoning = _free_turn_emitters(name, "")
    extra = {}
    if max_context_tokens is not None:
        extra["max_context_tokens"] = max_context_tokens
    try:
        info = pool.stream_complete(
            messages=native_msgs,
            tools=tools or None,
            capability=capability,
            temperature=temperature,
            prefer_reasoning=prefer_reasoning,
            on_token=on_token,
            on_reasoning=on_reasoning,
            **extra,
        )
    except Exception:
        info = pool.chat_with_info(
            messages=native_msgs,
            tools=tools or None,
            capability=capability,
            temperature=temperature,
            prefer_reasoning=prefer_reasoning,
            **extra,
        )
    reply = str(info.get("reply", "") or "")
    calls: list[dict] = []
    for index, tc in enumerate(info.get("tool_calls") or []):
        if not isinstance(tc, dict):
            continue
        tname = str(tc.get("name") or tc.get("tool") or "").strip()
        args = tc.get("args") or {}
        if isinstance(args, str):
            try:
                args = json.loads(args or "{}")
            except Exception:
                args = {}
        if tname and isinstance(args, dict):
            calls.append({
                "tool": tname,
                "args": args,
                "id": str(tc.get("id") or f"{name}-free-{index + 1}"),
            })
    return reply, calls, (str(info.get("reasoning", "") or "").strip() or None)

