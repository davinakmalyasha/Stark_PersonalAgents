import sys
PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from it_department_graph import app_graph

thread_id = "unit-chat-1-1781364981"
config = {"configurable": {"thread_id": thread_id}}

history = list(app_graph.get_state_history(config))
# Find the checkpoint for Step 2
step2_state = None
for state in history:
    if state.metadata.get("step") == 2:
        step2_state = state
        break

if not step2_state:
    print("Step 2 not found.")
    sys.exit(0)

messages = step2_state.values.get("messages", []) if step2_state.values else []
print(f"Total messages in Step 2: {len(messages)}")

with open("scratch/turn2_content.txt", "w", encoding="utf-8") as f:
    for idx, m in enumerate(messages):
        f.write(f"\n================ [{idx}] {type(m).__name__} ================ \n")
        f.write(m.content)
        f.write("\n")
        if hasattr(m, 'tool_calls') and m.tool_calls:
            f.write(f"Tool calls: {m.tool_calls}\n")
