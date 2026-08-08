import re

with open("D:/MyProject/LangChain/developer_agent.py", "r", encoding="utf-8") as f:
    content = f.read()

print(f"File size: {len(content)} bytes")

match = re.search(r"_STATIC_SYSTEM_TEMPLATE\s*=\s*r?\"\"\"", content)
if match:
    quote_start = match.end()
    close = content.find("\"\"\"", quote_start)
    template = content[quote_start:close]
    print(f"Template chars: {len(template)}")
    print(f"Est tokens (~4 char/tok): {len(template)//4}")
    print(f"Est tokens (~3.85 char/tok, Claude): {len(template)//3.85:.0f}")
    print()
    print("--- Skills found in template ---")
    skills = re.findall(r"^###\s+(.+)$", template, re.MULTILINE)
    for s in skills:
        print(f"  - {s}")
else:
    print("Could not find template start")
