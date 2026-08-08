import sys
import sqlite3
PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from it_department_graph import app_graph

thread_id = "unit-chat-1-1781364981"
config = {"configurable": {"thread_id": thread_id}}

history = list(app_graph.get_state_history(config))
print(f"Total history steps: {len(history)}")

for idx, state in enumerate(history):
    metadata = state.metadata
    values = state.values
    step = metadata.get("step")
    node = metadata.get("node")
    messages = values.get("messages", []) if values else []
    print(f"Index {idx} | Step {step} | Node {node} | Messages: {len(messages)}")
    if messages:
        # Print the last message role and preview
        last_m = messages[-1]
        print(f"  Last message: {type(last_m).__name__} : {str(last_m.content)[:100].replace('\n', ' ')}")
