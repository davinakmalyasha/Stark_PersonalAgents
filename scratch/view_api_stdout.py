import sys
sys.stdout.reconfigure(encoding='utf-8')

with open(r"d:\MyProject\LangChain\api_stdout.log", "r", encoding="utf-16le") as f:
    lines = f.readlines()
print("Total lines in api_stdout.log:", len(lines))
# Print the last 200 lines
for line in lines[-200:]:
    print(line, end="")
