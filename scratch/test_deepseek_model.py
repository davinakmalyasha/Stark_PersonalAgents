import os
import sys
PROJECT_ROOT = r"D:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from dotenv import load_dotenv
load_dotenv()
from langchain_openai import ChatOpenAI

key = os.getenv("DEEPSEEK_API_KEY")
client = ChatOpenAI(
    model="deepseek-v4-flash",
    api_key=key,
    base_url="https://api.deepseek.com",
    temperature=0.1,
)
try:
    res = client.invoke("hello")
    with open("scratch/model_response.txt", "w", encoding="utf-8") as f:
        f.write(res.content)
    print("Response written to scratch/model_response.txt")
except Exception as e:
    print("Error:", e)
