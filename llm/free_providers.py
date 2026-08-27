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
#  PHASE 1: PROVIDER CONFIGS
# ----------------------------------------------------------------------

# Each provider config includes:
#   name, base_url, auth_header, env_keys (list of env var names to check),
#   default_model, rpm, rpd, supports_vision, supports_audio, priority (per capability)

PROVIDER_CONFIGS = {
    "openrouter": {
        "name": "OpenRouter",
        "base_url": "https://openrouter.ai/api/v1",
        "auth": "Bearer",
        "env_key": "OPENROUTER_API_KEYS",
        "env_key_fallback": "OPENROUTER_API_KEY",
        "default_model": "openrouter/free",
        "rpm": 20,
        "rpd": 200,
        "tpm": 0,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 3,
            "vision": 2,
            "coding": 5,
            "reasoning": 4,
            "audio": 99,
        },
    },
    "groq": {
        "name": "Groq",
        "base_url": "https://api.groq.com/openai/v1",
        "auth": "Bearer",
        "env_key": "GROQ_API_KEYS",
        "env_key_fallback": "GROQ_API_KEY",
        "default_model": "llama-3.3-70b-versatile",
        "rpm": 30,
        "rpd": 1000,
        "tpm": 12000,
        "supports_vision": False,
        "supports_audio": True,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 1,
            "vision": 99,
            "coding": 1,
            "reasoning": 1,
            "audio": 1,
        },
    },
    "google": {
        "name": "Google AI Studio",
        "base_url": "https://generativelanguage.googleapis.com/v1beta/openai",
        "auth": "Bearer",
        "env_key": "GEMINI_API_KEYS",
        "env_key_fallback": "GEMINI_API_KEY",
        "default_model": "gemini-3.5-flash",
        "rpm": 15,
        "rpd": 1500,
        "tpm": 1000000,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 2,
            "vision": 1,
            "coding": 4,
            "reasoning": 3,
            "audio": 99,
        },
    },
    "mistral": {
        "name": "Mistral AI",
        "base_url": "https://api.mistral.ai/v1",
        "auth": "Bearer",
        "env_key": "MISTRAL_API_KEYS",
        "env_key_fallback": "MISTRAL_API_KEY",
        "default_model": "mistral-medium-3.5",
        "rpm": 60,
        "rpd": 0,
        "tpm": 500000,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 5,
            "vision": 4,
            "coding": 3,
            "reasoning": 5,
            "audio": 99,
        },
    },
    "nvidia": {
        "name": "NVIDIA NIM",
        "base_url": "https://integrate.api.nvidia.com/v1",
        "auth": "Bearer",
        "env_key": "NVIDIA_API_KEYS",
        "env_key_fallback": "NVIDIA_API_KEY",
        "default_model": "deepseek-ai/deepseek-v4-flash",
        "rpm": 40,
        "rpd": 0,
        "tpm": 0,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 4,
            "vision": 3,
            "coding": 2,
            "reasoning": 2,
            "audio": 99,
        },
    },
    "cloudflare": {
        "name": "Cloudflare Workers AI",
        "base_url": "https://api.cloudflare.com/client/v4/accounts/{account_id}/ai/v1",
        "auth": "Bearer",
        "env_key": "CLOUDFLARE_API_KEYS",
        "env_key_fallback": "CLOUDFLARE_API_TOKEN",
        "default_model": "@cf/meta/llama-3.3-70b-instruct-fp8-fast",
        "rpm": 300,
        "rpd": 0,
        "tpm": 0,
        "supports_vision": False,
        "supports_audio": True,
        "supports_coding": False,
        "supports_reasoning": False,
        "priority": {
            "text": 6,
            "vision": 5,
            "coding": 6,
            "reasoning": 6,
            "audio": 2,
        },
    },
    "zhipu": {
        "name": "Z.ai (GLM)",
        "base_url": "https://api.z.ai/api/paas/v4",
        "auth": "Bearer",
        "env_key": "ZAI_API_KEYS",
        "env_key_fallback": "ZAI_API_KEY",
        "default_model": "GLM-4.7-Flash",
        "rpm": 20,
        "rpd": 500,
        "tpm": 0,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": False,
        "priority": {
            "text": 7,
            "vision": 6,
            "coding": 7,
            "reasoning": 7,
            "audio": 99,
        },
    },
    "opencode": {
        "name": "OpenCode",
        "base_url": "https://opencode.ai/zen/v1",
        "auth": "Bearer",
        "env_key": "OPENCODE_API_KEYS",
        "env_key_fallback": "OPENCODE_API_KEY",
        "default_model": "deepseek-v4-flash-free",
        "rpm": 10,
        "rpd": 1000,
        "tpm": 0,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 0,
            "vision": 7,
            "coding": 0,
            "reasoning": 8,
            "audio": 99,
        },
    },
    "kilo": {
        "name": "Kilo Gateway",
        "base_url": "https://api.kilo.ai/api/gateway",
        "auth": "Bearer",
        "env_key": "KILO_API_KEYS",
        "env_key_fallback": "KILO_API_KEY",
        "default_model": "kilo-auto/free",
        "rpm": 10,
        "rpd": 0,
        "tpm": 0,
        "api_key_optional": True,
        "supports_vision": False,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 9,
            "vision": 99,
            "coding": 9,
            "reasoning": 9,
            "audio": 99,
        },
    },
    "cline": {
        "name": "Cline (usage-billing)",
        "base_url": "https://api.cline.bot/api/v1",
        "auth": "Bearer",
        "env_key": "CLINE_API_KEYS",
        "env_key_fallback": "CLINE_API_KEY",
        "default_model": "deepseek/deepseek-v4-flash",
        "rpm": 20,
        "rpd": 0,
        "tpm": 0,
        "supports_vision": False,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "response_wrapped": True,
        "priority": {
            "text": 0,
            "vision": 99,
            "coding": 0,
            "reasoning": 2,
            "audio": 99,
        },
    },
    "cerebras": {
        "name": "Cerebras",
        "base_url": "https://api.cerebras.ai/v1",
        "auth": "Bearer",
        "env_key": "CEREBRAS_API_KEYS",
        "env_key_fallback": "CEREBRAS_API_KEY",
        "default_model": "gpt-oss-120b",
        "rpm": 5,
        "rpd": 0,
        "tpm": 30000,
        "supports_vision": False,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 2,
            "vision": 99,
            "coding": 2,
            "reasoning": 2,
            "audio": 99,
        },
    },
    "vercel_gateway": {
        "name": "Vercel AI Gateway",
        "base_url": "https://ai-gateway.vercel.sh/v1",
        "auth": "Bearer",
        "env_key": "VERCEL_AI_GATEWAY_KEYS",
        "env_key_fallback": "AI_GATEWAY_API_KEY",
        "default_model": "minimax/minimax-m2.7-free",
        "rpm": 60,
        "rpd": 0,
        "tpm": 0,
        "supports_vision": True,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 4,
            "vision": 5,
            "coding": 5,
            "reasoning": 5,
            "audio": 99,
        },
    },
    "llm7": {
        "name": "LLM7.io",
        "base_url": "https://api.llm7.io/v1",
        "auth": "Bearer",
        "env_key": "LLM7_API_KEYS",
        "env_key_fallback": "LLM7_API_KEY",
        "default_model": "DeepSeek-V4-Flash-0731",
        "rpm": 40,
        "rpd": 0,
        "tpm": 0,
        "supports_vision": False,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 6,
            "vision": 99,
            "coding": 6,
            "reasoning": 6,
            "audio": 99,
        },
    },
    "glhf": {
        "name": "GLHF.chat",
        "base_url": "https://glhf.chat/api/openai/v1",
        "auth": "Bearer",
        "env_key": "GLHF_API_KEYS",
        "env_key_fallback": "GLHF_API_KEY",
        "default_model": "",
        "rpm": 20,
        "rpd": 0,
        "tpm": 0,
        "supports_vision": False,
        "supports_audio": False,
        "supports_coding": True,
        "supports_reasoning": True,
        "priority": {
            "text": 9,
            "vision": 99,
            "coding": 9,
            "reasoning": 9,
            "audio": 99,
        },
    },
}

