import unittest
import os
import sys

sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tools import write_planning_file, read_file

class TestPlanningWriter(unittest.TestCase):
    def test_planning_writer(self):
        test_plan_path = "scratch/temp_planning.md"
        if os.path.exists(test_plan_path):
            os.remove(test_plan_path)

        try:
            print("=== Test: Calling write_planning_file ===")
            result = write_planning_file(
                file_path=test_plan_path,
                goal="Test the planning tool",
                analysis="The codebase needs a test planning tool.",
                proposed_changes="[NEW] scratch/temp_planning.md",
                steps=["Step 1: Verify writing", "Step 2: Clean up"]
            )

            self.assertIn("[OK]", result)

            content = read_file(test_plan_path)

            self.assertIn("# Goal", content)
            self.assertIn("## Codebase Boundary & Fix Strategy", content)
            self.assertIn("- [ ] Step 1: Verify writing", content)
            self.assertIn("- [ ] Step 2: Clean up", content)
        finally:
            if os.path.exists(test_plan_path):
                os.remove(test_plan_path)

if __name__ == '__main__':
    unittest.main()
