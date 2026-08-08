import unittest
import os
import sys

sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from tools import run_js, read_file, write_file

class TestPTCInterpreter(unittest.TestCase):
    def setUp(self):
        self.test_file = "scratch/ptc_test_file.txt"
        self.test_py_file = "scratch/ptc_test_file.py"
        for f in (self.test_file, self.test_py_file):
            if os.path.exists(f):
                try:
                    os.remove(f)
                except Exception:
                    pass

    def tearDown(self):
        for f in (self.test_file, self.test_py_file):
            if os.path.exists(f):
                try:
                    os.remove(f)
                except Exception:
                    pass

    def test_ptc_read_write(self):
        # Test writing and reading files programmatically inside JS interpreter
        code = f"""
        write_file("{self.test_file}", "hello from js sandbox");
        var content = read_file("{self.test_file}");
        content;
        """
        result = run_js(code)
        self.assertIn("hello from js sandbox", result)
        
    def test_ptc_list_files(self):
        # Test listing files programmatically inside JS interpreter
        write_file(self.test_py_file, "print(1)")
        code = """
        var files = list_files("scratch", "*.py");
        files;
        """
        result = run_js(code)
        self.assertIn("ptc_test_file.py", result)

if __name__ == '__main__':
    unittest.main()
