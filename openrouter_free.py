"""
openrouter_free.py --- one-file LLM client for OpenRouter's free models.

Zero-cost AI inference via the openrouter/free router (or specific free models).

Quick start:
    export OPENROUTER_API_KEY="sk-or-v1-..."
    python openrouter_free.py chat "What is the capital of France?"

As a module:
    import openrouter_free as llm
    reply = llm.chat("Why is the sky blue?")
    print(reply)

Dependencies:
    pip install httpx   (or use asyncio + httpx natively)
"""

import base64
import json
import os
import sys
from dataclasses import dataclass, field
from typing import Optional, Union

# ----------------------------------------------------------------------
#  Configuration
# ----------------------------------------------------------------------

OPENROUTER_API_BASE = "https://openrouter.ai/api/v1"
MODEL_FREE_ROUTER = "openrouter/free"
MODEL_AUTO_ROUTER = "openrouter/auto"

# Default model --- "openrouter/free" auto-picks from free pool.
# Override via env var OPENROUTER_MODEL or pass model= to functions.
DEFAULT_MODEL = os.getenv("OPENROUTER_MODEL", MODEL_FREE_ROUTER)

# Optional display name / referer for OpenRouter leaderboard attribution.
APP_NAME = os.getenv("OPENROUTER_APP_NAME", "openrouter_free.py")
APP_URL = os.getenv("OPENROUTER_APP_URL", "")


# ----------------------------------------------------------------------
#  Exceptions
# ----------------------------------------------------------------------

class OpenRouterError(Exception):
    """Raised on API errors (auth failure, rate limit, bad request, etc.)."""
    def __init__(self, status: int, body: str):
        self.status = status
        self.body = body
        super().__init__(f"[{status}] {body[:300]}")


# ----------------------------------------------------------------------
#  Core client
# ----------------------------------------------------------------------

