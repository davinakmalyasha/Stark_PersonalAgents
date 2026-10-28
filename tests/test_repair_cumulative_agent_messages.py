"""The repair script only removes exact repeated prefixes."""
from __future__ import annotations

import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO_ROOT = os.path.dirname(HERE)
if REPO_ROOT not in sys.path:
    sys.path.insert(0, REPO_ROOT)

from scripts.repair_cumulative_agent_messages import _repaired_content


def test_strips_cumulative_prefixes_longest_first():
    repaired, changed = _repaired_content("firstsecondthird", ["first", "second"])
    assert changed is True
    assert repaired == "third"


def test_does_not_touch_a_fresh_reply():
    repaired, changed = _repaired_content("third", ["first", "second"])
    assert changed is False
    assert repaired == "third"


def test_does_not_strip_a_message_that_is_only_a_prior_reply():
    repaired, changed = _repaired_content("first", ["first"])
    assert changed is False
    assert repaired == "first"
