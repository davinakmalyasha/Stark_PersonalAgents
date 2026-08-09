import os
import sys
import traceback
from langchain_core.messages import HumanMessage, SystemMessage

# Add current directory to path
sys.path.append(os.path.abspath("."))

from llm import invoke_messages_with_fallback

print("DEEPSEEK_API_KEY set:", bool(os.getenv("DEEPSEEK_API_KEY")))
# Note: we won't print the API key itself for security reasons

try:
    messages = [
        SystemMessage(content="You are a helpful assistant."),
        HumanMessage(content="Hello!")
    ]
    print("Calling invoke_messages_with_fallback...")
    res = invoke_messages_with_fallback(
        role="Orchestrator",
        messages=messages,
        temp=0.2,
    )
    print("Success response:", res)
except Exception as e:
    print("Error occurred:")
    traceback.print_exc()
