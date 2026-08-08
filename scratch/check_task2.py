import sys
import sqlite3
import json

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)
from it_department_graph import app_graph

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
cursor = conn.cursor()
cursor.execute("SELECT DISTINCT thread_id FROM checkpoints WHERE thread_id LIKE 'unit-chat-2-%';")
threads = sorted([r[0] for r in cursor.fetchall()])
conn.close()

if not threads:
    print("No unit-chat-2 threads found.")
    sys.exit(0)

# We want to check the first/oldest thread (which corresponds to the first execution of Task 2)
# or all of them.
for thread in threads:
    print(f"\n==========================================")
    print(f"THREAD: {thread}")
    print(f"==========================================")
    config = {"configurable": {"thread_id": thread}}
    try:
        history = list(app_graph.get_state_history(config))
    except Exception as e:
        print(f"Error getting history: {e}")
        continue
        
    print(f"Total history steps: {len(history)}")
    
    # Let's inspect the steps
    for s in reversed(history):
        metadata = s.metadata
        val = s.values
        step = metadata.get("step")
        node = metadata.get("node")
        print(f"\nStep: {step} | Node: {node}")
        
        # Check if there is an error_count or error_msg in val
        err_cnt = val.get("error_count")
        test_rep = val.get("test_report")
        if err_cnt:
            print(f"  error_count: {err_cnt}")
        if test_rep:
            print(f"  test_report snippet: {test_rep.strip().splitlines()[:10]}")
            
        messages = val.get("messages", [])
        print(f"  Messages: {len(messages)}")
        
        # Let's print the last few messages to see what went wrong
        for idx in range(max(0, len(messages)-5), len(messages)):
            m = messages[idx]
            role = type(m).__name__
            content_snippet = m.content[:150].replace('\n', ' ') if m.content else ""
            print(f"    [{idx}] {role}: {content_snippet}")
            if hasattr(m, 'tool_calls') and m.tool_calls:
                print(f"      Tool calls: {m.tool_calls}")
            if hasattr(m, 'additional_kwargs') and 'tool_calls' in m.additional_kwargs:
                print(f"      Addnl Tool calls: {m.additional_kwargs['tool_calls']}")

