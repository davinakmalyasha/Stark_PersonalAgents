"""The native agent facade must use the NEWEST user turn as the live prompt.

Regression: seeding multi-turn history made `_split_user_prompt` pick the first
user message, so every run re-executed the oldest request.
"""
from pydantic_ai.messages import (
    ModelRequest, ModelResponse, SystemPromptPart, TextPart, UserPromptPart,
)

from llm.pai_transport import _split_user_prompt


def test_split_user_prompt_takes_latest_turn():
    history = [
        ModelRequest(parts=[SystemPromptPart(content="system")]),
        ModelRequest(parts=[UserPromptPart(content="old request")]),
        ModelResponse(parts=[TextPart(content="old answer")]),
        ModelRequest(parts=[UserPromptPart(content="new request")]),
    ]
    prompt, remaining = _split_user_prompt(history)
    assert prompt == "new request"
    assert len(remaining) == 3
    assert remaining[1].parts[0].content == "old request"
    assert remaining[2].parts[0].content == "old answer"


def test_split_user_prompt_single_turn_unchanged():
    history = [
        ModelRequest(parts=[SystemPromptPart(content="system")]),
        ModelRequest(parts=[UserPromptPart(content="only request")]),
    ]
    prompt, remaining = _split_user_prompt(history)
    assert prompt == "only request"
    assert len(remaining) == 1
