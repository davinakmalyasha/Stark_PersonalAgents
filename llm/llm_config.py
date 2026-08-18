"""
LLM Configuration — DeepSeek-only setup.
Orchestrator → deepseek-v4-flash first (must follow ```tool format precisely), pro fallback.
Subagents → deepseek-v4-flash (fast, XML/DSML output parsed by multi-format parser).
Standard/Lightweight → deepseek-v4-flash. Critic/Fixing → pro-first.

Model names are env-overridable so the whole stack can be routed through a compatible
gateway (e.g. OpenCode Zen) without code edits:
    DEEPSEEK_MODEL_FLASH=deepseek-v4-flash      # already OpenCode Zen's name
    DEEPSEEK_MODEL_PRO=deepseek-v4-pro          # set to an OpenCode model if available
"""
import os

DEEPSEEK_MODEL_FLASH = os.getenv("DEEPSEEK_MODEL_FLASH", "deepseek-v4-flash")
DEEPSEEK_MODEL_PRO = os.getenv("DEEPSEEK_MODEL_PRO", "deepseek-v4-pro")


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
    # Orchestrator roles get flash-first (must follow ```tool format exactly)
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
