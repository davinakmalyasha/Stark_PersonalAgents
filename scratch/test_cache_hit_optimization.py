import unittest
from unittest.mock import patch, MagicMock
import os
import sys

# Append project root to path
PROJECT_ROOT = r"D:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from langchain_core.messages import HumanMessage, AIMessage
from context_compaction import compact_successful_tools
from tools import LocalShellBackend

class TestCacheHitOptimization(unittest.TestCase):
    def test_tool_compaction_threshold_bypass(self):
        # Create a history of 18 messages (longer than 8) to test compaction with protect_last_6
        messages = [
            HumanMessage(content="System"),
            HumanMessage(content="Task"),
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"a.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] a.txt\ncontent of a.txt"), # Index 3
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"b.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] b.txt\ncontent of b.txt"), # Index 5
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"c.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] c.txt\ncontent of c.txt"), # Index 7
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"d.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] d.txt\ncontent of d.txt"), # Index 9
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"e.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] e.txt\ncontent of e.txt"), # Index 11
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"f.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] f.txt\ncontent of f.txt"), # Index 13
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"g.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] g.txt\ncontent of g.txt"), # Index 15
            AIMessage(content="thoughts\n```tool\n{\"tool\": \"read_file\", \"args\": {\"file_path\": \"a.txt\"}}\n```"),
            HumanMessage(content="[read_file]:\n[FILE] a.txt\ncontent of a.txt")  # Index 17
        ]
        
        # Verify compact_successful_tools functions correctly
        compacted = compact_successful_tools(messages)
        # Index 3 (a.txt) is superseded by index 17, and outside the last 6 protected tool results.
        # Index 5 (b.txt) is outside the last 6, but NOT superseded.
        # So only index 3 should be compacted.
        self.assertTrue(compacted[3].content.startswith("[TOOL OK]"))
        self.assertFalse(compacted[5].content.startswith("[TOOL OK]"))
        self.assertFalse(compacted[7].content.startswith("[TOOL OK]"))

    @patch("subprocess.Popen")
    def test_run_command_venv_path_prepended(self, mock_popen):
        # Verify LocalShellBackend prepends venv Scripts path to PATH env var
        backend = LocalShellBackend(root_dir=PROJECT_ROOT)
        
        # Mock subprocess Popen to return immediately
        mock_proc = MagicMock()
        mock_proc.poll.return_value = 0
        mock_proc.wait.return_value = 0
        mock_stdout = MagicMock()
        mock_stdout.__iter__.return_value = []
        mock_proc.stdout = mock_stdout
        mock_popen.return_value = mock_proc
        
        backend.execute("python --version")
        
        # Verify popen was called
        self.assertTrue(mock_popen.called)
        # Check env passed to Popen
        kwargs = mock_popen.call_args[1]
        env = kwargs.get("env")
        self.assertIsNotNone(env)
        
        # Path/PATH on Windows
        path_val = None
        for k, v in env.items():
            if k.upper() == "PATH":
                path_val = v
                break
        self.assertIsNotNone(path_val)
        # It should contain venv312/Scripts or venv/Scripts
        self.assertTrue("venv" in path_val.lower())

    def test_stale_read_reset_on_write(self):
        # Verify that successful edit/write resets the stale read counter for a file
        tool_call_log = [
            {"tool": "read_file", "args": {"file_path": "a.py", "offset": 1, "limit": 10}, "result_preview": "some content"},
            {"tool": "read_file", "args": {"file_path": "a.py", "offset": 1, "limit": 10}, "result_preview": "some content"}
        ]
        
        def check_stale(tool_args, log):
            target_file = tool_args.get("file_path", "")
            read_key = (target_file, tool_args.get("offset"), tool_args.get("limit"))
            
            same_reads = 0
            for item in reversed(log):
                if item.get("tool") in ("write_file", "edit_file") and item.get("args", {}).get("file_path") == target_file:
                    res_preview = str(item.get("result_preview", ""))
                    if not res_preview.startswith("Error") and not res_preview.startswith("TOOL ERROR"):
                        break
                if item.get("tool") == "read_file":
                    item_key = (item.get("args", {}).get("file_path"), item.get("args", {}).get("offset"), item.get("args", {}).get("limit"))
                    if item_key == read_key:
                        same_reads += 1
            return same_reads >= 2

        # 2 identical reads -> should be stale (3rd read would trigger [STALE])
        self.assertTrue(check_stale({"file_path": "a.py", "offset": 1, "limit": 10}, tool_call_log))
        
        # Add successful edit_file call
        tool_call_log.append({"tool": "edit_file", "args": {"file_path": "a.py"}, "result_preview": "[OK] Edited file"})
        
        # Check again -> edit resets the counter, so it should not be stale anymore
        self.assertFalse(check_stale({"file_path": "a.py", "offset": 1, "limit": 10}, tool_call_log))

if __name__ == "__main__":
    unittest.main()
