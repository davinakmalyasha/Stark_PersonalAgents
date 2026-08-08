import socket

try:
    ip = socket.gethostbyname("api.deepseek.com")
    print("api.deepseek.com resolves to:", ip)
except Exception as e:
    print("Failed to resolve:", e)
