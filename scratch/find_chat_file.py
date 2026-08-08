import os

search_dir = r"d:\MyProject\LangChain\.antigravity"
found = []
for root, dirs, files in os.walk(search_dir):
    for file in files:
        if "unit-chat-1" in file or "1781234562" in file:
            found.append(os.path.join(root, file))

print("Found files:", found)
