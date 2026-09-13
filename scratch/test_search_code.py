"""search_code checks: scoping, output modes, limits, annotation, snippets."""
import os
import shutil
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

FIXTURE_REL = os.path.join("scratch", "_search_test_tmp")
FIXTURE = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), FIXTURE_REL)


def _search(**kw):
    from tools.MainTool.searchCode import search_code
    return search_code(**kw)


class SearchCodeTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        shutil.rmtree(FIXTURE, ignore_errors=True)
        os.makedirs(FIXTURE)
        with open(os.path.join(FIXTURE, "alpha.py"), "w", encoding="utf-8") as f:
            f.write("import os\n")
            f.write("def alpha_func():\n")
            for i in range(39):
                f.write(f"    TARGET_VALUE = {i}\n")
            f.write("class AlphaClass:\n")
            f.write("    def method(self):\n")
            f.write("        TARGET_VALUE = 99\n")
        with open(os.path.join(FIXTURE, "beta.py"), "w", encoding="utf-8") as f:
            for i in range(3):
                f.write(f"target_value = {i}\n")
        with open(os.path.join(FIXTURE, "gamma.txt"), "w", encoding="utf-8") as f:
            f.write("TARGET_VALUE here\n")
            f.write("after line\n")
        with open(os.path.join(FIXTURE, "notes.test.py"), "w", encoding="utf-8") as f:
            f.write("TARGET_VALUE in tests\n")
        with open(os.path.join(FIXTURE, "binary.bin"), "wb") as f:
            f.write(b"\x00\x01\x02TARGET_VALUE\x00\xff")

    @classmethod
    def tearDownClass(cls):
        shutil.rmtree(FIXTURE, ignore_errors=True)

    def test_single_file_path(self):
        out = _search(pattern="TARGET_VALUE", path=os.path.join(FIXTURE_REL, "alpha.py"))
        self.assertIn("alpha.py", out)
        self.assertNotIn("gamma.txt", out)
        self.assertIn("per-file limit 25", out)

    def test_missing_path_errors(self):
        out = _search(pattern="TARGET_VALUE", path=os.path.join(FIXTURE_REL, "nope.py"))
        self.assertTrue(out.startswith("Error: path not found"), out)

    def test_binary_skipped(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="*.bin")
        self.assertTrue(out.startswith("No matches found"), out)

    def test_ignore_case(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="beta.py", ignore_case=True)
        self.assertIn("beta.py", out)
        self.assertIn("Found 3 matches", out)

    def test_smart_case(self):
        out = _search(pattern="target_value", path=FIXTURE_REL, glob="alpha.py", smart_case=True)
        self.assertIn("Found 25 matches", out)
        out2 = _search(pattern="target_value", path=FIXTURE_REL, glob="alpha.py")
        self.assertTrue(out2.startswith("No matches found"), out2)

    def test_word(self):
        out = _search(pattern="TARGET", path=FIXTURE_REL, glob="*.py", word=True)
        self.assertTrue(out.startswith("No matches found"), out)
        out2 = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="gamma.txt", word=True)
        self.assertIn("Found 1 match", out2)

    def test_fixed_strings_skips_regex(self):
        out = _search(pattern="1 (", path=FIXTURE_REL, glob="*.py")
        self.assertTrue(out.startswith("Error: Invalid regex"), out)
        out2 = _search(pattern="1 (", path=FIXTURE_REL, glob="*.py", fixed_strings=True)
        self.assertTrue(out2.startswith("No matches found"), out2)

    def test_output_files(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="*.txt", output="files")
        self.assertIn("gamma.txt", out)
        self.assertNotIn("TARGET_VALUE", out)
        self.assertIn("Found 1 file containing matches", out)

    def test_output_count(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="*.py", output="count")
        self.assertIn("alpha.py: 40", out)
        self.assertIn("notes.test.py: 1", out)
        self.assertIn("Found 2 files, 41 matches", out)

    def test_context_lines(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="gamma.txt", context=1)
        self.assertIn("gamma.txt:1: TARGET_VALUE here", out)
        self.assertIn("gamma.txt-2- after line", out)

    def test_annotation_default(self):
        out = _search(pattern="TARGET_VALUE", path=os.path.join(FIXTURE_REL, "alpha.py"))
        self.assertIn("[def alpha_func]", out)

    def test_annotate_off(self):
        out = _search(pattern="TARGET_VALUE", path=os.path.join(FIXTURE_REL, "alpha.py"),
                      annotate=False)
        self.assertNotIn("[def alpha_func]", out)

    def test_snippet_function(self):
        out = _search(pattern="TARGET_VALUE", path=os.path.join(FIXTURE_REL, "alpha.py"),
                      snippet="function", max_matches=1)
        self.assertIn("alpha.py-2- def alpha_func():", out)
        self.assertIn("alpha.py:3: [def alpha_func]", out)
        self.assertNotIn("AlphaClass", out)

    def test_max_matches_and_offload(self):
        out = _search(pattern="TARGET_VALUE", path=os.path.join(FIXTURE_REL, "alpha.py"), max_matches=5)
        self.assertIn("showing first 5", out)
        self.assertIn("/scratch/large_tool_results/search_code_", out)
        pointer = out.split("full list: ", 1)[1].split(" ", 1)[0].rstrip(";— ").strip()
        from tools import read_file
        back = read_file(file_path=pointer)
        self.assertGreaterEqual(back.count("TARGET_VALUE"), 20, back[:200])

    def test_glob_exclude(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL, glob="*.py,!*.test.*")
        self.assertIn("alpha.py", out)
        self.assertNotIn("notes.test.py", out)

    def test_multi_pattern_single_pass(self):
        out = _search(patterns=["TARGET_VALUE", "import os"], path=FIXTURE_REL, glob="*.py")
        self.assertIn("files ranked for 2 patterns", out)
        self.assertIn("alpha.py", out)
        self.assertIn("patterns:2", out)

    def test_filename_mode(self):
        out = _search(pattern="gamma", path=FIXTURE_REL, mode="files")
        self.assertIn("gamma.txt", out)
        self.assertNotIn("TARGET_VALUE", out)

    def test_relative_paths(self):
        out = _search(pattern="TARGET_VALUE", path=FIXTURE_REL)
        self.assertIn("scratch/_search_test_tmp/alpha.py", out)
        self.assertNotIn(os.getcwd().replace("\\", "/"), out)


if __name__ == "__main__":
    unittest.main(verbosity=2)