# ----------------------------------------------------------------------
#  PHASE 2: MODEL CATALOG
# ----------------------------------------------------------------------

# Each model entry: {id, provider, name, capabilities[], context, priority}
MODEL_CATALOG = [
    # ── OpenRouter ──
    {"id": "openrouter/free", "provider": "openrouter", "name": "Free Models Router (auto)",
     "capabilities": ["text", "vision", "coding", "reasoning"], "context": 200000, "priority": 1},
    {"id": "inclusionai/ling-3.0-flash:free", "provider": "openrouter", "name": "Ling 3.0 Flash",
     "capabilities": ["text", "coding"], "context": 262144, "priority": 6},
    {"id": "nvidia/nemotron-3-ultra-550b-a55b:free", "provider": "openrouter", "name": "Nemotron 3 Ultra",
     "capabilities": ["text", "reasoning"], "context": 1000000, "priority": 7},
    {"id": "cohere/north-mini-code:free", "provider": "openrouter", "name": "North Mini Code",
     "capabilities": ["text", "coding"], "context": 256000, "priority": 8},

    # ── Groq (excluded: allam-2-7b, gemma-3) ──
    {"id": "llama-3.3-70b-versatile", "provider": "groq", "name": "Llama 3.3 70B",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 1},
    {"id": "meta-llama/llama-4-scout-17b-16e-instruct", "provider": "groq", "name": "Llama 4 Scout 17B",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 2},
    {"id": "qwen3-32b", "provider": "groq", "name": "Qwen3 32B",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 5},
    {"id": "gpt-oss-120b", "provider": "groq", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 8},
    {"id": "moonshotai/kimi-k2-instruct", "provider": "groq", "name": "Kimi K2 Instruct",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 10},

    # ── Google AI Studio / Gemini (free Flash models only) ──
    {"id": "gemini-3.6-flash", "provider": "google", "name": "Gemini 3.6 Flash",
     "capabilities": ["text", "vision", "coding"], "context": 1000000, "priority": 1},
    {"id": "gemini-3.5-flash", "provider": "google", "name": "Gemini 3.5 Flash",
     "capabilities": ["text", "vision", "coding"], "context": 1000000, "priority": 2},

    # ── Mistral AI ──
    {"id": "mistral-medium-3.5", "provider": "mistral", "name": "Mistral Medium 3.5",
     "capabilities": ["text", "vision", "reasoning"], "context": 256000, "priority": 1},
    {"id": "mistral-large-3-2512", "provider": "mistral", "name": "Mistral Large 3",
     "capabilities": ["text", "reasoning"], "context": 256000, "priority": 2},
    {"id": "codestral-2508", "provider": "mistral", "name": "Codestral",
     "capabilities": ["text", "coding"], "context": 256000, "priority": 4},

    # ── NVIDIA NIM (excluded: phi-3-mini) ──
    {"id": "deepseek-ai/deepseek-v4-pro", "provider": "nvidia", "name": "DeepSeek V4 Pro",
     "capabilities": ["text", "coding", "reasoning"], "context": 1000000, "priority": 1},
    {"id": "deepseek-ai/deepseek-v4-flash", "provider": "nvidia", "name": "DeepSeek V4 Flash",
     "capabilities": ["text", "coding"], "context": 1000000, "priority": 2},
    {"id": "nvidia/nemotron-3-ultra-550b-a55b", "provider": "nvidia", "name": "Nemotron 3 Ultra 550B",
     "capabilities": ["text", "reasoning"], "context": 1000000, "priority": 3},
    {"id": "nvidia/nemotron-3-super-120b-a12b", "provider": "nvidia", "name": "Nemotron 3 Super 120B",
     "capabilities": ["text", "reasoning"], "context": 262144, "priority": 4},
    {"id": "qwen/qwen3-coder-480b", "provider": "nvidia", "name": "Qwen3 Coder 480B",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 7},
    {"id": "nvidia/qwen3.5-397b-a17b", "provider": "nvidia", "name": "Qwen3.5 397B (vision)",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 8},
    {"id": "nvidia/qwen3.5-122b-a10b", "provider": "nvidia", "name": "Qwen3.5 122B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 9},
        {"id": "nvidia/glm-5.2", "provider": "nvidia", "name": "GLM 5.2",
     "capabilities": ["text", "reasoning", "coding"], "context": 131072, "priority": 16},
    {"id": "nvidia/step-3.7-flash", "provider": "nvidia", "name": "Step 3.7 Flash",
     "capabilities": ["text", "reasoning", "coding"], "context": 131072, "priority": 17},
    {"id": "nvidia/minimax-m2.7", "provider": "nvidia", "name": "MiniMax M2.7",
     "capabilities": ["text", "coding", "reasoning"], "context": 196608, "priority": 18},
    {"id": "nvidia/kimi-k2.6", "provider": "nvidia", "name": "Kimi K2.6",
     "capabilities": ["text", "vision", "coding"], "context": 262144, "priority": 19},

    # ── Z.ai / GLM (free Flash models) ──
    {"id": "GLM-4.7-Flash", "provider": "zhipu", "name": "GLM 4.7 Flash",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 1},
    {"id": "GLM-4.6V-Flash", "provider": "zhipu", "name": "GLM 4.6V Flash",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 3},

    # ── OpenCode (free tier) ──
    {"id": "deepseek-v4-flash-free", "provider": "opencode", "name": "DeepSeek V4 Flash (free)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 1},
    {"id": "big-pickle", "provider": "opencode", "name": "Big Pickle (stealth / DS V4)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 2},
    {"id": "qwen3.6-plus-free", "provider": "opencode", "name": "Qwen 3.6 Plus (free)",
     "capabilities": ["text", "vision", "coding"], "context": 200000, "priority": 3},
    {"id": "minimax-m3-free", "provider": "opencode", "name": "MiniMax M3 (free)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 5},
    {"id": "nemotron-3-ultra-free", "provider": "opencode", "name": "Nemotron 3 Ultra (free)",
     "capabilities": ["text", "reasoning"], "context": 200000, "priority": 6},

    # ── Cline (usage-billing, free tier; verified serving deepseek-v4-flash-0731) ──
    {"id": "deepseek/deepseek-v4-flash", "provider": "cline", "name": "DeepSeek V4 Flash 0731 (free)",
     "capabilities": ["text", "coding"], "context": 1000000, "priority": 1},
    {"id": "stepfun/step-3.7-flash", "provider": "cline", "name": "Step 3.7 Flash (free)",
     "capabilities": ["text", "coding"], "context": 256000, "priority": 2},
    {"id": "poolside/laguna-s-2.1:free", "provider": "cline", "name": "Laguna S 2.1 (free)",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 3},

    # ── Kilo Gateway (free tier, no API key needed) ──
    {"id": "kilo-auto/free", "provider": "kilo", "name": "Kilo Auto (free router)",
     "capabilities": ["text", "coding", "reasoning"], "context": 256000, "priority": 1},
    {"id": "qwen/qwen3-coder:free", "provider": "kilo", "name": "Qwen3 Coder 480B",
     "capabilities": ["text", "coding"], "context": 262144, "priority": 3},
    {"id": "z-ai/glm-5:free", "provider": "kilo", "name": "GLM 5",
     "capabilities": ["text", "coding", "reasoning"], "context": 202000, "priority": 4},
    {"id": "deepseek/deepseek-r1-0528:free", "provider": "kilo", "name": "DeepSeek R1 (reasoning)",
     "capabilities": ["text", "reasoning"], "context": 164000, "priority": 5},
    {"id": "meta-llama/llama-3.3-70b-instruct:free", "provider": "kilo", "name": "Llama 3.3 70B",
     "capabilities": ["text", "reasoning"], "context": 128000, "priority": 6},
    {"id": "nvidia/nemotron-3-ultra-550b-a55b:free", "provider": "kilo", "name": "Nemotron 3 Ultra 550B",
     "capabilities": ["text", "reasoning"], "context": 1000000, "priority": 7},
    {"id": "openai/gpt-oss-120b:free", "provider": "kilo", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 8},
    {"id": "minimax/minimax-m2.5:free", "provider": "kilo", "name": "MiniMax M2.5",
     "capabilities": ["text", "coding"], "context": 205000, "priority": 9},
    {"id": "stepfun/step-3.7-flash:free", "provider": "kilo", "name": "Step 3.7 Flash 196B",
     "capabilities": ["text", "coding"], "context": 256000, "priority": 10},

    # ── Cloudflare Workers AI (excluded: granite, deepseek-r1, gemma-sea-lion) ──
    {"id": "@cf/meta/llama-4-scout-17b-16e-instruct", "provider": "cloudflare", "name": "Llama 4 Scout 17B",
     "capabilities": ["text", "vision"], "context": 1000000, "priority": 1},
    {"id": "@cf/meta/llama-3.3-70b-instruct-fp8-fast", "provider": "cloudflare", "name": "Llama 3.3 70B",
     "capabilities": ["text"], "context": 131072, "priority": 2},
    {"id": "@cf/openai/gpt-oss-120b", "provider": "cloudflare", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 5},
    {"id": "@cf/moonshotai/kimi-k2.6", "provider": "cloudflare", "name": "Kimi K2.6",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 10},
    {"id": "@cf/moonshot-ai/kimi-k2.7-code", "provider": "cloudflare", "name": "Kimi K2.7 Code",
     "capabilities": ["text", "vision", "coding"], "context": 262144, "priority": 11},
    {"id": "@cf/nvidia/nemotron-3-120b-a12b", "provider": "cloudflare", "name": "Nemotron 3 120B",
     "capabilities": ["text", "reasoning"], "context": 262144, "priority": 15},
    {"id": "@cf/zhipu-ai/glm-4.7-flash", "provider": "cloudflare", "name": "GLM 4.7 Flash",
     "capabilities": ["text"], "context": 131072, "priority": 16},
    {"id": "@cf/glm-5.2", "provider": "cloudflare", "name": "GLM 5.2",
     "capabilities": ["text", "reasoning", "coding"], "context": 131072, "priority": 17},
    # ── Cerebras (wafer-scale; free tier 5 RPM / 30K TPM / 1M tokens/day) ──
    {"id": "gpt-oss-120b", "provider": "cerebras", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 1},

    # ── Vercel AI Gateway (free rotating open models; 60 RPM shared) ──
    {"id": "minimax/minimax-m2.7-free", "provider": "vercel_gateway", "name": "MiniMax M2.7 (free)",
     "capabilities": ["text", "coding", "reasoning"], "context": 196608, "priority": 1},
    {"id": "minimax/minimax-m3-free", "provider": "vercel_gateway", "name": "MiniMax M3 (free)",
     "capabilities": ["text", "coding", "reasoning"], "context": 196608, "priority": 2},
    {"id": "openai/gpt-oss-120b", "provider": "vercel_gateway", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 3},

    # ── LLM7.io (free router; token from token.llm7.io; contexts conservative) ──
    {"id": "DeepSeek-V4-Flash-0731", "provider": "llm7", "name": "DeepSeek V4 Flash 0731",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 1},
    {"id": "glm-5.3", "provider": "llm7", "name": "GLM 5.3",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 2},
    {"id": "kimi-k2.6", "provider": "llm7", "name": "Kimi K2.6",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 3},
    {"id": "minimax-m2.7", "provider": "llm7", "name": "MiniMax M2.7",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 4},

    # ── OpenRouter newest-generation :free slots (verified 2026-08) ──
    {"id": "thinkingmachines/inkling:free", "provider": "openrouter", "name": "Thinking Machines Inkling",
     "capabilities": ["text", "vision", "coding", "reasoning"], "context": 1048576, "priority": 9},
    {"id": "thinkingmachines/inkling-small:free", "provider": "openrouter", "name": "Inkling Small",
     "capabilities": ["text", "vision", "coding", "reasoning"], "context": 1048576, "priority": 10},
    {"id": "minimax/minimax-m3:free", "provider": "openrouter", "name": "MiniMax M3",
     "capabilities": ["text", "coding", "reasoning"], "context": 1048576, "priority": 6},
    {"id": "nvidia/nemotron-3.5-lightning:free", "provider": "openrouter", "name": "Nemotron 3.5 Lightning",
     "capabilities": ["text", "reasoning"], "context": 1000000, "priority": 9},
    {"id": "poolside/laguna-s-2.1:free", "provider": "openrouter", "name": "Laguna S 2.1",
     "capabilities": ["text", "coding"], "context": 262144, "priority": 8},

    # ── Vercel AI Gateway newest (laguna -free verified; others metered-unless-rotated) ──
    {"id": "poolside/laguna-s-2.1-free", "provider": "vercel_gateway", "name": "Laguna S 2.1 (free)",
     "capabilities": ["text", "coding"], "context": 262144, "priority": 4},
    {"id": "moonshotai/kimi-k3", "provider": "vercel_gateway", "name": "Kimi K3",
     "capabilities": ["text", "coding", "reasoning"], "context": 262144, "priority": 5},
    {"id": "zai/glm-5.3", "provider": "vercel_gateway", "name": "GLM 5.3",
     "capabilities": ["text", "coding", "reasoning"], "context": 202000, "priority": 6},
    {"id": "deepseek/deepseek-v4-pro-0813", "provider": "vercel_gateway", "name": "DeepSeek V4 Pro 0813",
     "capabilities": ["text", "coding", "reasoning"], "context": 1000000, "priority": 7},
    {"id": "alibaba/qwen3.8-max", "provider": "vercel_gateway", "name": "Qwen 3.8 Max",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 8},

    # ── NVIDIA NIM newest dev-tier additions ──
    {"id": "moonshotai/kimi-k3", "provider": "nvidia", "name": "Kimi K3",
     "capabilities": ["text", "coding", "reasoning"], "context": 262144, "priority": 1},
    {"id": "minimaxai/minimax-m3", "provider": "nvidia", "name": "MiniMax M3",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 3},

    # ── Kilo Gateway auto tiers + newest :free ──
    {"id": "kilo-auto/frontier", "provider": "kilo", "name": "Kilo Auto Frontier",
     "capabilities": ["text", "coding", "reasoning"], "context": 256000, "priority": 1},
    {"id": "nvidia/nemotron-3.5-lightning:free", "provider": "kilo", "name": "Nemotron 3.5 Lightning (free)",
     "capabilities": ["text", "reasoning"], "context": 1000000, "priority": 2},

]
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

    # ── key loading ──────────────────────────────────────────────

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

    # ── URL building ─────────────────────────────────────────────

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

    # ── header / body building ───────────────────────────────────

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

    # ── HTTP ─────────────────────────────────────────────────────

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

    # ── public API ───────────────────────────────────────────────

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
    # ±30 stays below the provider-priority quantum (100): routing bias
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
        return max(shortest, 2.0)  # at least 2s — fast fallback beats patience

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

            # All providers rate-limited — wait and loop back
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
                        # Auth error — this key will not heal; disable the
                        # provider for the lifetime of the pool.
                        client.auth_dead = True
                        continue
                    elif e.status == 429:
                        # Rate limited — cooldown and try next
                        client.rate_limiter.trigger_cooldown(30)
                        continue
                    elif e.status == 0 or e.status >= 500:
                        # Transport/server failure — escalate cooldown so
                        # repeated dead providers stop costing us seconds.
                        client.fail_streak += 1
                        client.rate_limiter.trigger_cooldown(
                            min(90.0, 30.0 * client.fail_streak)
                        )
                        continue
                    else:
                        # Other error — try next
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

            # All providers in this round failed — wait, then loop back to start
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
        """Routed streaming turn — same fallback ladder as chat(), but the
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
    out: list[dict] = []
    for m in messages:
        kind = type(m).__name__
        content = getattr(m, "content", "")
        if kind == "SystemMessage":
            out.append({"role": "system", "content": content})
        elif kind == "HumanMessage":
            out.append({"role": "user", "content": content})
        elif kind == "AIMessage":
            entry = {"role": "assistant", "content": content or ""}
            tcs = getattr(m, "tool_calls", None) or []
            if tcs:
                entry["tool_calls"] = [
                    {
                        "id": tc.get("id", ""),
                        "type": "function",
                        "function": {
                            "name": tc.get("name") or tc.get("tool", ""),
                            "arguments": json.dumps(
                                tc.get("args") or {}, separators=(",", ":")
                            ),
                        },
                    }
                    for tc in tcs
                    if isinstance(tc, dict)
                ]
            out.append(entry)
        elif kind == "ToolMessage":
            out.append(
                {
                    "role": "tool",
                    "tool_call_id": getattr(m, "tool_call_id", "") or "",
                    "name": getattr(m, "name", "") or "",
                    "content": str(content),
                }
            )
    return out


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
        from stream_bus import emit as _bus_emit

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
    consumes. Raises loudly when the whole pool fails — no silent paid
    fallback by design.
    """
    pool = FreeProviders()
    native_msgs = messages if (
        messages and isinstance(messages[0], dict)
    ) else to_native_messages(messages or [])
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
