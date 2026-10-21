"""free_provider.py — D1: Free pool as pydantic-ai Provider + FallbackModel.

Wraps the hand-rolled FreeProviders routing (llm/free_providers.py) into a
native pydantic-ai Provider so `Agent(model=FallbackModel(...))` can handle
fallback natively. Each FreeProvider instance represents one free gateway
(openrouter, groq, etc.) and reuses its base_url / api_key / model.

The routing order (provider_priority*100 + model_priority, reasoning_bias,
dedup) is preserved by the FallbackModel's sequential try. Note: this
pydantic-ai adapter does NOT consult the pool's rate-limit tracker — that
lives on the hand-rolled path (``free_providers.free_native_turn``).

Usage:
    from llm.free_provider import build_free_fallback
    fallback = build_free_fallback(capability="coding", prefer_reasoning=False)
    agent = Agent(model=fallback, toolsets=[...], deps_type=WorkspaceDeps)
"""
from __future__ import annotations

import os
from typing import Any

from pydantic_ai.providers.openai import OpenAIProvider
try:
    from pydantic_ai.models.fallback import FallbackModel
except ImportError:
    try:
        from pydantic_ai.models import FallbackModel
    except ImportError:
        FallbackModel = None  # type: ignore

# Reuse the catalog/routing from free_providers without duplicating config
try:
    from llm.free_providers import PROVIDER_CONFIGS, MODEL_CATALOG, FreeProviders
except Exception:
    PROVIDER_CONFIGS = {}
    MODEL_CATALOG = []
    FreeProviders = None  # type: ignore


class FreeProvider(OpenAIProvider):
    """One free gateway as an OpenAI-compatible Provider.

    Thin subclass of OpenAIProvider that loads base_url/api_key from
    free_providers.PROVIDER_CONFIGS and selects a model from MODEL_CATALOG.
    """

    def __init__(self, provider_name: str, model_id: str | None = None):
        cfg = (PROVIDER_CONFIGS or {}).get(provider_name, {})
        base_url = cfg.get("base_url") or os.getenv(cfg.get("env_key", ""), "")
        # Handle comma-separated keys; pick first
        api_key = ""
        env_key = cfg.get("env_key", "")
        if env_key:
            raw = os.getenv(env_key, "") or os.getenv(cfg.get("env_key_fallback", ""), "")
            if raw:
                api_key = raw.split(",")[0].split(";")[0].strip()
        # Fallback to explicit key if no env
        if not api_key and cfg.get("auth") != "none":
            api_key = cfg.get("api_key", "") or ""
        # Cloudflare interpolation
        if "{account_id}" in str(base_url):
            acct = os.getenv("CLOUDFLARE_ACCOUNT_ID", "")
            base_url = str(base_url).replace("{account_id}", acct)
        # Select model if not given: first model for this provider in catalog
        if not model_id:
            for m in (MODEL_CATALOG or []):
                if m.get("provider") == provider_name:
                    model_id = m.get("id") or m.get("name") or ""
                    break
        self._free_provider_name = provider_name
        self._free_model_id = model_id or cfg.get("default_model", "")
        super().__init__(base_url=base_url or "https://api.openai.com/v1", api_key=api_key or "sk-free")

    @property
    def name(self) -> str:
        return f"free-{self._free_provider_name}"

    @property
    def free_model_id(self) -> str:
        return self._free_model_id


def build_free_fallback(
    capability: str = "coding",
    prefer_reasoning: bool | None = None,
    max_context_tokens: int | None = None,
    min_context_tokens: int | None = None,
    prefer_context: bool = False,
    preferred_model: str = "",
) -> Any:
    """Build a FallbackModel from free providers ordered for capability.

    The returned FallbackModel can be passed directly to `Agent(model=...)`
    or `OpenAIChatModel`. Each entry is an OpenAIChatModel with a FreeProvider.

    ``min_context_tokens`` excludes models that cannot fit the request;
    ``prefer_context`` sorts largest-context first. ``preferred_model`` moves
    the matching catalog entry (biggest window variant) to the front while the
    remaining pool stays as fallback.
    """
    try:
        from pydantic_ai.models.openai import OpenAIChatModel
        from llm.free_providers import _get_models_for_capability

        # Get ordered (provider, model_id) pairs for capability
        # Reuse FreeProviders routing but for fallback we need model instances
        if FreeProviders is None:
            return None
        # Build ordered list of (provider_name, model_id) for fallback
        # Use the same scoring as FreeProviders._route
        kwargs: dict[str, Any] = {}
        if prefer_reasoning is not None:
            kwargs["prefer_reasoning"] = prefer_reasoning
        if max_context_tokens is not None:
            kwargs["max_context_tokens"] = max_context_tokens
        if min_context_tokens is not None:
            kwargs["min_context_tokens"] = min_context_tokens
        if prefer_context:
            kwargs["prefer_context"] = prefer_context
        # _get_models_for_capability returns sorted models
        try:
            models = _get_models_for_capability(capability, **kwargs)
        except TypeError:
            models = _get_models_for_capability(capability)
        if preferred_model:
            def _norm(value: Any) -> str:
                return str(value or "").strip().lower().replace(":free", "-free")

            want = _norm(preferred_model)
            pinned = [m for m in models if _norm(m.get("id") or m.get("name")) == want]
            rest = [m for m in models if m not in pinned]
            # Biggest window first so the pinned chain starts at its best variant.
            pinned.sort(key=lambda m: -int(m.get("context", 0) or 0))
            models = pinned + rest
        providers: list[Any] = []
        seen_providers: set[str] = set()
        for m in models:
            pname = m.get("provider")
            mid = m.get("id") or m.get("name")
            if not pname or not mid or pname in seen_providers:
                continue
            cfg = (PROVIDER_CONFIGS or {}).get(pname)
            if not cfg:
                continue
            # Check key availability
            if not cfg.get("api_key_optional"):
                key = os.getenv(cfg.get("env_key", ""), "") or os.getenv(cfg.get("env_key_fallback", ""), "")
                if not key:
                    continue
            seen_providers.add(pname)
            try:
                prov = FreeProvider(pname, mid)
                model = OpenAIChatModel(model_name=mid, provider=prov)
                providers.append(model)
            except Exception:
                continue
            if len(providers) >= 8:  # cap fallback chain
                break
        if not providers:
            return None
        if len(providers) == 1:
            return providers[0]
        # Free gateways fail in many ways beyond HTTP errors: malformed bodies
        # (`UnexpectedModelBehavior`), missing models, quota messages. The
        # pydantic-ai default only falls back on ModelAPIError, so one flaky
        # provider (e.g. cline) would abort the whole chain even when a later
        # provider works. Fall back on any exception.
        try:
            return FallbackModel(*providers, fallback_on=Exception)
        except TypeError:
            return FallbackModel(*providers)
    except Exception:
        return None
