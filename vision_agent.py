"""
vision_agent.py — DEPRECATED. Use free_agent.py instead.

This file is kept for backward compatibility. All functionality has been
moved to free_agent.py which supports 6 free providers (OpenRouter, Groq,
Google AI Studio, Mistral, NVIDIA NIM, Cloudflare) instead of just OpenRouter.

To migrate:
    from vision_agent import vision_node  →  from free_agent import free_agent_node
    from vision_agent import VisionClient →  from free_agent import FreeAgent
"""
import warnings
from free_agent import free_agent_node as vision_node, FreeAgent as VisionClient, configure, FreeAgent

warnings.warn(
    "vision_agent.py is deprecated. Use free_agent.py instead: "
    "'from free_agent import free_agent_node'",
    DeprecationWarning,
    stacklevel=2,
)
