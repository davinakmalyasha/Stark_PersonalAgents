import sys
import sqlite3
from langchain_core.messages import AIMessage, HumanMessage, SystemMessage

PROJECT_ROOT = r"d:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from compressed_checkpointer import CompressedSqliteSaver

db_path = r"d:\MyProject\LangChain\.deep_agents\checkpoints.db"
conn = sqlite3.connect(db_path)
saver = CompressedSqliteSaver(conn)

# Let's inspect the latest checkpoints for unit-chat-1-1781364981
thread_id = "unit-chat-1-1781364981"
print(f"Loading history for thread: {thread_id}")

config = {"configurable": {"thread_id": thread_id, "checkpoint_ns": ""}}
history = list(saver.list(config))

print(f"Found {len(history)} checkpoints in history.")

for idx, tup in enumerate(history):
    checkpoint = tup.checkpoint
    checkpoint_id = tup.config["configurable"]["checkpoint_id"]
    metadata = tup.metadata
    print(f"\n==========================================")
    print(f"Index: {idx} | Checkpoint ID: {checkpoint_id} | Metadata: {metadata}")
    if checkpoint and "channel_values" in checkpoint:
        messages = checkpoint["channel_values"].get("messages", [])
        print(f"Number of messages: {len(messages)}")
        for m_idx, m in enumerate(messages):
            print(f"  [{m_idx}] {type(m).__name__}: {str(m.content)[:300]}")
            if hasattr(m, 'tool_calls') and m.tool_calls:
                print(f"    Tool calls: {m.tool_calls}")

conn.close()
