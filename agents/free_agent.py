"""
free_agent.py --- Multi-provider free LLM agent.

Single file that routes across 10 free providers:
  OpenRouter, Groq, Google AI Studio, Mistral AI, NVIDIA NIM, Cloudflare, Z.ai (GLM), OpenCode Zen, Kilo Gateway, Cline

Zero cost. Supports text, vision, coding, reasoning, and audio.

Quick start:
    export OPENROUTER_API_KEY="sk-or-v1-..."
    python free_agent.py chat "What is the capital of France?"
    python free_agent.py vision "What's in this photo?" photo.jpg
    python free_agent.py models

As a module:
    import free_agent
    reply = free_agent.chat("Why is the sky blue?")
    print(reply)

Dependencies: none (uses urllib from stdlib)
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

PROVIDER_REQUEST_TIMEOUT = float(os.getenv("FREE_AGENT_REQUEST_TIMEOUT", "30"))
MAX_PROVIDER_RETRY_CYCLES = int(os.getenv("FREE_AGENT_MAX_RETRY_CYCLES", "2"))

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
        "name": "OpenCode Zen",
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
}

# ----------------------------------------------------------------------
#  PHASE 2: MODEL CATALOG
# ----------------------------------------------------------------------

# Each model entry: {id, provider, name, capabilities[], context, priority}

MODEL_CATALOG = [
    # ── OpenRouter ──
    {"id": "openrouter/free", "provider": "openrouter", "name": "Free Models Router (auto)",
     "capabilities": ["text", "vision", "coding", "reasoning"], "context": 200000, "priority": 1},
    {"id": "google/gemma-4-31b-it:free", "provider": "openrouter", "name": "Gemma 4 31B",
     "capabilities": ["text", "vision"], "context": 262144, "priority": 91},
    {"id": "google/gemma-4-26b-a4b-it:free", "provider": "openrouter", "name": "Gemma 4 26B A4B",
     "capabilities": ["text", "vision"], "context": 262144, "priority": 90},
    {"id": "google/lyria-3-pro-preview", "provider": "openrouter", "name": "Lyria 3 Pro",
     "capabilities": ["text", "vision"], "context": 1048576, "priority": 4},
    {"id": "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning:free", "provider": "openrouter", "name": "Nemotron 3 Nano Omni",
     "capabilities": ["text", "vision"], "context": 256000, "priority": 92},
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
    {"id": "llama-4-maverick-17b", "provider": "groq", "name": "Llama 4 Maverick 17B",
     "capabilities": ["text"], "context": 131072, "priority": 90},
    {"id": "qwen3-32b", "provider": "groq", "name": "Qwen3 32B",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 5},
    {"id": "qwen-qwq-32b", "provider": "groq", "name": "Qwen QWQ 32B",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 6},
    {"id": "deepseek-r1-distill-70b", "provider": "groq", "name": "DeepSeek R1 Distill 70B",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 7},
    {"id": "gpt-oss-120b", "provider": "groq", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 8},
    {"id": "mistral-saba-24b", "provider": "groq", "name": "Mistral Saba 24B",
     "capabilities": ["text"], "context": 131072, "priority": 91},
    {"id": "moonshotai/kimi-k2-instruct", "provider": "groq", "name": "Kimi K2 Instruct",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 10},
    {"id": "groq/compound-mini", "provider": "groq", "name": "Compound Mini (agentic)",
     "capabilities": ["text"], "context": 131072, "priority": 11},
    {"id": "whisper-large-v3-turbo", "provider": "groq", "name": "Whisper Large V3 Turbo",
     "capabilities": ["audio"], "context": 0, "priority": 1},

    # ── Google AI Studio / Gemini (free Flash models only) ──
    {"id": "gemini-3.6-flash", "provider": "google", "name": "Gemini 3.6 Flash",
     "capabilities": ["text", "vision", "coding"], "context": 1000000, "priority": 1},
    {"id": "gemini-3.5-flash", "provider": "google", "name": "Gemini 3.5 Flash",
     "capabilities": ["text", "vision", "coding"], "context": 1000000, "priority": 2},
    {"id": "gemini-3.5-flash-lite", "provider": "google", "name": "Gemini 3.5 Flash Lite",
     "capabilities": ["text"], "context": 1000000, "priority": 3},
    {"id": "gemini-3.1-flash-lite", "provider": "google", "name": "Gemini 3.1 Flash Lite",
     "capabilities": ["text"], "context": 1000000, "priority": 90},

    # ── Mistral AI ──
    {"id": "mistral-medium-3.5", "provider": "mistral", "name": "Mistral Medium 3.5",
     "capabilities": ["text", "vision", "reasoning"], "context": 256000, "priority": 1},
    {"id": "mistral-large-3-2512", "provider": "mistral", "name": "Mistral Large 3",
     "capabilities": ["text", "reasoning"], "context": 256000, "priority": 2},
    {"id": "mistral-small-4", "provider": "mistral", "name": "Mistral Small 4",
     "capabilities": ["text"], "context": 256000, "priority": 3},
    {"id": "codestral-2508", "provider": "mistral", "name": "Codestral",
     "capabilities": ["text", "coding"], "context": 256000, "priority": 4},
    {"id": "pixtral-large", "provider": "mistral", "name": "Pixtral Large",
     "capabilities": ["text", "vision"], "context": 128000, "priority": 5},

    # ── NVIDIA NIM (excluded: phi-3-mini) ──
    {"id": "deepseek-ai/deepseek-v4-pro", "provider": "nvidia", "name": "DeepSeek V4 Pro",
     "capabilities": ["text", "coding", "reasoning"], "context": 1000000, "priority": 1},
    {"id": "deepseek-ai/deepseek-v4-flash", "provider": "nvidia", "name": "DeepSeek V4 Flash",
     "capabilities": ["text", "coding"], "context": 1000000, "priority": 2},
    {"id": "nvidia/nemotron-3-ultra-550b-a55b", "provider": "nvidia", "name": "Nemotron 3 Ultra 550B",
     "capabilities": ["text", "reasoning"], "context": 1000000, "priority": 3},
    {"id": "nvidia/nemotron-3-super-120b-a12b", "provider": "nvidia", "name": "Nemotron 3 Super 120B",
     "capabilities": ["text", "reasoning"], "context": 262144, "priority": 4},
    {"id": "nvidia/nemotron-3-nano-30b-a3b", "provider": "nvidia", "name": "Nemotron 3 Nano 30B",
     "capabilities": ["text"], "context": 256000, "priority": 90},
    {"id": "qwen/qwen3-coder-480b", "provider": "nvidia", "name": "Qwen3 Coder 480B",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 7},
    {"id": "nvidia/qwen3.5-397b-a17b", "provider": "nvidia", "name": "Qwen3.5 397B (vision)",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 8},
    {"id": "nvidia/qwen3.5-122b-a10b", "provider": "nvidia", "name": "Qwen3.5 122B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 9},
    {"id": "meta/llama-3.1-405b-instruct", "provider": "nvidia", "name": "Llama 3.1 405B",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 10},
        {"id": "meta/llama-3.3-70b-instruct", "provider": "nvidia", "name": "Llama 3.3 70B",
     "capabilities": ["text"], "context": 131072, "priority": 11},
    {"id": "google/gemma-4-31b-it", "provider": "nvidia", "name": "Gemma 4 31B",
     "capabilities": ["text", "vision"], "context": 262144, "priority": 91},
    {"id": "mistralai/mistral-medium-3.5-128b", "provider": "nvidia", "name": "Mistral Medium 3.5",
     "capabilities": ["text", "vision"], "context": 128000, "priority": 13},
    {"id": "nvidia/glm-5.1", "provider": "nvidia", "name": "GLM 5.1",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 15},
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
    {"id": "GLM-4.5-Flash", "provider": "zhipu", "name": "GLM 4.5 Flash",
     "capabilities": ["text"], "context": 131072, "priority": 2},
    {"id": "GLM-4.6V-Flash", "provider": "zhipu", "name": "GLM 4.6V Flash",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 3},

    # ── OpenCode Zen (free tier) ──
    {"id": "deepseek-v4-flash-free", "provider": "opencode", "name": "DeepSeek V4 Flash (free)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 1},
    {"id": "big-pickle", "provider": "opencode", "name": "Big Pickle (stealth / DS V4)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 2},
    {"id": "qwen3.6-plus-free", "provider": "opencode", "name": "Qwen 3.6 Plus (free)",
     "capabilities": ["text", "vision", "coding"], "context": 200000, "priority": 3},
    {"id": "mimo-v2.5-free", "provider": "opencode", "name": "MiMo V2.5 (free)",
     "capabilities": ["text"], "context": 200000, "priority": 90},
    {"id": "minimax-m3-free", "provider": "opencode", "name": "MiniMax M3 (free)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 5},
    {"id": "nemotron-3-ultra-free", "provider": "opencode", "name": "Nemotron 3 Ultra (free)",
     "capabilities": ["text", "reasoning"], "context": 200000, "priority": 6},
    {"id": "north-mini-code-free", "provider": "opencode", "name": "North Mini Code (free)",
     "capabilities": ["text", "coding"], "context": 200000, "priority": 91},

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
    {"id": "nousresearch/hermes-3-llama-3.1-405b:free", "provider": "kilo", "name": "Hermes 3 405B",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 2},
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
    {"id": "qwen/qwen3-next-80b-a3b-instruct:free", "provider": "kilo", "name": "Qwen3 Next 80B",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 92},
    {"id": "upstage/solar-pro-3:free", "provider": "kilo", "name": "Solar Pro",
     "capabilities": ["text"], "context": 131072, "priority": 90},
    {"id": "corethink:free", "provider": "kilo", "name": "CoreThink (reasoning)",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 13},
    {"id": "poolside/laguna-m.1:free", "provider": "kilo", "name": "Poolside Laguna M.1",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 91},

    # ── Cloudflare Workers AI (excluded: granite, deepseek-r1, gemma-sea-lion) ──
    {"id": "@cf/meta/llama-4-scout-17b-16e-instruct", "provider": "cloudflare", "name": "Llama 4 Scout 17B",
     "capabilities": ["text", "vision"], "context": 1000000, "priority": 1},
    {"id": "@cf/meta/llama-3.3-70b-instruct-fp8-fast", "provider": "cloudflare", "name": "Llama 3.3 70B",
     "capabilities": ["text"], "context": 131072, "priority": 2},
    {"id": "@cf/meta/llama-3.2-11b-vision-instruct", "provider": "cloudflare", "name": "Llama 3.2 11B Vision",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 92},
    {"id": "@cf/openai/gpt-oss-120b", "provider": "cloudflare", "name": "GPT-OSS 120B",
     "capabilities": ["text", "coding", "reasoning"], "context": 131072, "priority": 5},
    {"id": "@cf/openai/gpt-oss-20b", "provider": "cloudflare", "name": "GPT-OSS 20B",
     "capabilities": ["text"], "context": 131072, "priority": 90},
    {"id": "@cf/qwen/qwen3-30b-a3b-fp8", "provider": "cloudflare", "name": "Qwen3 30B",
     "capabilities": ["text"], "context": 131072, "priority": 94},
    {"id": "@cf/qwen/qwq-32b", "provider": "cloudflare", "name": "QWQ 32B (reasoning)",
     "capabilities": ["text", "reasoning"], "context": 131072, "priority": 8},
    {"id": "@cf/qwen/qwen2.5-coder-32b-instruct", "provider": "cloudflare", "name": "Qwen2.5 Coder 32B",
     "capabilities": ["text", "coding"], "context": 131072, "priority": 91},
    {"id": "@cf/moonshotai/kimi-k2.6", "provider": "cloudflare", "name": "Kimi K2.6",
     "capabilities": ["text", "vision"], "context": 131072, "priority": 10},
    {"id": "@cf/moonshot-ai/kimi-k2.7-code", "provider": "cloudflare", "name": "Kimi K2.7 Code",
     "capabilities": ["text", "vision", "coding"], "context": 262144, "priority": 11},
    {"id": "@cf/mistralai/mistral-small-3.1-24b-instruct", "provider": "cloudflare", "name": "Mistral Small 3.1 24B",
     "capabilities": ["text", "vision"], "context": 128000, "priority": 93},
    {"id": "@cf/google/gemma-4-26b-a4b-it", "provider": "cloudflare", "name": "Gemma 4 26B",
     "capabilities": ["text", "vision"], "context": 262144, "priority": 14},
    {"id": "@cf/nvidia/nemotron-3-120b-a12b", "provider": "cloudflare", "name": "Nemotron 3 120B",
     "capabilities": ["text", "reasoning"], "context": 262144, "priority": 15},
    {"id": "@cf/zhipu-ai/glm-4.7-flash", "provider": "cloudflare", "name": "GLM 4.7 Flash",
     "capabilities": ["text"], "context": 131072, "priority": 16},
    {"id": "@cf/glm-5.2", "provider": "cloudflare", "name": "GLM 5.2",
     "capabilities": ["text", "reasoning", "coding"], "context": 131072, "priority": 17},
    {"id": "@cf/openai/whisper", "provider": "cloudflare", "name": "Whisper",
     "capabilities": ["audio"], "context": 0, "priority": 18},
]


# ----------------------------------------------------------------------
#  PHASE 3: CORE BUILDING BLOCKS
# ----------------------------------------------------------------------

class FreeAgentError(Exception):
    """Base error for FreeAgent."""
    def __init__(self, message: str, provider: str = "", status: int = 0):
        self.provider = provider
        self.status = status
        super().__init__(f"[{provider}] {message}" if provider else message)


class AllProvidersExhaustedError(FreeAgentError):
    """All providers failed for this request."""


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

    Handles: OpenRouter, Groq, Google AI Studio, Mistral, NVIDIA NIM, Cloudflare.
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
                raise FreeAgentError("CLOUDFLARE_ACCOUNT_ID not set", provider=self.name)
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
            headers["X-Title"] = os.getenv("OPENROUTER_APP_NAME", "free_agent")
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
    ) -> list[dict]:
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
            "messages": self._build_messages(prompt, system_prompt, images, image_urls),
            "stream": stream,
            **params,
        }
        return body

    # ── HTTP ─────────────────────────────────────────────────────

    def _post(self, body: dict) -> dict:
        import urllib.error
        import urllib.request

        url = self._build_url()
        headers = self._build_headers()
        payload = json.dumps(body).encode()

        req = urllib.request.Request(url, data=payload, headers=headers, method="POST")
        try:
            with urllib.request.urlopen(req, timeout=PROVIDER_REQUEST_TIMEOUT) as resp:
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
            raise FreeAgentError(msg, provider=self.name, status=exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise FreeAgentError(f"{self.name} request failed or timed out: {exc}", provider=self.name, status=0) from exc

    def _parse_reply(self, body: dict) -> str:
        choice = body.get("choices", [{}])[0]
        msg = choice.get("message", {})
        content = msg.get("content", "")
        return content.strip() if content else ""

    def _parse_model_used(self, body: dict) -> str:
        return body.get("model", self.default_model)

    # ── public API ───────────────────────────────────────────────

    def chat(self, prompt: str, **overrides) -> str:
        """Text-only chat. Returns reply string."""
        body = self._build_body(prompt, stream=False, **overrides)
        resp = self._post(body)
        return self._parse_reply(resp)

    def chat_with_info(self, prompt: str, **overrides) -> dict:
        """Text-only chat with metadata."""
        raw = self._post(self._build_body(prompt, stream=False, **overrides))
        return {
            "reply": self._parse_reply(raw),
            "model": self._parse_model_used(raw),
            "provider": self.name,
            "usage": raw.get("usage", {}),
            "raw": raw,
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
            raise FreeAgentError(msg, provider=self.name, status=exc.code) from exc
        except (urllib.error.URLError, TimeoutError, OSError) as exc:
            raise FreeAgentError(f"{self.name} stream failed or timed out: {exc}", provider=self.name, status=0) from exc

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


def _get_models_for_capability(capability: str) -> list[dict]:
    """Get all models that support a capability, sorted by provider priority."""
    cap_priority = {"text": 0, "vision": 1, "coding": 2, "reasoning": 3, "audio": 4}
    cap_idx = cap_priority.get(capability, 0)

    matches = []
    for model in MODEL_CATALOG:
        if capability in model.get("capabilities", []):
            provider = model["provider"]
            pcfg = PROVIDER_CONFIGS.get(provider, {})
            prov_priority = pcfg.get("priority", {}).get(capability, 99)

            # Combined score: provider priority + model priority
            score = prov_priority * 100 + model.get("priority", 99)
            matches.append((score, prov_priority, model))

    matches.sort(key=lambda x: x[0])
    return [m[2] for m in matches]


class FreeAgent:
    """Multi-provider free LLM agent with smart routing and fallback.

    Routes requests to the best available provider based on capability,
    rate limit status, and priority.

    Usage:
        agent = FreeAgent()
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
                "  or OPENCODE_API_KEY for OpenCode Zen\n"
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
    ) -> list[tuple[ProviderClient, str]]:
        """Build an ordered list of (client, model_id) to try.

        Filters by capability, checks rate limits, sorts by priority.
        """
        # Auto-detect capability if images present
        actual_capability = _detect_capability("", images, image_urls)
        if actual_capability != "text":
            capability = actual_capability

        models = _get_models_for_capability(capability)

        # Build route list
        route_list = []
        for model in models:
            client = self.providers.get(model["provider"])
            if client is None:
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

    def _get_shortest_cooldown(self) -> float:
        """Find the shortest cooldown time among all rate-limited providers."""
        now = time.time()
        shortest = 60.0  # max wait
        for client in self.providers.values():
            remaining = client.rate_limiter._cooldown_until - now
            if 0 < remaining < shortest:
                shortest = remaining
        return max(shortest, 5.0)  # at least 5s

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

        Falls back through providers on failure. When all providers
        are exhausted, loops back to the first and retries indefinitely
        (up to 100 cycles).
        """
        max_loops = MAX_PROVIDER_RETRY_CYCLES
        for loop in range(max_loops):
            routes = self._route(capability, preferred_provider, images, image_urls)

            # All providers rate-limited — wait and loop back
            if not routes:
                if loop >= max_loops - 1:
                    raise AllProvidersExhaustedError(
                        "All providers exhausted after 100 retry cycles."
                    )
                wait = self._get_shortest_cooldown()
                time.sleep(wait)
                continue

            last_error: Optional[Exception] = None
            for client, model_id in routes:
                try:
                    overrides["model"] = model_id
                    has_media = (images and len(images) > 0) or (image_urls and len(image_urls) > 0)

                    if has_media:
                        result = client.chat_multimodal(
                            task, images=images, image_urls=image_urls, **overrides
                        )
                    else:
                        result = client.chat(task, **overrides)

                    client.rate_limiter.record_request()
                    self._last_provider = client.name
                    return result

                except FreeAgentError as e:
                    last_error = e
                    if e.status == 401:
                        # Auth error — skip this provider entirely
                        continue
                    elif e.status == 429:
                        # Rate limited — cooldown and try next
                        client.rate_limiter.trigger_cooldown(30)
                        continue
                    else:
                        # Other error — try next
                        continue
                except Exception as e:
                    last_error = e
                    continue

            # All providers in this round failed — wait, then loop back to start
            if loop < max_loops - 1:
                wait = self._get_shortest_cooldown()
                time.sleep(wait)
                continue

        raise AllProvidersExhaustedError(
            f"All providers failed after {max_loops} retry cycles. Last error: {last_error}"
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

        Loops back through providers indefinitely when exhausted.
        """
        max_loops = MAX_PROVIDER_RETRY_CYCLES
        for loop in range(max_loops):
            routes = self._route(capability, preferred_provider, images, image_urls)

            if not routes:
                if loop >= max_loops - 1:
                    raise AllProvidersExhaustedError("No available providers for streaming after 100 retries.")
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
                except FreeAgentError as e:
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
#  PHASE 6: MODULE-LEVEL CONVENIENCE (singleton)
# ----------------------------------------------------------------------

