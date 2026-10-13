import http.client, json
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
body = json.dumps({"title": "davin_from_py2"})
headers = {"Content-Type": "application/json"}
conn.request("PUT", "/api/workspaces/_general/chats/chat-83672318f770/title", body=body, headers=headers)
resp = conn.getresponse()
print(resp.status, resp.read().decode())
conn.close()
import http.client, json, time
time.sleep(0.3)
conn = http.client.HTTPConnection("127.0.0.1", 8123, timeout=5)
conn.request("GET", "/api/workspaces/_general/chats/chat-83672318f770")
resp = conn.getresponse()
print(resp.read().decode()[:2000])
