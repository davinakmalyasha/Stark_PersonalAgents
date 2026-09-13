import os
import sys
import traceback
from dotenv import load_dotenv
from langchain_openai import ChatOpenAI
from langchain_core.messages import HumanMessage, SystemMessage

load_dotenv()
print("DEEPSEEK_API_KEY set:", bool(os.getenv("DEEPSEEK_API_KEY")))

try:
    llm = ChatOpenAI(
        model="deepseek-v4.1-flash",
        api_key=os.getenv("DEEPSEEK_API_KEY"),
        base_url="https://api.deepseek.com",
        temperature=0.1,
    )
    messages = [
        SystemMessage(content="You are a helpful assistant."),
        HumanMessage(content="Hello!")
    ]
    print("Calling llm.invoke directly...")
    res = llm.invoke(messages)
    print("Success:", res)
except Exception as e:
    print("Raw exception occurred:")
    traceback.print_exc()
