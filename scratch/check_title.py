import sqlite3
conn = sqlite3.connect(r'D:\MyProject\LangChain\.deep_agents\chats.db')
cur = conn.execute("SELECT title, updated_at FROM chats WHERE id='chat-83672318f770'")
print(cur.fetchone())
cur = conn.execute("SELECT sql FROM sqlite_master WHERE type='table' AND name='chats'")
print(cur.fetchone())
