"""Scans the files that would be pushed for hardcoded secrets. Masks values."""
import subprocess, re, os, sys

# Files introduced/changed by the new commits (what would be pushed)
out = subprocess.run(["git", "diff", "--name-only", "3fcb674..HEAD"],
                     capture_output=True, text=True, cwd=os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
files = [f for f in out.stdout.splitlines() if f.strip()]

PATTERNS = [
    ("OpenAI/Anthropic key",  r"\bsk-[A-Za-z0-9_-]{16,}"),
    ("Google API key",        r"\bAIza[0-9A-Za-z_-]{20,}"),
    ("AWS access key",        r"\bAKIA[0-9A-Z]{16}\b"),
    ("GitHub token",          r"\b(?:ghp_|gho_|github_pat_)[A-Za-z0-9_]{20,}"),
    ("Slack token",           r"\bxox[baprs]-[A-Za-z0-9-]{10,}"),
    ("Private key block",     r"-----BEGIN (?:RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----"),
    ("assign key/secret/pass", r"\b(?:api[_-]?key|apikey|password|passwd|secret|client[_-]?secret|auth_token|bearer)\b\s*[=:]\s*['\"][^'\"]{8,}"),
]

def mask(val: str, n: int = 6) -> str:
    v = val.strip("'\"")
    if len(v) <= n:
        return "***"
    return v[:n] + "***"

hits = 0
for f in files:
    if not os.path.isfile(f):
        continue
    try:
        with open(f, "r", encoding="utf-8", errors="replace") as fh:
            for i, line in enumerate(fh, 1):
                for label, pat in PATTERNS:
                    m = re.search(pat, line)
                    if m:
                        hits += 1
                        # mask the matched value
                        masked = re.sub(pat, lambda mm: mask(mm.group(0)), line.rstrip())
                        print(f"{f}:{i} [{label}] -> {masked[:120]}")
    except Exception:
        pass

print(f"\nSCAN DONE: {hits} potential secret(s) found.")
sys.exit(0)
