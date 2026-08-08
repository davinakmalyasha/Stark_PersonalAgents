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
    reasoning_effort="max"
)
try:
    print("Calling invoke with reasoning_effort='max'...")
    res = client.invoke("hello")
    print("Response succeeded!")
except Exception as e:
    print("Error:", e)
