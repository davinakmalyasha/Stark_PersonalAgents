import unittest
from unittest.mock import patch
import os
from llm import _compact_prompt
from tools import list_files, _get_cache_version, _increment_cache_version
from langgraph.store.memory import InMemoryStore

class TestCostOptimization(unittest.TestCase):
    def test_compact_prompt(self):
        prompt = """
        System instruction:
        
        - Do not collapse this code block:
        ```python
        def my_func():
            # Keep indentation
            return 42
        ```
        
        Final check.
        """
        compacted = _compact_prompt(prompt)
        self.assertIn("    # Keep indentation", compacted)
        self.assertIn("            return 42", compacted)
        self.assertNotIn("\n\n\n", compacted)
        self.assertIn("System instruction:", compacted)
        self.assertIn("- Do not collapse this code block:", compacted)

    def test_store_based_caching_and_invalidation(self):
        from state_sync import active_store
        mock_store = InMemoryStore()
        token = active_store.set(mock_store)
        try:
            self.assertEqual(_get_cache_version(), 0)
            
            res1 = list_files("scratch", "*.py")
            
            from tools import _sanitize_path
            sanitized = _sanitize_path("scratch")
            norm_path = os.path.normpath(sanitized).lower().replace('\\', '/')
            cache_key_norm = f"list_files:{norm_path}:*.py:False"
            
            item = mock_store.get(("tool_cache",), cache_key_norm)
            self.assertIsNotNone(item)
            self.assertEqual(item.value["version"], 0)
            self.assertEqual(item.value["result"], res1)
            
            _increment_cache_version()
            self.assertEqual(_get_cache_version(), 1)
            
            res2 = list_files("scratch", "*.py")
            item = mock_store.get(("tool_cache",), cache_key_norm)
            self.assertEqual(item.value["version"], 1)
            
        finally:
            active_store.reset(token)

if __name__ == '__main__':
    unittest.main()
