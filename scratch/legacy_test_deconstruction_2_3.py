import unittest
import os
import shutil
from unittest.mock import MagicMock
from tools import auto_offload_result, get_vfs_router
from ba_subagents import SUBAGENTS as ba_subs
from devops_subagents import SUBAGENTS as devops_subs
from analytics_subagents import SUBAGENTS as analytics_subs

class TestDeconstruction2and3(unittest.TestCase):
    def test_auto_offload_small_output(self):
        # Small output should return unchanged
        small = "Hello standard output"
        res = auto_offload_result(small, "test_tool", max_chars=100)
        self.assertEqual(res, small)

    def test_auto_offload_large_output(self):
        # Large output should write to VFS and return native warning template
        large = "Line\n" * 2000  # 10000 characters
        res = auto_offload_result(large, "test_large_tool", max_chars=100)
        
        # Verify it contains native warning components
        self.assertIn("Tool result too large", res)
        self.assertIn("saved in the filesystem at this path:", res)
        self.assertIn("/scratch/large_tool_results/test_large_tool_", res)
        
        # Read the file back via Composite VFS router to check content integrity
        import re
        match = re.search(r"path:\s*([^\s\n]+)", res)
        self.assertTrue(match is not None)
        vfs_path = match.group(1).strip()
        
        from tools import read_file
        read_res = read_file(vfs_path)
        self.assertNotIn("Error:", read_res)
        self.assertIn("Line", read_res)
        self.assertIn("lines 1-300 of 2000", read_res)

    def test_subagent_registries_validity(self):
        # Test that they are lists of dicts conforming to SubAgent structure
        for sub in ba_subs:
            self.assertIn("name", sub)
            self.assertIn("description", sub)
            self.assertIn("system_prompt", sub)
            
        for sub in devops_subs:
            self.assertIn("name", sub)
            self.assertIn("description", sub)
            self.assertIn("system_prompt", sub)
            
        for sub in analytics_subs:
            self.assertIn("name", sub)
            self.assertIn("description", sub)
            self.assertIn("system_prompt", sub)

if __name__ == "__main__":
    unittest.main()
