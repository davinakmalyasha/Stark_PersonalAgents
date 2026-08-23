import unittest
import sys
import os

sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from llm.loop_detector import LoopGuard

class TestLoopDetectorOptimization(unittest.TestCase):
    def test_exact_repeat_is_advisory(self):
        # Repeated calls receive a warning; the controller never skips or aborts.
        log = [
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "some content"},
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "some content"},
        ]
        res = LoopGuard.check_pre_execute(log, "read_file", {"file_path": "a.py"})
        self.assertIsNotNone(res)
        self.assertEqual(res[0], "WARNING")
        self.assertIn("exact call #3", res[1])

    def test_reset_on_edit(self):
        # 2. Writing/editing resets stale check
        log = [
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "some content"},
            {"tool": "write_file", "args": {"file_path": "a.py", "content": "new"}, "result_preview": "success"},
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "some content"},
        ]
        res = LoopGuard.check_pre_execute(log, "read_file", {"file_path": "a.py"})
        self.assertNullOrNone(res) # Should be OK to read after write

    def assertNullOrNone(self, val):
        self.assertTrue(val is None)

    def test_identical_calls_warn_without_abort(self):
        # The first warning is emitted on the third exact consecutive call.
        log = [
            {"tool": "list_files", "args": {"path": "."}},
            {"tool": "list_files", "args": {"path": "."}},
        ]
        res = LoopGuard.check_pre_execute(log, "list_files", {"path": "."})
        self.assertIsNotNone(res)
        self.assertEqual(res[0], "WARNING")

        log.append({"tool": "list_files", "args": {"path": "."}})
        res = LoopGuard.check_pre_execute(log, "list_files", {"path": "."})
        self.assertIsNone(res)

        log.append({"tool": "list_files", "args": {"path": "."}})
        res = LoopGuard.check_pre_execute(log, "list_files", {"path": "."})
        self.assertIsNone(res)

if __name__ == '__main__':
    unittest.main()
