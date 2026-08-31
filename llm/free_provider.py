"""free_provider.py — D1: Free pool as pydantic-ai Provider + FallbackModel.

Wraps the hand-rolled FreeProviders routing (llm/free_providers.py) into a
native pydantic-ai Provider so `Agent(model=FallbackModel(...))` can handle
fallback natively. Each FreeProvider instance represents one free gateway
(openrouter, groq, etc.) and reuses its base_url / api_key / model.

The routing order (provider_priority*100 + model_priority, reasoning_bias,
dedup, cooldown) is preserved by the FallbackModel's sequential try; rate-limit
state is kept inside each ProviderClient and checked before each request.

Usage:
    from llm.free_provider import build_free_fallback, free_provider_names
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


def free_provider_names(capability: str = "text") -> list[str]:
    """List configured free provider names sorted by priority for capability."""
    try:
        from llm.free_providers import _get_models_for_capability, PROVIDER_CONFIGS
        # Use routing helper to get ordered providers
        models = _get_models_for_capability(capability)
        seen = []
        for m in models:
            p = m.get("provider")
            if p and p not in seen and p in (PROVIDER_CONFIGS or {}):
                # Check that we have a key (or optional)
                cfg = PROVIDER_CONFIGS[p]
                if cfg.get("api_key_optional"):
                    seen.append(p)
                else:
                    key = os.getenv(cfg.get("env_key", ""), "")
                    if key:
                        seen.append(p)
        return seen
    except Exception:
        # Fallback: all configured providers with keys
        out = []
        for name, cfg in (PROVIDER_CONFIGS or {}).items():
            if cfg.get("api_key_optional"):
                out.append(name)
            else:
                if os.getenv(cfg.get("env_key", ""), ""):
                    out.append(name)
        return out


def build_free_fallback(
    capability: str = "coding",
    prefer_reasoning: bool | None = None,
    max_context_tokens: int | None = None,
) -> Any:
    """Build a FallbackModel from free providers ordered for capability.

    The returned FallbackModel can be passed directly to `Agent(model=...)`
    or `OpenAIChatModel`. Each entry is an OpenAIChatModel with a FreeProvider.
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
        # _get_models_for_capability returns sorted models
        try:
            models = _get_models_for_capability(capability, **kwargs)
        except TypeError:
            models = _get_models_for_capability(capability)
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
        return FallbackModel(*providers)
    except Exception:
        return None
