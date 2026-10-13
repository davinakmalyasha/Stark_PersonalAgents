import http.client, json
# create
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
body = json.dumps({"title": "To Delete", "model": "Automatic Fallback"})
headers = {"Content-Type": "application/json"}
conn.request("POST", "/api/workspaces/_general/chats", body=body, headers=headers)
resp = conn.getresponse()
data = json.loads(resp.read().decode())
chat_id = data["id"]
print("created", chat_id)
conn.close()
# delete
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
conn.request("DELETE", f"/api/workspaces/_general/chats/{chat_id}")
resp = conn.getresponse()
print("delete", resp.status, resp.read().decode())
conn.close()
# list
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
conn.request("GET", "/api/workspaces/_general/chats")
resp = conn.getresponse()
j = json.loads(resp.read().decode())
exists = any(c["id"] == chat_id for c in j["chats"])
print("still exists?", exists)
conn.close()
