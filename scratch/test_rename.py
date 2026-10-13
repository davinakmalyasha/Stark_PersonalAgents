import os, sys
sys.path.insert(0, r'D:\MyProject\LangChain')
from server.store.chat_store import update_chat_title, get_chat

chat_id = 'chat-83672318f770'
ws_path = ''  # _general virtual path
print("before get:", get_chat(ws_path, chat_id))
print("update result:", update_chat_title(ws_path, chat_id, "davin_test2"))
print("after get:", get_chat(ws_path, chat_id))

import sqlite3
conn = sqlite3.connect(r'D:\MyProject\LangChain\.deep_agents\chats.db')
cur = conn.execute("SELECT title, updated_at FROM chats WHERE id=?", (chat_id,))
print("db after:", cur.fetchone())
