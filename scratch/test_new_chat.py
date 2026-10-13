import http.client, json, time
# create new chat
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
body = json.dumps({"title": "Temp Chat", "model": "Automatic Fallback"})
headers = {"Content-Type": "application/json"}
conn.request("POST", "/api/workspaces/_general/chats", body=body, headers=headers)
resp = conn.getresponse()
data = json.loads(resp.read().decode())
print("create:", data)
chat_id = data["id"]
conn.close()
time.sleep(0.2)
# rename it
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
body = json.dumps({"title": "Renamed Title"})
conn.request("PUT", f"/api/workspaces/_general/chats/{chat_id}/title", body=body, headers=headers)
resp = conn.getresponse()
print("rename status:", resp.status, resp.read().decode())
conn.close()
time.sleep(0.3)
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
conn.request("GET", f"/api/workspaces/_general/chats/{chat_id}")
resp = conn.getresponse()
print("get after rename:", json.loads(resp.read().decode())["title"])
conn.close()
# list
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
conn.request("GET", "/api/workspaces/_general/chats")
resp = conn.getresponse()
j = json.loads(resp.read().decode())
for c in j["chats"]:
    if c["id"] == chat_id:
        print("list title:", c["title"])
        break
conn.close()
