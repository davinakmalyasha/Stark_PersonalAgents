import requests, json
url = "http://127.0.0.1:8123/api/workspaces/_general/chats/chat-83672318f770/title"
resp = requests.put(url, json={"title": "davin_from_py"})
print(resp.status_code, resp.text)
import time; time.sleep(0.3)
resp2 = requests.get("http://127.0.0.1:8123/api/workspaces/_general/chats/chat-83672318f770")
print(resp2.json()["title"])
resp3 = requests.get("http://127.0.0.1:8123/api/workspaces/_general/chats")
for c in resp3.json()["chats"]:
    if c["id"] == "chat-83672318f770":
        print("list title:", c["title"])
        break