_default_agent: Optional[FreeAgent] = None


def _get_agent() -> FreeAgent:
    global _default_agent
    if _default_agent is None:
        _default_agent = FreeAgent()
    return _default_agent


def chat(task: str, **kwargs) -> str:
    """Send a chat to the best free provider. Module-level convenience."""
    return _get_agent().chat(task, **kwargs)


def chat_stream(task: str, **kwargs):
    """Stream from the best free provider."""
    yield from _get_agent().stream(task, **kwargs)


def configure(enabled_providers: Optional[list[str]] = None):
    """Reconfigure the default agent (e.g., limit to specific providers)."""
    global _default_agent
    _default_agent = FreeAgent(enabled_providers=enabled_providers)


# ----------------------------------------------------------------------
#  PHASE 7: ORCHESTRATOR ENTRY POINT
# ----------------------------------------------------------------------

def free_agent_node(task: str, images: Optional[list[str]] = None, image_urls: Optional[list[str]] = None) -> str:
    """Orchestrator-facing entry point. Called from tools.py -> task('FreeAgent', ...).

    Args:
        task: The user's request or question.
        images: Local image file paths.
        image_urls: Public image URLs.

    Returns:
        Text response from the best available free provider.
    """
    try:
        agent = _get_agent()
        result = agent.chat(task=task, images=images, image_urls=image_urls)
        return result
    except AllProvidersExhaustedError as e:
        return f"[FreeAgent] All providers exhausted: {e}"
    except Exception as e:
        return f"[FreeAgent Error] {e}"


