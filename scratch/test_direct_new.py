import sys
sys.path.insert(0, r'D:\MyProject\LangChain')
from server.store.chat_store import get_chat, update_chat_title
chat_id = 'chat-57800a8e3dcc'
print("before get:", get_chat("", chat_id)["title"])
print("update:", update_chat_title("", chat_id, "Direct Renamed"))
print("after:", get_chat("", chat_id)["title"])
