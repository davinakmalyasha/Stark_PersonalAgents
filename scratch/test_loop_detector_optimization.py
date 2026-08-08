import unittest
import sys
import os

sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from loop_detector import LoopGuard

class TestLoopDetectorOptimization(unittest.TestCase):
    def test_stale_read_detection(self):
        # 1. Reading the same file multiple times without edit
        log = [
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "some content"},
            {"tool": "read_file", "args": {"file_path": "a.py"}, "result_preview": "some content"},
        ]
        # Third read of same file with same offset/limit
        res = LoopGuard.check_pre_execute(log, "read_file", {"file_path": "a.py"})
        self.assertIsNotNone(res)
        self.assertEqual(res[0], "STALE")
        self.assertIn("already read 3 times", res[1])

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

    def test_identical_calls_count_warning_and_abort(self):
        # 3. Warning at 3 same calls, abort at 4 (list_files reads)
        log = [
            {"tool": "list_files", "args": {"path": "."}},
            {"tool": "list_files", "args": {"path": "."}},
        ]
        # 3rd call
        res = LoopGuard.check_pre_execute(log, "list_files", {"path": "."})
        self.assertNullOrNone(res) # same_count is 2 (less than warning threshold of 3)

        # Append 3rd call
        log.append({"tool": "list_files", "args": {"path": "."}})
        # 4th call (same_count is 3)
        res = LoopGuard.check_pre_execute(log, "list_files", {"path": "."})
        self.assertIsNotNone(res)
        self.assertEqual(res[0], "WARNING")

        # Append 4th call
        log.append({"tool": "list_files", "args": {"path": "."}})
        # 5th call (same_count is 4)
        res = LoopGuard.check_pre_execute(log, "list_files", {"path": "."})
        self.assertIsNotNone(res)
        self.assertEqual(res[0], "ABORT")

if __name__ == '__main__':
    unittest.main()