# ----------------------------------------------------------------------
#  PHASE 8: CLI
# ----------------------------------------------------------------------

def _list_models(args=None):
    """Display all models grouped by provider."""
    # Group by provider
    from collections import OrderedDict
    groups = OrderedDict()
    for m in MODEL_CATALOG:
        prov = m["provider"]
        cfg = PROVIDER_CONFIGS.get(prov, {})
        prov_name = cfg.get("name", prov)
        if prov_name not in groups:
            groups[prov_name] = []
        groups[prov_name].append(m)

    for prov_name, models in groups.items():
        print(f"-- {prov_name} ({len(models)} models) --")
        for m in models:
            caps = ", ".join(c for c in ["text", "vision", "coding", "reasoning", "audio"] if c in m.get("capabilities", []))
            ctx = m.get("context", 0)
            ctx_str = f"{ctx:,} ctx" if ctx else ""
            print(f"  {m['id']}")
            print(f"      Name:  {m['name']}")
            print(f"      Tags:  {caps}")
            if ctx_str:
                print(f"      {ctx_str}")
            print()
    print(f"Total: {len(MODEL_CATALOG)} models across {len(groups)} providers")


def _list_providers(args=None):
    """Display which providers are configured."""
    try:
        agent = _get_agent()
        providers = agent.list_configured_providers()
        print("-- Configured Providers --\n")
        for p in providers:
            status = "⚠️ rate limited" if p["rate_limited"] else "ready"
            print(f"  {p['name']:25s}  {status}")
            print(f"      Default: {p['default_model']}")
            print(f"      Models:  {p['models']} free models")
            vision = "yes" if p["supports_vision"] else "no"
            audio = "yes" if p["supports_audio"] else "no"
            print(f"      Vision:  {vision}  Audio: {audio}")
            print()
    except ValueError as e:
        print(f"Not configured: {e}")


