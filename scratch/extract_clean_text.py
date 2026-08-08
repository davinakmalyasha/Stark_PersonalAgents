import html
from html.parser import HTMLParser
import re

class TextExtractor(HTMLParser):
    def __init__(self):
        super().__init__()
        self.result = []
        self.ignore = False

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style", "head", "nav", "footer", "button"):
            self.ignore = True

    def handle_endtag(self, tag):
        if tag in ("script", "style", "head", "nav", "footer", "button"):
            self.ignore = False

    def handle_data(self, data):
        if not self.ignore:
            self.result.append(data)

with open(r"C:\Users\MY LENOVO\AppData\Local\Temp\deep_agents_blog.html", "w") as f:
    pass # we'll use the already fetched content

import os
html_path = r"C:\Users\MY LENOVO\.gemini\antigravity\brain\bd43a71b-6ed0-4b72-a997-37e21598ab13\.system_generated\steps\887\content.md"

if os.path.exists(html_path):
    with open(html_path, "r", encoding="utf-8") as f:
        html_content = f.read()
    
    parser = TextExtractor()
    parser.feed(html_content)
    text = "".join(parser.result)
    
    # Clean up empty lines
    lines = [line.strip() for line in text.split("\n")]
    clean_lines = [line for line in lines if line]
    
    clean_text = "\n".join(clean_lines)
    
    out_path = r"d:\MyProject\LangChain\scratch\clean_blog.txt"
    with open(out_path, "w", encoding="utf-8") as out_f:
        out_f.write(clean_text)
    print("Successfully wrote clean text to", out_path)
else:
    print("Html file not found at", html_path)
