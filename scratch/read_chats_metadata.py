import os
import json

chat_dir = r"d:\MyProject\LangChain\.antigravity\chats\ws-1d8c26da"
for file in os.listdir(chat_dir):
    if file.endswith(".json") and not file.endswith("_traces.json") and not file.endswith("_usage.json"):
        path = os.path.join(chat_dir, file)
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            messages = data.get("messages", [])
            print(f"\n=== File: {file} | Messages: {len(messages)} ===")
            for m in messages[:3]:
                print(f"  [{m.get('sender') or m.get('role')}]: {repr(m.get('content') or m.get('text'))[:150]}")
        except Exception as e:
            print(f"Error reading {file}: {e}")
