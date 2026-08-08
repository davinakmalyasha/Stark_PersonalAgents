import os

def read_and_print(filename):
    if not os.path.exists(filename):
        print(f"{filename} does not exist")
        return
    print(f"\n--- {filename} (last 500 lines) ---")
    try:
        with open(filename, "r", encoding="utf-16") as f:
            lines = f.readlines()
        for line in lines[-500:]:
            print(line.strip())
    except Exception as e:
        print(f"Failed to read {filename} in UTF-16: {e}")

read_and_print("api_stdout.log")
