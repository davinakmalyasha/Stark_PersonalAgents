"""
Context Budget Management.
Provides token estimation for context window management.
"""


def estimate_tokens(text: str) -> int:
    """Fast heuristic: ~3.5 chars per token for mixed code/text."""
    if not text:
        return 0
    return max(1, len(text) // 3)
