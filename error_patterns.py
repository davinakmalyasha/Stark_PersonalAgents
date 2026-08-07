"""error_patterns.py — verified failure→fix pattern library (harness-global).

Learns ONLY from objective evidence: a hidden test / verification FAILED, a
repair round fixed it, and the same verification then PASSED. The stored
"fix" is the actual diff of the repair round — deterministic, no LLM
summarization, no subagent judgment. Patterns are injected ONLY into the
SDET-FIX repair package (volatile, matched subset by error signature).

Gated by BSA_ERROR_PATTERNS=1 (learn + inject). The store lives at
.deep_agents/error_patterns.json (override with BSA_ERROR_PATTERNS_FILE for
isolated A/B runs). Never touches the stable zone / system prompts.
"""
import os
import json
import re
import threading

_DEFAULT_PATH = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    ".deep_agents", "error_patterns.json",
)
MAX_PATTERNS = 50
MAX_SNIPPET = 400
MAX_FIX = 600
MAX_MATCHES = 3
MIN_SIG_LEN = 20

_lock = threading.Lock()


def _store_path() -> str:
    return os.environ.get("BSA_ERROR_PATTERNS_FILE") or _DEFAULT_PATH


def _now() -> str:
    from datetime import datetime
    return datetime.utcnow().strftime("%Y-%m-%d %H:%M UTC")


def _normalize(text: str) -> str:
    """Lowercase, strip paths/timestamps/line numbers, collapse whitespace —
    so the same error across projects/tasks normalizes to the same signature."""
    t = (text or "").lower()
    t = re.sub(r"[a-z]:[\\/][^\s'\"`]+", " <path> ", t)          # windows paths
    t = re.sub(r"/[^\s'\"`]+/", " <path> ", t)                    # unix-ish paths
    t = re.sub(r"\b\d{4}-\d{2}-\d{2}[t ]\d{2}:\d{2}:\d{2}\b", " <ts> ", t)
    t = re.sub(r"line \d+", "line n", t)
    t = re.sub(r"\.\w+:\d+", ".<ext>:n", t)
    t = re.sub(r":\d+", ":n", t)
    t = re.sub(r"\s+", " ", t)
    return t.strip()[:2000]


def _read_store() -> list:
    try:
        p = _store_path()
        if not os.path.isfile(p):
            return []
        with open(p, "r", encoding="utf-8") as f:
            data = json.load(f)
        return data if isinstance(data, list) else []
    except Exception:
        return []


def _write_store(patterns: list) -> None:
    p = _store_path()
    try:
        os.makedirs(os.path.dirname(p), exist_ok=True)
    except Exception:
        pass
    tmp = p + ".tmp"
    with open(tmp, "w", encoding="utf-8") as f:
        json.dump(patterns, f, indent=1, ensure_ascii=False)
    os.replace(tmp, p)  # atomic — parallel eval workers never read a torn file


def learn(failure_detail: str, fix_diff: str, workspace: str = "", stack: str = "") -> bool:
    """Learn a verified pattern: real failure text -> the diff that made the
    verification pass. Dedupes on the normalized signature (bumps hits).
    Bounded at MAX_PATTERNS (evict lowest hits / oldest)."""
    if os.environ.get("BSA_ERROR_PATTERNS", "1") != "1":
        return False
    try:
        norm = _normalize(failure_detail)
        if not norm or len(norm) < MIN_SIG_LEN:
            return False
        fix = (fix_diff or "").strip()
        if len(fix) < 10:
            return False
        sig = norm[:150]
        with _lock:
            patterns = _read_store()
            for p in patterns:
                if p.get("norm") == norm[:200] or p.get("sig") == sig:
                    p["hits"] = p.get("hits", 0) + 1
                    p["last_used"] = _now()
                    _write_store(patterns)
                    return True
            patterns.append({
                "sig": sig,
                "norm": norm[:200],
                "snippet": (failure_detail or "").strip()[:MAX_SNIPPET],
                "fix": fix[:MAX_FIX],
                "hits": 1,
                "used": 0,
                "workspace": workspace,
                "stack": stack,
                "learned_at": _now(),
                "last_used": _now(),
            })
            if len(patterns) > MAX_PATTERNS:
                patterns.sort(key=lambda p: (p.get("hits", 0), p.get("learned_at", "")),
                              reverse=True)
                del patterns[MAX_PATTERNS:]
            _write_store(patterns)
            return True
    except Exception as e:
        print(f"[patterns] learn failed: {e}")
        return False


def match_patterns(failure_text: str, top_k: int = MAX_MATCHES) -> list:
    """Match a failure against the library by error signature. Strong match:
    the stored signature appears in the failure text; weak match: token-dice
    overlap >= 0.35. Returns top_k as {sig, snippet, fix} (read-only)."""
    if os.environ.get("BSA_ERROR_PATTERNS", "0") != "1":
        return []
    patterns = _read_store()
    if not patterns or not failure_text:
        return []
    norm = _normalize(failure_text)
    if len(norm) < MIN_SIG_LEN:
        return []
    scored = []
    for p in patterns:
        psig = p.get("sig", "")
        if not psig:
            continue
        strong = len(psig) >= MIN_SIG_LEN and psig in norm
        toks_a = set(_normalize(psig).split())
        toks_b = set(norm.split())
        dice = 0.0
        if toks_a and toks_b:
            dice = 2.0 * len(toks_a & toks_b) / (len(toks_a) + len(toks_b))
        if strong or dice >= 0.35:
            scored.append((1 if strong else 0, dice, p.get("hits", 0),
                           p.get("learned_at", ""), p))
    scored.sort(key=lambda x: (x[0], x[2], x[3]), reverse=True)
    return [{"sig": s[4].get("sig", ""),
             "snippet": s[4].get("snippet", ""),
             "fix": s[4].get("fix", "")}
            for s in scored[:top_k]]


def pattern_count() -> int:
    return len(_read_store())
