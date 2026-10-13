import sys
sys.path.insert(0, r'D:\MyProject\LangChain')
from server.store.workspace_registry import get_workspace
from server.store.chat_store import update_chat_title, get_chat

ws = get_workspace("_general")
print("ws:", ws)
print("path:", repr(ws["path"]))
chat_id = "chat-83672318f770"
print("before:", get_chat(ws["path"], chat_id)["title"])
print("update:", update_chat_title(ws["path"], chat_id, "davin_api2"))
print("after:", get_chat(ws["path"], chat_id)["title"])

# try via _general string directly
print("--- direct '' ---")
print(update_chat_title("", chat_id, "davin_api3"))
print(get_chat("", chat_id)["title"])
