import sys
import os

# Add parent directory to path so we can import context_compaction
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from langchain_core.messages import HumanMessage, AIMessage
from context_compaction import invalidate_stale_reads

def test_stale_reads():
    messages = [
        # Turn 1: Read a file
        HumanMessage(content="[read_file]:\n[FILE] foo.py\nprint('hello')\n"),
        # Turn 2: Edit that file
        AIMessage(content="", tool_calls=[{"name": "edit_file", "args": {"file_path": "foo.py"}, "id": "tc-1"}]),
        HumanMessage(content="[edit_file]:\n[OK] Edited foo.py successfully\n"),
        # Turn 3: Read a different file
        HumanMessage(content="[read_file]:\n[FILE] bar.py\nprint('world')\n"),
        # Turn 4: Read foo.py again (this is the fresh copy)
        HumanMessage(content="[read_file]:\n[FILE] foo.py\nprint('hello updated')\n"),
    ]

    processed = invalidate_stale_reads(messages)
    
    # Assertions:
    # 1. The first read of foo.py (index 0) must be stale
    content_0 = processed[0].content
    assert "content stale - file was modified later" in content_0, f"Expected stale read at index 0, got: {content_0}"
    
    # 2. The read of bar.py (index 3) must NOT be stale
    content_3 = processed[3].content
    assert "content stale" not in content_3, f"Expected non-stale read at index 3, got: {content_3}"
    
    # 3. The second read of foo.py (index 4) must NOT be stale because it happened after the edit
    content_4 = processed[4].content
    assert "content stale" not in content_4, f"Expected non-stale read at index 4, got: {content_4}"
    
    print("SUCCESS: test_stale_reads passed successfully!")

if __name__ == "__main__":
    test_stale_reads()
