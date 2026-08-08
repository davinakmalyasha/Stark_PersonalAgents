import os
import sys

sys.stdout.reconfigure(encoding='utf-8')

search_paths = [
    r"d:\MyProject\LangChain",
    r"D:\MyProject\TestProjectForAgent",
    r"C:\Users\MY LENOVO\.gemini\antigravity"
]

print("Searching for chat files...")
found = []
for p in search_paths:
    if not os.path.exists(p):
        continue
    for root, dirs, files in os.walk(p):
        for f in files:
            if "chat" in f or "task" in f or "call" in f:
                if f.endswith(".json") or f.endswith(".jsonl") or f.endswith(".db"):
                    path = os.path.join(root, f)
                    found.append((path, os.path.getsize(path)))

found.sort(key=lambda x: x[1], reverse=True)
print(f"Found {len(found)} files. Top 30 largest:")
for path, size in found[:30]:
    print(f"Size: {size:<10} | Path: {path}")