@dataclass
class FreeLLM:
    """
    Lightweight OpenRouter client targeting free models.

    Usage:
        llm = FreeLLM()
        reply = llm.chat("Hello!")
        print(reply)

        for chunk in llm.chat_stream("Tell me a story"):
            print(chunk, end="", flush=True)
    """

    api_key: Optional[str] = None
    model: str = DEFAULT_MODEL
    base_url: str = OPENROUTER_API_BASE
    app_name: str = APP_NAME
    app_url: str = APP_URL
    system_prompt: Optional[str] = None
    default_params: dict = field(default_factory=lambda: {
        "temperature": 0.7,
        "max_tokens": 4096,
        "top_p": 0.9,
    })

    def __post_init__(self):
        self.api_key = self.api_key or os.getenv("OPENROUTER_API_KEY")
        if not self.api_key:
            raise ValueError(
                "OPENROUTER_API_KEY not set --- pass api_key= or set the env var."
                "\nGet yours at: https://openrouter.ai/keys"
            )
        self._headers = {
            "Authorization": f"Bearer {self.api_key}",
            "Content-Type": "application/json",
        }
        if self.app_name:
            self._headers["X-Title"] = self.app_name
        if self.app_url:
            self._headers["HTTP-Referer"] = self.app_url

    # -- helpers ------------------------------------------------------

    @staticmethod
    def _image_to_data_uri(image_path: str) -> str:
        """Convert a local image file to a base64 data URI.

        Supports: jpg, jpeg, png, gif, webp, bmp.
        """
        ext = os.path.splitext(image_path)[1].lower().lstrip(".")
        mime_map = {
            "jpg": "image/jpeg",
            "jpeg": "image/jpeg",
            "png": "image/png",
            "gif": "image/gif",
            "webp": "image/webp",
            "bmp": "image/bmp",
        }
        mime = mime_map.get(ext, "image/png")
        with open(image_path, "rb") as f:
            data = base64.b64encode(f.read()).decode("utf-8")
        return f"data:{mime};base64,{data}"

    def _build_messages(self, prompt: str) -> list[dict]:
        msgs = []
        if self.system_prompt:
            msgs.append({"role": "system", "content": self.system_prompt})
        msgs.append({"role": "user", "content": prompt})
        return msgs

    def _build_multimodal_messages(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
    ) -> list[dict]:
        """Build messages with text + image content parts.

        Args:
            prompt: Text prompt.
            images: Local file paths to encode as base64 data URIs.
            image_urls: Public URLs to images (sent directly).
        """
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
        msgs = []
        if self.system_prompt:
            msgs.append({"role": "system", "content": self.system_prompt})
        msgs.append({"role": "user", "content": content_parts})
        return msgs

    def _build_body(
        self,
        prompt: str,
        stream: bool = False,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ) -> dict:
        params = {**self.default_params, **overrides}
        params.pop("api_key", None)
        has_images = (images and len(images) > 0) or (image_urls and len(image_urls) > 0)
        body: dict = {
            "model": params.pop("model", self.model),
            "messages": self._build_multimodal_messages(prompt, images, image_urls) if has_images
                       else self._build_messages(prompt),
            "stream": stream,
            **params,
        }
        for key in ("system_prompt", "app_name", "app_url", "base_url"):
            body.pop(key, None)
        return body

    def _parse_reply(self, body: dict) -> str:
        choice = body.get("choices", [{}])[0]
        msg = choice.get("message", {})
        content = msg.get("content", "")
        return content.strip() if content else ""

    def _model_used(self, body: dict) -> str:
        """Return the actual model that served the request (useful with routers)."""
        return body.get("model", self.model)

    # -- text chat ----------------------------------------------------

    def chat(self, prompt: str, **overrides) -> str:
        """Send a text prompt and return the reply."""
        body = self._request_base(prompt, stream=False, **overrides)
        return self._parse_reply(body)

    def chat_with_info(self, prompt: str, **overrides) -> dict:
        """Send a text prompt and return {reply, model, usage, raw}."""
        raw = self._request_base(prompt, stream=False, **overrides)
        return {
            "reply": self._parse_reply(raw),
            "model": self._model_used(raw),
            "usage": raw.get("usage", {}),
            "raw": raw,
        }

    # -- multimodal (vision) chat -------------------------------------

    def chat_multimodal(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ) -> str:
        """Send a text + image(s) prompt and return the reply.

        Args:
            prompt: Text prompt / question about the image.
            images: Local file paths (will be base64-encoded).
            image_urls: Public image URLs.

        Example:
            llm.chat_multimodal("What's in this photo?", images=["photo.jpg"])
            llm.chat_multimodal("Read this sign", image_urls=["https://..."])
        """
        body = self._request_multimodal(prompt, images, image_urls, **overrides)
        return self._parse_reply(body)

    def chat_multimodal_with_info(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ) -> dict:
        """Send a text + image(s) prompt and return {reply, model, usage, raw}."""
        raw = self._request_multimodal(prompt, images, image_urls, **overrides)
        return {
            "reply": self._parse_reply(raw),
            "model": self._model_used(raw),
            "usage": raw.get("usage", {}),
            "raw": raw,
        }

    # -- streaming ----------------------------------------------------

    def chat_stream(self, prompt: str, **overrides):
        """Stream a text reply token-by-token."""
        body = self._build_body(prompt, stream=True, **overrides)
        yield from self._stream_request(body)

    def chat_multimodal_stream(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ):
        """Stream a multimodal (text + image) reply token-by-token."""
        body = self._build_body(
            prompt, stream=True, images=images, image_urls=image_urls, **overrides
        )
        yield from self._stream_request(body)

    # -- low-level request --------------------------------------------

    def _request_base(self, prompt: str, stream: bool = False, **overrides) -> dict:
        body = self._build_body(prompt, stream=stream, **overrides)
        return self._post(body)

    def _request_multimodal(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ) -> dict:
        body = self._build_body(prompt, images=images, image_urls=image_urls, **overrides)
        return self._post(body)

    def _post(self, body: dict) -> dict:
        import urllib.error
        import urllib.request

        payload = json.dumps(body).encode()
        req = urllib.request.Request(
            f"{self.base_url}/chat/completions",
            data=payload,
            headers=self._headers,
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=120) as resp:
                return json.loads(resp.read().decode())
        except urllib.error.HTTPError as exc:
            raw = exc.read().decode()
            try:
                detail = json.loads(raw)
                msg = detail.get("error", {}).get("message", raw)
            except json.JSONDecodeError:
                msg = raw
            raise OpenRouterError(exc.code, msg) from exc

    def _stream_request(self, body: dict):
        import urllib.error
        import urllib.request

        payload = json.dumps(body).encode()
        req = urllib.request.Request(
            f"{self.base_url}/chat/completions",
            data=payload,
            headers=self._headers,
            method="POST",
        )
        try:
            resp = urllib.request.urlopen(req)
        except urllib.error.HTTPError as exc:
            raise OpenRouterError(exc.code, exc.read().decode()) from exc

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
            delta = chunk.get("choices", [{}])[0].get("delta", {})
            token = delta.get("content", "")
            if token:
                yield token

    # -- async streaming ----------------------------------------------

    async def chat_stream_async(self, prompt: str, **overrides):
        """Async generator -- same as chat_stream but requires httpx."""
        try:
            import httpx
        except ImportError:
            raise ImportError("chat_stream_async requires httpx: pip install httpx")

        body = self._build_body(prompt, stream=True, **overrides)
        async with httpx.AsyncClient(timeout=120) as client:
            async with client.stream(
                "POST",
                f"{self.base_url}/chat/completions",
                json=body,
                headers=self._headers,
            ) as resp:
                if resp.status_code != 200:
                    raise OpenRouterError(resp.status_code, await resp.aread())
                async for line in resp.aiter_lines():
                    line = line.strip()
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
                    delta = chunk.get("choices", [{}])[0].get("delta", {})
                    token = delta.get("content", "")
                    if token:
                        yield token

    async def chat_multimodal_stream_async(
        self,
        prompt: str,
        images: Optional[list[str]] = None,
        image_urls: Optional[list[str]] = None,
        **overrides,
    ):
        """Async generator for multimodal (text + image) streaming."""
        try:
            import httpx
        except ImportError:
            raise ImportError("chat_multimodal_stream_async requires httpx: pip install httpx")

        body = self._build_body(
            prompt, stream=True, images=images, image_urls=image_urls, **overrides
        )
        async with httpx.AsyncClient(timeout=120) as client:
            async with client.stream(
                "POST",
                f"{self.base_url}/chat/completions",
                json=body,
                headers=self._headers,
            ) as resp:
                if resp.status_code != 200:
                    raise OpenRouterError(resp.status_code, await resp.aread())
                async for line in resp.aiter_lines():
                    line = line.strip()
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
                    delta = chunk.get("choices", [{}])[0].get("delta", {})
                    token = delta.get("content", "")
                    if token:
                        yield token


