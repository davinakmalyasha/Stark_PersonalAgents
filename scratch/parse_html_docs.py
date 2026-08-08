import os
from html.parser import HTMLParser

class HTMLTextExtractor(HTMLParser):
    def __init__(self):
        super().__init__()
        self.text_parts = []
        self.in_script_or_style = False

    def handle_starttag(self, tag, attrs):
        if tag in ("script", "style"):
            self.in_script_or_style = True

    def handle_endtag(self, tag):
        if tag in ("script", "style"):
            self.in_script_or_style = False

    def handle_data(self, data):
        if not self.in_script_or_style:
            self.text_parts.append(data)

    def get_text(self):
        return "".join(self.text_parts)

def clean_html_file(filepath, output_path):
    if not os.path.exists(filepath):
        print(f"File not found: {filepath}")
        return
    
    with open(filepath, "r", encoding="utf-8") as f:
        content = f.read()
        
    parser = HTMLTextExtractor()
    parser.feed(content)
    text = parser.get_text()
    
    # Clean up whitespace
    lines = (line.strip() for line in text.splitlines())
    chunks = (phrase.strip() for line in lines for phrase in line.split("  "))
    clean_text = "\n".join(chunk for chunk in chunks if chunk)
    
    with open(output_path, "w", encoding="utf-8") as f:
        f.write(clean_text)
    print(f"Cleaned {filepath} -> {output_path}")

steps_dir = r"C:\Users\MY LENOVO\.gemini\antigravity\brain\e18bc1ec-e60e-4103-8cd1-f1137d2bae58\.system_generated\steps"
clean_html_file(os.path.join(steps_dir, "739", "content.md"), "scratch/deepagents_clean.txt")
clean_html_file(os.path.join(steps_dir, "741", "content.md"), "scratch/langchain_clean.txt")
clean_html_file(os.path.join(steps_dir, "743", "content.md"), "scratch/langgraph_clean.txt")
clean_html_file(os.path.join(steps_dir, "765", "content.md"), "scratch/deepagents_memory_clean.txt")
clean_html_file(os.path.join(steps_dir, "767", "content.md"), "scratch/deepagents_tools_clean.txt")
clean_html_file(os.path.join(steps_dir, "769", "content.md"), "scratch/deepagents_context_clean.txt")
clean_html_file(os.path.join(steps_dir, "771", "content.md"), "scratch/deepagents_backends_clean.txt")
clean_html_file(os.path.join(steps_dir, "773", "content.md"), "scratch/deepagents_subagents_clean.txt")
