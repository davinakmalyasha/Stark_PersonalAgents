"""Regression: a native turn must never repeat the prior assistant transcript.

``AgentRunResult.all_messages()`` includes the caller's ``message_history``.
The old extraction concatenated every TextPart from that whole list, so each
new reply began with every earlier assistant reply. These tests pin the pure
slice/helper used by the fix; they involve no model or network.
"""
from __future__ import annotations

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

from pydantic_ai.messages import ModelResponse, TextPart

from llm.native_tools import _new_messages_since, _text_from_message_slice


def _response(text: str) -> ModelResponse:
    return ModelResponse(parts=[TextPart(content=text)])


def test_new_slice_excludes_prior_history():
    prior = _response("previous reply")
    current = _response("new reply")
    history = [prior]

    appended = _new_messages_since([prior, current], history)

    assert appended == [current]
    assert _text_from_message_slice(appended, [], None) == "new reply"


def test_new_slice_falls_back_to_length_when_objects_are_rebuilt():
    history = [_response("previous reply")]
    rebuilt_prior = _response("previous reply")
    current = _response("new reply")

    appended = _new_messages_since([rebuilt_prior, current], history)

    assert appended == [rebuilt_prior, current]


def test_text_prefers_appended_parts_then_stream_then_result_output():
    current = _response("from message slice")
    assert _text_from_message_slice([current], [{"kind": "text", "text": "from stream"}], "from output") == "from message slice"
    assert _text_from_message_slice([], [{"kind": "text", "text": "from stream"}], "from output") == "from stream"
    assert _text_from_message_slice([], [], "from output") == "from output"
