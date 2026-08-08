import os
import json
import sys

sys.stdout.reconfigure(encoding='utf-8')

chat_dir = r"d:\MyProject\LangChain\.antigravity\chats\ws-1d8c26da"
all_agent_messages = []

for file in os.listdir(chat_dir):
    if file.endswith(".json"):
        path = os.path.join(chat_dir, file)
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            messages = data.get("messages", [])
            for m in messages:
                role = m.get("role") or m.get("sender") or ""
                content = m.get("content") or m.get("text") or ""
                if role in ("agent", "assistant", "AIMessage") and content:
                    all_agent_messages.append({
                        "file": file,
                        "length": len(content),
                        "content": content,
                        "metadata": m.get("metadata", {})
                    })
        except Exception:
            pass

# Sort by length descending
all_agent_messages.sort(key=lambda x: x["length"], reverse=True)

print(f"Top 10 largest agent outputs found in {chat_dir}:")
print(f"{'Rank':<4} | {'File':<25} | {'Length':<8} | {'Type':<12} | {'Preview'}")
print("-" * 90)

for idx, m in enumerate(all_agent_messages[:10]):
    is_trace = m["metadata"].get("isTrace", False)
    m_type = "Trace (Tool)" if is_trace else "Chat message"
    preview = m["content"][:60].replace('\n', ' ')
    print(f"{idx+1:<4} | {m['file']:<25} | {m['length']:<8} | {m_type:<12} | {preview}")

if all_agent_messages:
    print("\n--- DETAILED VIEW OF THE LARGEST OUTPUT ---")
    largest = all_agent_messages[0]
    print(f"File: {largest['file']}")
    print(f"Length: {largest['length']} characters")
    print(f"Content:\n{largest['content'][:1000]}")
    print("...\n[TRUNCATED]")