def _check_versions(args=None):
    """Probe each provider for the DeepSeek V4 Flash build they currently serve.

    OpenRouter/Kilo expose dated model IDs -> authoritative.
    Cline's chat response reports the real upstream build in the model field.
    OpenCode Zen / NVIDIA NIM expose no version info -> unverifiable.
    """
    import datetime
    import urllib.error
    import urllib.request

    def get_json(url, headers=None):
        req = urllib.request.Request(url, headers=headers or {"User-Agent": "Mozilla/5.0"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                return json.loads(r.read().decode())
        except Exception as e:
            return {"_error": str(e)}

    def fmt_created(ts):
        if not ts:
            return "no timestamp"
        try:
            return datetime.datetime.utcfromtimestamp(ts).strftime("%Y-%m-%d %H:%M UTC")
        except Exception:
            return str(ts)

    print("== DeepSeek V4 Flash build check (run anytime to spot newer builds) ==\n")

    # OpenRouter: dated IDs are authoritative
    data = get_json("https://openrouter.ai/api/v1/models")
    if "data" in data:
        print("-- OpenRouter --")
        for m in data["data"]:
            mid = m.get("id", "")
            if "deepseek" in mid.lower() and "flash" in mid.lower():
                tag = ""
                if mid.endswith(":free"):
                    tag = " [free]"
                print(f"  {mid}{tag}  created: {fmt_created(m.get('created'))}")
    else:
        print(f"-- OpenRouter -- {data.get('_error', data)}")

    # Kilo: mirrors OpenRouter IDs
    try:
        req = urllib.request.Request("https://api.kilo.ai/api/gateway/models", headers={"User-Agent": "Mozilla/5.0"})
        with urllib.request.urlopen(req, timeout=30) as r:
            data = json.loads(r.read().decode())
        print("\n-- Kilo Gateway --")
        for m in data.get("data", []):
            mid = m.get("id", "")
            if "deepseek" in mid.lower() and "flash" in mid.lower():
                print(f"  {mid}")
    except Exception as e:
        print(f"\n-- Kilo Gateway -- {e}")

    # Cline: live probe reveals the real build served
    keys = os.getenv("CLINE_API_KEYS", "").split(",")
    if not keys or not keys[0].strip():
        keys = [os.getenv("CLINE_API_KEY", "")]
    ck = keys[0].strip() if keys else ""
    print("\n-- Cline (usage-billing) --")
    if not ck:
        print("  CLINE_API_KEY not set - skipping live probe")
    else:
        body = {
            "model": "deepseek/deepseek-v4-flash",
            "messages": [{"role": "user", "content": "Say OK."}],
            "max_tokens": 16,
        }
        req = urllib.request.Request(
            "https://api.cline.bot/api/v1/chat/completions",
            data=json.dumps(body).encode(),
            headers={"Content-Type": "application/json", "Authorization": "Bearer " + ck},
            method="POST",
        )
        try:
            with urllib.request.urlopen(req, timeout=60) as r:
                d = json.loads(r.read().decode())
            d = d.get("data", d)
            print(f"  free 'deepseek/deepseek-v4-flash' serves -> {d.get('model', 'unknown')}")
        except urllib.error.HTTPError as e:
            print(f"  live probe failed: HTTP {e.code}")
        except Exception as e:
            print(f"  live probe failed: {e}")

    # OpenCode Zen / NVIDIA NIM: no version info exposed
    print("\n-- OpenCode Zen --")
    print("  'deepseek-v4-flash-free' - unverifiable via API (no build date exposed); Zen refreshes in place")
    print("\n-- NVIDIA NIM --")
    print("  'deepseek-ai/deepseek-v4-flash' - unverifiable via API (no build date exposed); NIM refreshes in place")


def _cli_chat(args):
    """Chat command."""
    try:
        agent = _get_agent()
        overrides = {}
        if args.temperature is not None:
            overrides["temperature"] = args.temperature
        if args.max_tokens is not None:
            overrides["max_tokens"] = args.max_tokens

        result = agent.chat(
            task=args.prompt,
            images=args.image or None,
            image_urls=args.url or None,
            preferred_provider=args.provider,
            **overrides,
        )
        print(result)
        if agent._last_provider:
            print(f"\n--- via {agent._last_provider} ---", file=sys.stderr)
    except AllProvidersExhaustedError as e:
        print(f"All providers failed: {e}", file=sys.stderr)
        sys.exit(1)
    except FreeAgentError as e:
        print(f"API Error: {e}", file=sys.stderr)
        sys.exit(1)
    except ValueError as e:
        print(f"Config: {e}", file=sys.stderr)
        sys.exit(1)



# ----------------------------------------------------------------------
#  General Profile Helper
# ----------------------------------------------------------------------

def build_general_profile_prompt() -> str:
    """Build a prompt prefix from general_profile.json for General mode.
    Returns empty string if profile doesnt exist or has no content.
    """
    gp_path = r"d:\MyProject\LangChain\.deep_agents\general_profile.json"
    if not os.path.isfile(gp_path):
        return ""
    try:
        with open(gp_path, "r", encoding="utf-8") as f:
            gp = json.load(f)
        lines = []
        user = gp.get("user_info", {})
        for key, label in [("name", "User"), ("birthday", "Birthday"), ("education", "Education")]:
            val = user.get(key, "")
            if val:
                lines.append(label + ": " + val)
        inst = gp.get("instructions", "")
        if inst:
            lines.append("Preferences: " + inst)
        mem = gp.get("memory", [])
        for m in mem:
            lines.append("Reminder: " + m)
        if not lines:
            return ""
        return "\n".join(lines) + "\n\n---\n\n"
    except Exception:
        return ""




def run_freeagent_direct(prompt, chat_id="", images=None):
    """Run FreeAgent directly with a tool loop, skipping the orchestrator.
    Used for General mode chat. Yields state dicts (same format as run_and_sync_graph).
    """
    import time, json as _json, re, traceback
    from sync.state_sync import shared_state, safe_update_state, safe_serialize_state

    profile = build_general_profile_prompt()
    tools_desc = (
        "You have these tools available:\n"
        "- run_command(command) - Full POSIX bash (curl for research)\n"
        "- run_command(command) - Full POSIX bash (curl for research)\n"
        "- browser_navigate(url) - Open a webpage\n"
        "- browser_extract(what?, selector?) - Get page content\n"
        "- browser_screenshot(path?) - Take a screenshot\n"
        "- browser_click(selector) - Click an element\n"
        "- browser_fill(selector, text) - Fill an input\n"
        "- create_docx(file_path, title?, content?) - Create Word doc\n"
        "- create_xlsx(file_path, headers, rows) - Create Excel\n"
        "- read_pdf(file_path) - Read a PDF\n\n"
        "When you need to use a tool, output:\n"
        "```tool\n{\"tool\": \"tool_name\", \"args\": {\"param\": \"value\"}}\n```\n"
        "Then wait for the result and continue."
    )

    chat_history = ""
    if chat_id:
        try:
            from memory.chat_context import build_chat_context
            chat_history = build_chat_context("_general", chat_id, max_messages=30, max_chars_per_msg=3000, exclude_traces=True)
        except Exception:
            pass

    system_note = "You are FreeAgent, a general-purpose assistant. Be concise and helpful."
    parts = []
    if profile:
        parts.append(profile)
    parts.append(system_note)
    parts.append(tools_desc)
    if chat_history:
        parts.append("## Chat History\n" + chat_history)
    parts.append("## User\n" + prompt)
    full_prompt = "\n\n".join(parts)

    safe_update_state({"live_terminal_log": "[FreeAgent] Processing...\n", "outputs": {}})
    yield _json.loads(safe_serialize_state())

    try:
        agent = FreeAgent()
        conversation = [{"role": "system", "content": full_prompt}]

        for iteration in range(10):
            last_msg = conversation[-1]["content"]
            # Only the first turn contains the user's visual context. Follow-up
            # tool turns are text-only, so we do not repeatedly re-upload files.
            # P10: prefer token streaming (live deltas to the StreamBus); fall
            # back to a plain chat call if the provider stream fails.
            response = ""
            _emit_chat = chat_id or str(shared_state.get("chat_id") or "")
            try:
                for _tok in agent.stream(task=last_msg, images=images if iteration == 0 else None):
                    response += _tok
                    try:
                        from stream_bus import emit as _bus_emit
                        _bus_emit(_emit_chat, "text", _tok, agent="FreeAgent")
                    except Exception as _bus_err:
                        print(f"[FreeAgent] bus emit skipped: {_bus_err}")
            except Exception as _stream_err:
                print(f"[FreeAgent] stream failed, falling back to chat: {_stream_err}")
                response = agent.chat(task=last_msg, images=images if iteration == 0 else None)
            if not response:
                response = agent.chat(task=last_msg, images=images if iteration == 0 else None)

            tool_calls = []
            for match in re.finditer(r'```tool\n(.*?)\n```', response, re.DOTALL):
                try:
                    tc = _json.loads(match.group(1))
                    if "tool" in tc:
                        tool_calls.append(tc)
                except _json.JSONDecodeError:
                    continue

            if not tool_calls:
                safe_update_state({
                    "live_terminal_log": "[FreeAgent] Done\n",
                    "outputs": {"agent_report": response.strip()},
                })
                yield _json.loads(safe_serialize_state())
                return

            for tc in tool_calls:
                tool_name = tc.get("tool", "")
                args = tc.get("args", {})
                safe_update_state({"live_terminal_log": "[FreeAgent] Using " + tool_name + "...\n"})
                yield _json.loads(safe_serialize_state())
                try:
                    from tools import execute_tool
                    result = execute_tool(tool_name, args)
                except Exception as e:
                    result = "Error: " + str(e)
                conversation.append({"role": "assistant", "content": response})
                conversation.append({"role": "user", "content": "Tool " + tool_name + " returned:\n" + str(result)[:5000]})

        safe_update_state({
            "live_terminal_log": "[FreeAgent] Max iterations reached\n",
            "outputs": {"agent_report": "I reached the maximum number of tool iterations."},
        })
        yield _json.loads(safe_serialize_state())

    except Exception as e:
        safe_update_state({
            "error": str(e),
            "live_terminal_log": "[FreeAgent Error] " + str(e) + "\n" + traceback.format_exc(),
        })
        yield _json.loads(safe_serialize_state())



def _cli():
    import argparse

    parser = argparse.ArgumentParser(
        description="FreeAgent — Multi-provider free LLM CLI (zero cost)",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=(
            "Examples:\n"
            "  %(prog)s chat 'Tell me a joke'\n"
            "  %(prog)s chat --image photo.jpg 'What is this?'\n"
            "  %(prog)s chat --url https://example.com/img.jpg 'Read this sign'\n"
            "  %(prog)s chat --provider groq 'Hello'\n"
            "  %(prog)s models\n"
            "  %(prog)s providers\n"
            "  %(prog)s version-check\n"
        ),
    )

    sub = parser.add_subparsers(dest="command", required=True)

    # -- chat --
    chat_p = sub.add_parser("chat", help="Send a prompt to the best free provider")
    chat_p.add_argument("prompt", help="Your message or question")
    chat_p.add_argument("--image", nargs="+", default=[], help="Local image file path(s)")
    chat_p.add_argument("--url", nargs="+", default=[], help="Public image URL(s)")
    chat_p.add_argument("--provider", help="Preferred provider (openrouter, groq, google, mistral, nvidia, cloudflare, zhipu, opencode, kilo, cline)")
    chat_p.add_argument("--temperature", type=float, default=None)
    chat_p.add_argument("--max-tokens", type=int, default=None)
    chat_p.set_defaults(func=_cli_chat)

    # -- models --
    sub.add_parser("models", help="List all available free models").set_defaults(func=_list_models)

    # -- providers --
    sub.add_parser("providers", help="Show configured providers and status").set_defaults(func=_list_providers)

    # -- version-check --
    sub.add_parser("version-check", help="Show which DeepSeek V4 Flash build each provider serves").set_defaults(func=_check_versions)

    args = parser.parse_args()
    if hasattr(args, "func"):
        args.func(args)
    else:
        parser.print_help()


# ----------------------------------------------------------------------
#  ENTRY
# ----------------------------------------------------------------------

if __name__ == "__main__":
    _cli()
