import os
import sys
import threading
import queue
import traceback
from langchain_core.messages import HumanMessage, SystemMessage

sys.path.append(os.path.abspath("."))
from llm import invoke_messages_with_fallback

def run_in_thread(q):
    try:
        messages = [
            SystemMessage(content="You are a helpful assistant."),
            HumanMessage(content="Hello!")
        ]
        res = invoke_messages_with_fallback(
            role="Orchestrator",
            messages=messages,
            temp=0.2,
        )
        q.put(("success", res))
    except Exception as e:
        q.put(("error", traceback.format_exc()))

q = queue.Queue()
t = threading.Thread(target=run_in_thread, args=(q,), daemon=True)
t.start()
t.join()

status, val = q.get()
print(f"Status: {status}")
print(f"Result:\n{val}")
