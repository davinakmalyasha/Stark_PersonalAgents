import requests

print("=== Test: prompt 'hy' ===")
try:
    r = requests.post(
        'http://127.0.0.1:8000/api/run',
        json={'prompt': 'hy', 'model': 'Automatic Fallback', 'temperature': 0.7, 'workspace_path': r'd:\MyProject\LangChain'},
        stream=True,
        timeout=60,
    )
    print(f"Status: {r.status_code}")
    for line in r.iter_lines():
        if line:
            print("EVENT:", line.decode('utf-8'))
except Exception as e:
    print("Error:", e)
