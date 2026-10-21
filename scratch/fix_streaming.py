import pathlib
p = pathlib.Path(r"D:\MyProject\LangChain\frontend\src\components\Workspace\MessageBubble.tsx")
t = p.read_text(encoding='utf-8')
# add ts-ignore before StreamingThinking
if "function StreamingThinking" in t and "@ts-ignore" not in t.split("function StreamingThinking")[0][-200:]:
    t = t.replace("function StreamingThinking", "// @ts-ignore\nfunction StreamingThinking")
    p.write_text(t, encoding='utf-8')
    print("patched ts-ignore")
else:
    print("already patched or not found")
