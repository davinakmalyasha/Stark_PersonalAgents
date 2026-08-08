import unittest
import os
import json
from unittest.mock import patch, MagicMock

class TestDeepAgentsIntegration(unittest.TestCase):
    
    def test_harness_profile_registration(self):
        # Test registered developer HarnessProfile
        from developer_agent import get_harness_profile
        try:
            profile = get_harness_profile("developer")
            self.assertIsNotNone(profile)
        except Exception as e:
            self.fail(f"Failed to retrieve developer HarnessProfile: {e}")

    def test_task_tool_subagent_registry(self):
        # Test that subagents are successfully loaded and retrievable
        from tools import _load_subagents, _SUBAGENTS_REGISTRY
        _load_subagents()
        
        # Check some key subagents from BA, SA, DevOps, Analytics
        self.assertIn("BA-GapAnalyzer", _SUBAGENTS_REGISTRY)
        self.assertIn("SA-Database", _SUBAGENTS_REGISTRY)
        self.assertIn("DevOps-Pipeline-Docker", _SUBAGENTS_REGISTRY)
        self.assertIn("Analytics-Auditor", _SUBAGENTS_REGISTRY)

    def test_task_tool_offloading(self):
        # Test that large subagent outputs (>80k chars) are automatically offloaded
        from tools import task as run_task, _SUBAGENTS_REGISTRY, _load_subagents
        _load_subagents()
        
        # Mock invoke_with_fallback to return a very large string
        large_response = "A" * 85000
        
        with patch("llm.invoke_messages_with_fallback", return_value=large_response):
            # DevOps-Issues is expected to return json-formatted or string
            res = run_task("DevOps-Issues", "Some task")
            
            # The return value should be a JSON string describing the offloaded state
            data = json.loads(res)
            self.assertEqual(data["status"], "OFFLOADED")
            self.assertTrue(data["vfs_path"].startswith("/scratch/subagent_output_DevOps-Issues_"))
            
            # Check that the file was written
            from tools import _sanitize_path
            real_path = _sanitize_path(data["vfs_path"])
            self.assertTrue(os.path.exists(real_path))
            
            # Read and assert content size
            with open(real_path, "r", encoding="utf-8") as f:
                saved_content = f.read()
            self.assertEqual(saved_content, large_response)
            
            # Cleanup
            if os.path.exists(real_path):
                os.remove(real_path)

if __name__ == "__main__":
    unittest.main()
