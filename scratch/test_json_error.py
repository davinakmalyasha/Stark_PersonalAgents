import re
import json

def fix_json_backslashes(text: str) -> str:
    pattern = re.compile(r'\\\\|\\"|\\/|\\b|\\f|\\n|\\r|\\t|\\u[0-9a-fA-F]{4}|\\')
    def replace(match):
        val = match.group(0)
        if val == '\\':
            return '\\\\'
        return val
    return pattern.sub(replace, text)

# Test string with already escaped backslashes (D:\\MyProject) and unescaped namespace (Laravel\Sanctum)
raw_response = """{
  "global_segment": {
    "updates": {
      "client_name": "Davin Akmal Yasha",
      "project_path": "D:\\\\MyProject\\\\TestProjectForAgent"
    },
    "deletions": []
  },
  "security_segment": {
    "namespace": "Laravel\\Sanctum\\HasApiTokens"
  }
}"""

print("Original:")
print(raw_response)

fixed_text = fix_json_backslashes(raw_response)
print("\nAfter fix:")
print(fixed_text)

try:
    json.loads(fixed_text)
    print("\nParsed successfully!")
except Exception as e:
    print("\nFailed to parse:", e)
