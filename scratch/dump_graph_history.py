import sys
PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from it_department_graph import app_graph

thread_id = "unit-chat-2-1781365361"
config = {"configurable": {"thread_id": thread_id}}

history = list(app_graph.get_state_history(config))
print(f"Total history steps: {len(history)}")

for idx, state in enumerate(history):
    metadata = state.metadata
    values = state.values
    step = metadata.get("step")
    source = metadata.get("source")
    nxt = state.next
    next_agent = values.get("next_agent") if values else None
    remaining_steps = values.get("remaining_steps") if values else None
    print(f"Index {idx:2d} | Step {step:2d} | Source {source:<8} | Next Node {nxt} | next_agent {next_agent} | remaining_steps {remaining_steps}")
