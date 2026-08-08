import os

def read_log(path, num_lines=150):
    if not os.path.exists(path):
        print(f"File {path} does not exist.")
        return
    print(f"\n=== LOG: {path} ===")
    for encoding in ['utf-8', 'utf-16', 'utf-16-le', 'cp1252', 'latin1']:
        try:
            with open(path, 'r', encoding=encoding) as f:
                lines = f.readlines()
            print(f"Successfully read with encoding: {encoding}")
            for line in lines[-num_lines:]:
                print(line, end='')
            return
        except Exception as e:
            pass
    print(f"Failed to read {path} with any common encoding.")

read_log(r"d:\MyProject\LangChain\api_stdout.log")
read_log(r"d:\MyProject\LangChain\api_stderr.log")
