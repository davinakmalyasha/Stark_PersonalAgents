import os
import sys
PROJECT_ROOT = r"D:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from dotenv import load_dotenv
load_dotenv()
from langchain_openai import ChatOpenAI

key = os.getenv("DEEPSEEK_API_KEY")
client = ChatOpenAI(
    model="deepseek-v4-pro",
    api_key=key,
    base_url="https://api.deepseek.com",
    temperature=0.1,
    extra_body={"thinking": {"type": "enabled"}},
    reasoning_effort="medium"
)
try:
    print("Invoking model...")
    res = client.invoke("Solve: x^2 - 5x + 6 = 0")
    print("Content:", res.content)
    reasoning = res.additional_kwargs.get("reasoning_content")
    print("Reasoning exists:", bool(reasoning))
    if reasoning:
        print("Reasoning:", reasoning[:300])
except Exception as e:
    print("Error:", e)
