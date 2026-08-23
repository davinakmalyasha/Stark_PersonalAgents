"""
LLM Configuration — DeepSeek-only setup.
Orchestrator → deepseek-v4-flash first (native structured tools), pro fallback.
Subagents → deepseek-v4-flash (fast, native structured tools).
Standard/Lightweight → deepseek-v4-flash. Critic/Fixing → pro-first.

Model names are env-overridable so the whole stack can be routed through a compatible
gateway (e.g. OpenCode Go, https://opencode.ai/zen/go/v1) without code edits:
    DEEPSEEK_MODEL_FLASH=deepseek-v4-flash      # already OpenCode Go's name
    DEEPSEEK_MODEL_PRO=deepseek-v4-pro          # set to an OpenCode model if available
"""
import os

DEEPSEEK_MODEL_FLASH = os.getenv("DEEPSEEK_MODEL_FLASH", "deepseek-v4-flash")
DEEPSEEK_MODEL_PRO = os.getenv("DEEPSEEK_MODEL_PRO", "deepseek-v4-pro")

# ── Vision capability registry ────────────────────────────────────────────────
# Today exactly ONE model accepts image input (deepseek-v4-flash-vision-exp) and
# it is experimental / premium on gateways, so it is used ONLY when a request
# actually carries images (auto-switch in llm._prepare_llm_call). When vision
# goes GA / normal-priced, integration is a one-line change here:
#   - add ids to VISION_MODELS, or
#   - point DEEPSEEK_MODEL_VISION at the GA slug.
# Routing, guards, and the tool image plumbing all key off supports_vision().
DEEPSEEK_MODEL_VISION = os.getenv("DEEPSEEK_MODEL_VISION", "deepseek-v4-flash-vision-exp")
VISION_MODELS = {DEEPSEEK_MODEL_VISION}


def supports_vision(model: str) -> bool:
    """True if the model accepts image input.

    The 'vision' substring keeps future vision variants auto-supported without
    edits; exact ids live in VISION_MODELS."""
    m = (model or "").lower()
    return m in {v.lower() for v in VISION_MODELS} or ("vision" in m)


TASK_CATEGORIES = {
    "orchestrator": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
    ],
    "complex": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
    ],
    "fixing": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
    ],
    "critic": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
    ],
    "standard": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_PRO},
    ],
    "lightweight": [
        {"provider": "deepseek", "model": DEEPSEEK_MODEL_FLASH},
    ],
}


def get_task_category(task_name: str) -> str:
    """Classifies a task or agent role into lightweight, standard, complex, or orchestrator."""
    name_lower = task_name.lower()
    # Orchestrator/developer roles get flash-first with native tool schemas.
    if any(k in name_lower for k in ["developer", "orchestrator"]):
        return "orchestrator"
    if "critic" in name_lower:
        return "critic"
    if "fixing" in name_lower:
        return "fixing"
    elif (
        name_lower in ["sa", "dev", "coder"]
        or name_lower.startswith("sa-")
        or name_lower.startswith("dev-")
        or any(k in name_lower for k in [
            "coder", "architect", "complex",
            "diagnostics", "review", "planner",
        ])
    ):
        return "complex"
    elif (
        name_lower in ["route", "router", "chat", "compact", "archivist"]
        or any(k in name_lower for k in ["summary", "summarize", "compact", "lite"])
    ):
        return "lightweight"
    return "standard"