# ----------------------------------------------------------------------
#  Module-level convenience functions
# ----------------------------------------------------------------------
#  These use the DEFAULT_MODEL and OPENROUTER_API_KEY from the env.
#  For advanced use, instantiate FreeLLM() directly.

_default_llm: Optional[FreeLLM] = None


def _get_default() -> FreeLLM:
    global _default_llm
    if _default_llm is None:
        _default_llm = FreeLLM()
    return _default_llm


def chat(prompt: str, **kwargs) -> str:
    """Send a prompt and return the reply (uses global default client)."""
    return _get_default().chat(prompt, **kwargs)


def chat_with_info(prompt: str, **kwargs) -> dict:
    """Send a prompt and get back {reply, model, usage, raw}."""
    return _get_default().chat_with_info(prompt, **kwargs)


def chat_stream(prompt: str, **kwargs):
    """Stream a reply token-by-token."""
    yield from _get_default().chat_stream(prompt, **kwargs)


def configure(api_key: str = None, model: str = None, **kwargs):
    """
    Reconfigure the default client.

    Call this once at the top of your script to set your key / model:
        import openrouter_free as llm
        llm.configure(api_key="sk-...", model="openrouter/free")
    """
    global _default_llm
    current = _default_llm
    _default_llm = FreeLLM(
        api_key=api_key or (current.api_key if current else None),
        model=model or (current.model if current else MODEL_FREE_ROUTER),
        **kwargs,
    )
    if current:
        for k, v in current.__dict__.items():
            if k not in ("api_key", "model"):
                setattr(_default_llm, k, v)


