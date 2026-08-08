import unittest
import sys
import os

PROJECT_ROOT = r"D:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from langchain_core.messages import SystemMessage, HumanMessage, AIMessage
from context_compaction import tier1_compact, tier2_compact, checkpoint_compact, invalidate_stale_reads

class TestCompactionAlgorithms(unittest.TestCase):
    def test_tier1_and_tier2_compaction(self):
        messages = [
            SystemMessage(content="System instruction"),
            HumanMessage(content="Task prompt"),
            AIMessage(content="<thinking>thought block</thinking>```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"a.py\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] a.py\nline 1\nline 2"),
            AIMessage(content="<thinking>another thought</thinking>```tool\n{\"tool\": \"edit_file\", \"args\": {\"file_path\": \"a.py\"}}\n```"),
            HumanMessage(content="[edit_file]:\n[FILE] a.py\n[OK] Edited file"),
            AIMessage(content="<thinking>read again</thinking>```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"a.py\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] a.py\nline 1 updated\nline 2 updated"),
            AIMessage(content="Done"),
        ]

        # 1. Test Invalidate Stale Reads
        invalidated = invalidate_stale_reads(messages)
        # The first read at index 3 should be marked stale because a.py was edited at index 5
        self.assertIn("content stale", invalidated[3].content)

        # 2. Test Tier 1 Compaction
        compacted1 = tier1_compact(messages, keep_last_n=2)
        # Thinking should be stripped from AI messages
        self.assertNotIn("thought block", compacted1[2].content)
        self.assertIn("```tool", compacted1[2].content)
        # Superseded read (at index 3) should be placeholder
        self.assertIn("superseded by a later read", compacted1[3].content)

        # 3. Test Tier 2 Compaction
        compacted2 = tier2_compact(messages, keep_last_n=2)
        # Action tool result (edit_file at index 5) should be condensed to a one-liner
        # (module design: no [TOOL OK] markers — file content is on disk)
        self.assertIn("Wrote/edited", compacted2[5].content)
        self.assertIn("a.py", compacted2[5].content)
        self.assertNotIn("Edited file", compacted2[5].content)

    def test_checkpoint_compact(self):
        messages = [
            SystemMessage(content="System"),
            HumanMessage(content="Task"),
            HumanMessage(content="[read_file]:\n[FILE] a.py\ncontent"),
            HumanMessage(content="[write_file]:\n[FILE] b.py\ncontent"),
            AIMessage(content="Done"),
        ]
        tool_call_log = [
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "content"},
            {"tool": "write_file", "args": {"file_path": "b.py"}, "result_preview": "content"},
        ]
        
        compacted = checkpoint_compact(
            messages,
            tool_call_log=tool_call_log,
            created=["b.py"],
            modified=["a.py"],
            keep_last_n=1
        )
        
        # Check that it condensed messages to: System, Task, Checkpoint Summary message, and last 1 message (Done)
        self.assertEqual(len(compacted), 4)
        self.assertEqual(compacted[0].content, "System")
        self.assertEqual(compacted[1].content, "Task")
        self.assertIn("[SYSTEM INFO] History compacted", compacted[2].content)
        self.assertIn("Files Created: b.py", compacted[2].content)
        self.assertEqual(compacted[3].content, "Done")

if __name__ == "__main__":
    unittest.main()
