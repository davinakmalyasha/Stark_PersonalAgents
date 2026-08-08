import requests

r = requests.post(
    'http://127.0.0.1:8000/api/run',
    json={'prompt': 'what project is this?', 'model': 'Automatic Fallback', 'temperature': 0.7, 'workspace_path': r'd:\MyProject\LangChain', 'chat_id': 'chat-05e6cd372997'},
    timeout=30,
)
print("=== Status ===")
print(r.status_code)
print("=== Headers ===")
print(r.headers)
print("=== Raw Response ===")
print(r.text)
