import os
import sys
PROJECT_ROOT = r"D:\MyProject\LangChain"
sys.path.append(PROJECT_ROOT)

from dotenv import load_dotenv
load_dotenv()
from langchain_openai import ChatOpenAI

key = os.getenv("DEEPSEEK_API_KEY")
client = ChatOpenAI(
    model="deepseek-v4.1-flash",
    api_key=key,
    base_url="https://api.deepseek.com",
    temperature=0.1,
    extra_body={"thinking": {"type": "enabled"}},
    reasoning_effort="high"
)
try:
    print("Calling invoke...")
    res = client.invoke("hello")
    print("Response succeeded!")
    with open("scratch/model_response.txt", "w", encoding="utf-8") as f:
        f.write("CONTENT:\n")
        f.write(str(res.content))
        f.write("\n\nREASONING:\n")
        reasoning = res.additional_kwargs.get("reasoning_content")
        f.write(str(reasoning))
    print("Results written to scratch/model_response.txt")
except Exception as e:
    print("Error:", e)
