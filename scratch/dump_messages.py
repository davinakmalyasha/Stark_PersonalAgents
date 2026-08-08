import sys
import sqlite3
import json
from langchain_core.messages import AIMessage, HumanMessage, SystemMessage

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)
from it_department_graph import app_graph

db_path = r"d:\MyProject\LangChain\.antigravity\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-1-%';")
threads = [r[0] for r in cursor.fetchall()]
conn.close()

if not threads:
    print("No threads found.")
    sys.exit(0)

latest_thread = sorted(threads)[-1]
print(f"Latest thread: {latest_thread}")
config = {"configurable": {"thread_id": latest_thread}}
history = list(app_graph.get_state_history(config))

for s in reversed(history):
    metadata = s.metadata
    val = s.values
    step = metadata.get("step")
    node = metadata.get("node")
    print(f"\n==========================================")
    print(f"Step: {step} | Node: {node}")
    
    # Dump messages if present in channel_values
    messages = val.get("messages", [])
    print(f"Number of messages: {len(messages)}")
    for idx, m in enumerate(messages):
        role = type(m).__name__
        content_snippet = m.content[:150].replace('\n', ' ') if m.content else ""
        print(f"  [{idx}] {role}: {content_snippet}")
        # Print tool calls if AIMessage
        if hasattr(m, 'additional_kwargs') and 'tool_calls' in m.additional_kwargs:
            print(f"    Tool calls: {m.additional_kwargs['tool_calls']}")
        if hasattr(m, 'tool_calls') and m.tool_calls:
            print(f"    Tool calls (lc): {m.tool_calls}")