# ----------------------------------------------------------------------
#  CLI
# ----------------------------------------------------------------------

def _cli():
    import argparse

    parser = argparse.ArgumentParser(
        description="OpenRouter free LLM --- one-file chat client",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  %(prog)s chat 'Why is the sky blue?'\n"
            "  %(prog)s chat --system 'You are a poet' 'Write a haiku'\n"
            "  %(prog)s chat --model google/gemini-2.0-flash-exp:free 'Hi'\n"
            "  %(prog)s stream 'Tell me a long story'\n"
            "  %(prog)s models\n"
        ),
    )
    sub = parser.add_subparsers(dest="command", required=True)

    # -- chat --
    chat_p = sub.add_parser("chat", help="Send a prompt and print the reply")
    chat_p.add_argument("prompt", help="Your message")
    chat_p.add_argument("--model", default=None, help="Override model (default: openrouter/free)")
    chat_p.add_argument("--system", default=None, help="System prompt")
    chat_p.add_argument("--temperature", type=float, default=None)
    chat_p.add_argument("--max-tokens", type=int, default=None)
    chat_p.add_argument("--json", action="store_true", help="Show full response (model, usage, reply)")

    # -- stream --
    stream_p = sub.add_parser("stream", help="Stream a reply token-by-token")
    stream_p.add_argument("prompt", help="Your message")
    stream_p.add_argument("--model", default=None)
    stream_p.add_argument("--system", default=None)
    stream_p.add_argument("--temperature", type=float, default=None)
    stream_p.add_argument("--max-tokens", type=int, default=None)

    # -- models --
    sub.add_parser("models", help="List currently available free models from OpenRouter")

    args = parser.parse_args()

    # -- Handle "models" --
    if args.command == "models":
        _list_models()
        return

    # -- Build client --
    overrides = {}
    if args.temperature is not None:
        overrides["temperature"] = args.temperature
    if args.max_tokens is not None:
        overrides["max_tokens"] = args.max_tokens
    if args.model:
        overrides["model"] = args.model

    sys_prompt = args.system or os.getenv("OPENROUTER_SYSTEM_PROMPT")

    llm = FreeLLM(system_prompt=sys_prompt)

    try:
        if args.command == "chat":
            if args.json:
                info = llm.chat_with_info(args.prompt, **overrides)
                print(json.dumps(info, indent=2, ensure_ascii=False))
            else:
                reply = llm.chat(args.prompt, **overrides)
                print(reply)
        elif args.command == "stream":
            for token in llm.chat_stream(args.prompt, **overrides):
                print(token, end="", flush=True)
            print()
    except OpenRouterError as exc:
        print(f"x {exc}", file=sys.stderr)
        sys.exit(1)
    except KeyboardInterrupt:
        print("\nAborted.", file=sys.stderr)
        sys.exit(130)


def _list_models():
    """Fetch and print free models from the /api/v1/models endpoint."""
    try:
        import urllib.request
        resp = urllib.request.urlopen(
            f"{OPENROUTER_API_BASE}/models", timeout=30
        )
        data = json.loads(resp.read().decode())
    except Exception as exc:
        print(f"x Failed to fetch models: {exc}", file=sys.stderr)
        sys.exit(1)

    models = data.get("data", [])
    free = [
        m for m in models
        if m.get("pricing", {}).get("prompt") in ("0", 0)
        and m.get("pricing", {}).get("completion") in ("0", 0)
    ]

    if not free:
        print("No free models found via API. Try: https://openrouter.ai/openrouter/free")
        return

    print(f"-- Free models ({len(free)} available) --\n")
    for m in sorted(free, key=lambda x: x.get("id", "")):
        mid = m.get("id", "?")
        name = m.get("name", mid.split("/")[-1])
        ctx = m.get("context_length", "?")
        print(f"  {mid}")
        print(f"      Name:    {name}")
        print(f"      Context: {ctx:,} tokens" if isinstance(ctx, int) else f"      Context: {ctx}")
        print()


# ----------------------------------------------------------------------
#  Entry
# ----------------------------------------------------------------------

if __name__ == "__main__":
    _cli()
