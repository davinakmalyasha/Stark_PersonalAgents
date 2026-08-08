import os
from dotenv import load_dotenv
load_dotenv()

base_url = os.getenv("DEEPSEEK_BASE_URL")
api_key = os.getenv("DEEPSEEK_API_KEY")

print("DEEPSEEK_BASE_URL in env:", base_url)
if api_key:
    # Print key details securely (only prefix/suffix/length)
    print(f"DEEPSEEK_API_KEY: Length={len(api_key)}, Starts with={api_key[:8]}..., Ends with=...{api_key[-8:] if len(api_key) > 8 else ''}")
else:
    print("DEEPSEEK_API_KEY is not set!")
