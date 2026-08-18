import sys
import os
import tiktoken

def main():
    base_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    sys.path.append(base_dir)
    try:
        from agents.developer_prompt import _STATIC_SYSTEM_TEMPLATE
        enc = tiktoken.get_encoding("cl100k_base")
        tokens = enc.encode(_STATIC_SYSTEM_TEMPLATE)
        print(f"Token count: {len(tokens)}")
        print(f"Character count: {len(_STATIC_SYSTEM_TEMPLATE)}")
    except Exception as e:
        print(f"Error: {e}")

if __name__ == "__main__":
    main()
