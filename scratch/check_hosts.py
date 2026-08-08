import os

hosts_path = r"C:\Windows\System32\drivers\etc\hosts"
if os.path.exists(hosts_path):
    print("Reading hosts file:")
    try:
        with open(hosts_path, "r", encoding="utf-8") as f:
            for line in f:
                if "deepseek" in line:
                    print("Found:", line.strip())
    except Exception as e:
        print("Error reading hosts file:", e)
else:
    print("Hosts file does not exist at standard path.")
