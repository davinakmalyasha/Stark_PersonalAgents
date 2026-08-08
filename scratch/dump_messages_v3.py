import sys
PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from it_department_graph import app_graph

thread_id = "unit-chat-1-1781364981"
config = {"configurable": {"thread_id": thread_id}}

state = app_graph.get_state(config)

messages = state.values.get("messages", []) if state.values else []

with open("scratch/messages_dump.txt", "w", encoding="utf-8") as f:
    f.write(f"Total messages in state: {len(messages)}\n")
    for idx, m in enumerate(messages):
        f.write(f"\n[{idx}] {type(m).__name__} (ID: {getattr(m, 'id', None)})\n")
        f.write(f"Content:\n{m.content}\n")
        if hasattr(m, 'tool_calls') and m.tool_calls:
            f.write(f"Tool calls: {m.tool_calls}\n")

print("Dumped messages to scratch/messages_dump.txt successfully.")
