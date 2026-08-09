import urllib.request
import os

print("Env proxies:")
for k, v in os.environ.items():
    if 'proxy' in k.lower() or 'url' in k.lower():
        print(f"  {k} = {v}")

print("\nTesting urllib connection to https://api.deepseek.com ...")
try:
    with urllib.request.urlopen("https://api.deepseek.com") as r:
        print("urllib success status:", r.status)
except Exception as e:
    import traceback
    print("urllib failed:")
    traceback.print_exc()

print("\nTesting httpx/requests connection if possible ...")
try:
    import httpx
    print("Using httpx...")
    r = httpx.get("https://api.deepseek.com")
    print("httpx success status:", r.status_code)
except Exception as e:
    import traceback
    print("httpx failed:")
    traceback.print_exc()
