import os
import sys

# Add LangChain dir to path
sys.path.append(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from tools import _validate_syntax, write_file, edit_file

# Create a temporary file with syntax error
test_py_file = "scratch/temp_invalid.py"
test_json_file = "scratch/temp_invalid.json"

print("=== Test 1: Validate Invalid Python ast parsing ===")
with open(test_py_file, "w") as f:
    f.write("def hello()\n    print('missing colon')")

err = _validate_syntax(test_py_file)
print("Result:", err)
assert "Python Syntax Error" in err
print("Test 1 Passed!")

print("\n=== Test 2: Validate Invalid JSON parsing ===")
with open(test_json_file, "w") as f:
    f.write("{invalid json: true}")

err = _validate_syntax(test_json_file)
print("Result:", err)
assert "JSON Parsing Error" in err
print("Test 2 Passed!")

print("\n=== Test 3: Validate write_file syntax warnings ===")
output = write_file("scratch/temp_invalid.py", "def broken():\n  return broken syntax")
print("write_file Output:", output)
assert "[WARNING]" in output
print("Test 3 Passed!")

# Cleanup
if os.path.exists(test_py_file):
    os.remove(test_py_file)
if os.path.exists(test_json_file):
    os.remove(test_json_file)

print("\nAll syntax checking tests passed successfully!")
