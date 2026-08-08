import os
import json
import sys

sys.stdout.reconfigure(encoding='utf-8')

chats_base_dir = r"d:\MyProject\LangChain\.antigravity\chats"
all_agent_messages = []

if os.path.exists(chats_base_dir):
    for ws_dir in os.listdir(chats_base_dir):
        ws_path = os.path.join(chats_base_dir, ws_dir)
        if os.path.isdir(ws_path):
            for file in os.listdir(ws_path):
                if file.endswith(".json") and not file.endswith("_usage.json") and not file.endswith("_traces.json"):
                    path = os.path.join(ws_path, file)
                    try:
                        with open(path, "r", encoding="utf-8") as f:
                            data = json.load(f)
                        messages = data.get("messages", [])
                        for m in messages:
                            role = m.get("role") or m.get("sender") or ""
                            content = m.get("content") or m.get("text") or ""
                            # Sometimes the role is 'assistant' or 'agent' or 'AIMessage'
                            if role in ("agent", "assistant", "AIMessage", "developer", "Developer", "DeveloperFixing") and content:
                                all_agent_messages.append({
                                    "workspace": ws_dir,
                                    "file": file,
                                    "length": len(content),
                                    "content": content,
                                    "role": role,
                                    "metadata": m.get("metadata", {})
                                })
                    except Exception as e:
                        print(f"Error reading {path}: {e}")

# Sort by length descending
all_agent_messages.sort(key=lambda x: x["length"], reverse=True)

print(f"Total agent messages found: {len(all_agent_messages)}")
print(f"Top 15 largest agent outputs:")
print(f"{'Rank':<4} | {'Workspace':<12} | {'File':<25} | {'Role':<15} | {'Length':<8} | {'Preview'}")
print("-" * 110)

for idx, m in enumerate(all_agent_messages[:15]):
    preview = m["content"][:50].replace('\n', ' ')
    print(f"{idx+1:<4} | {m['workspace']:<12} | {m['file']:<25} | {m['role']:<15} | {m['length']:<8} | {preview}")

for idx in range(min(5, len(all_agent_messages))):
    print(f"\n--- DETAILED VIEW OF LARGEST OUTPUT RANK {idx+1} ---")
    m = all_agent_messages[idx]
    print(f"Workspace: {m['workspace']} | File: {m['file']} | Role: {m['role']}")
    print(f"Length: {m['length']} characters")
    print(f"Content snippet (first 1500 chars):\n{m['content'][:1500]}")
    if len(m['content']) > 1500:
        print(f"\n... [TRUNCATED] ...\n")
        print(f"Content snippet (last 1000 chars):\n{m['content'][-1000:]}")
    print("=" * 80)
