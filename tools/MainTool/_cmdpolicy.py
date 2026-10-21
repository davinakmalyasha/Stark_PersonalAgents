"""MainTool._cmdpolicy — one place for shell command parsing and safety policy.

Used by run_command (destructive-command veto) and plan_mode_guard (read-only
enforcement). Commands are segmented on unquoted ``; && || | &`` and newline,
then each segment is tokenized (shlex, tolerant fallback) so rules match the
actual program + arguments instead of raw substrings. That fixes both classes
of false judgment in the old deny list:

  * false positives — ``rm -rf ./node_modules`` or ``echo "git push"`` no
    longer trip the policy,
  * bypasses — ``git  push``, ``git "push"``, ``rm -rf "$HOME"`` now parse to
    the same tokens and are blocked.

Unparseable input degrades to the conservative substring list, never to allow.
"""
from __future__ import annotations

import os
import re
import shlex
from dataclasses import dataclass, field

_MAX_PARSE_CHARS = 20000
_MAX_SUBST_DEPTH = 3

#: Patterns searched as raw substrings in every command (unbounded constructs
#: that cannot be caught by token rules, kept for backward compatibility).
GLOBAL_DENY_SUBSTRINGS = {
    ":(){",        # fork bomb
    "dd if=",      # raw device reads
    "mkfs",        # filesystem creation
}

#: Substring patterns applied only when the command reaches a nested shell
#: (powershell / cmd / another bash), where token rules do not apply.
SUBSHELL_DENY_SUBSTRINGS = {
    "rm -rf /", "rm -rf ~", "del /s /q c:\\", "format c:",
    "shutdown", "reg delete", "reg add", "regedit",
    "rd /s /q", "rmdir /s /q", "del /f /q /s",
    "remove-item -recurse -force .", "remove-item -recurse -force *",
    "remove-item -recurse -force c:", "remove-item -recurse -force d:",
    "net user", "net localgroup", "diskpart",
    "chkdsk /f", "sfc /scannow", "format d:",
}

#: Backward-compatible aggregate (runCommand re-exports this name).
BLOCKED_PATTERNS = GLOBAL_DENY_SUBSTRINGS | SUBSHELL_DENY_SUBSTRINGS

_WRAPPER_PROGRAMS = {"env", "nohup", "time", "command", "exec", "sudo", "builtin", "nice"}
_SUBSHELL_PROGRAMS = {"bash", "sh", "zsh", "dash", "powershell", "pwsh", "cmd"}

#: Strict allowlist for plan mode. Anything not proven read-only is vetoed.
_READ_ONLY_PROGRAMS = {
    "ls", "dir", "cat", "head", "tail", "wc", "find", "grep", "rg",
    "which", "where", "echo", "printf", "pwd", "env", "printenv",
    "date", "whoami", "id", "hostname", "uname", "stat", "file",
    "tree", "du", "df", "sort", "uniq", "cut", "diff", "cmp",
    "md5sum", "sha256sum", "true", "false",
}
_GIT_READ_SUBCMDS = {
    "status", "log", "diff", "show", "branch", "remote", "ls-files",
    "rev-parse", "describe", "blame", "shortlog", "reflog", "config",
}
_GIT_VALUE_FLAGS = {"-C", "-c", "--git-dir", "--work-tree", "--namespace",
                    "--exec-path", "--config-env"}
_NPM_VALUE_FLAGS = {"--prefix", "--workspace", "-w", "--filter", "-F", "--cwd", "--dir"}
_PY_WRITE_HINTS = re.compile(
    r"(open\s*\([^)]*['\"][wax]|\.write\s*\(|\.writelines\s*\(|os\.remove|"
    r"os\.unlink|os\.rmdir|os\.mkdir|os\.makedirs|os\.rename|os\.replace|"
    r"shutil\.|rmtree|subprocess|os\.system|os\.popen|pathlib[^\n]*write|"
    r"savefig|to_csv|to_excel|\.unlink\s*\(|\.rename\s*\(|chmod|chown)",
    re.IGNORECASE,
)

_SERVER_PROGRAMS = {
    "vite", "nodemon", "uvicorn", "gunicorn", "http-server", "serve",
    "webpack-dev-server", "webpack", "next", "nuxt", "astro", "ng",
    "parcel", "gatsby", "hugo", "jekyll", "flask", "streamlit",
}
_SERVER_SUBCOMMANDS = {"dev", "start", "serve", "develop", "runserver"}
_CRED_PROGRAMS_RE = re.compile(
    r"(?i)(?:^|[\s;&|/\\])(gh|git|curl|wget|npm|pnpm|yarn|bun|pip[0-9]*|poetry|uv|"
    r"huggingface-cli|hf|wrangler|vercel|netlify|aws|az|gcloud)(?:\.exe)?(?=[\s;&|]|$)"
)

#: Exit 1 from these programs is a normal negative answer (no match/differences),
#: not a failure. Mirrors Claude Code's whitelist so VERIFY-GATE and telemetry
#: do not flag `grep`/`find`/`diff` negative results as errors.
_NEGATIVE_EXIT_LABELS = {
    "grep": "no match", "egrep": "no match", "fgrep": "no match",
    "rg": "no match", "find": "no match", "findstr": "no match",
    "diff": "differences", "cmp": "differences",
    "test": "condition false", "[": "condition false",
}
_NEGATIVE_GIT_SUBCMDS = {"diff": "differences", "grep": "no match"}

#: Commands that read file contents; used by the secret-file guard.
_READ_FILE_PROGRAMS = {
    "cat", "head", "tail", "less", "more", "strings", "xxd", "od", "base64",
    "cp", "scp", "rsync", "tar", "zip", "unzip", "gpg", "openssl", "sqlite3",
}
_GREP_LIKE_PROGRAMS = {"grep", "egrep", "fgrep", "rg", "findstr"}
_SECRET_PATH_FRAGMENTS = ("/.ssh/", "/.aws/", "/.config/gh/", "/appdata/roaming/gh/")


@dataclass
class PolicyResult:
    """Outcome of a policy check. ``allowed=False`` carries the reason."""

    allowed: bool = True
    rule: str = ""
    reason: str = ""
    roots: list = field(default_factory=list)
    has_redirection: bool = False
    parse_ok: bool = True


def _extra_deny_patterns() -> set:
    """CMD_DENY_EXTRA: comma-separated extra substrings (operator escape hatch)."""
    extra = os.environ.get("CMD_DENY_EXTRA", "")
    if not extra:
        return set()
    return {p.strip().lower() for p in extra.split(",") if p.strip()}


def _basename(prog: str) -> str:
    p = str(prog or "").replace("\\", "/").rstrip("/")
    p = p.split("/")[-1] if p else str(prog or "")
    p = p.lower()
    if p.endswith(".exe"):
        p = p[:-4]
    return p


def _split_segments(command: str):
    """Split on unquoted separators. Returns (segments, has_redirection, parse_ok)."""
    segments: list = []
    current: list = []
    quote = None
    has_redirection = False
    parse_ok = True
    i = 0
    n = len(command)
    while i < n:
        ch = command[i]
        if quote:
            current.append(ch)
            if ch == "\\" and quote == '"' and i + 1 < n:
                current.append(command[i + 1])
                i += 2
                continue
            if ch == quote:
                quote = None
            i += 1
            continue
        if ch in ("'", '"'):
            quote = ch
            current.append(ch)
            i += 1
            continue
        if ch == "\\" and i + 1 < n:
            current.append(ch)
            current.append(command[i + 1])
            i += 2
            continue
        if ch == "\n" or ch == ";":
            segments.append("".join(current))
            current = []
            i += 1
            continue
        if ch in "&|":
            if i + 1 < n and command[i + 1] == ch:
                segments.append("".join(current))
                current = []
                i += 2
                continue
            segments.append("".join(current))
            current = []
            i += 1
            continue
        if ch == ">":
            has_redirection = True
        current.append(ch)
        i += 1
    if quote:
        parse_ok = False
    segments.append("".join(current))
    return [s.strip() for s in segments if s.strip()], has_redirection, parse_ok


def _tokens(segment: str) -> list:
    """Tolerant tokenizer: shlex when possible, whitespace split otherwise."""
    try:
        return shlex.split(segment, posix=True)
    except ValueError:
        return segment.split()


def _unwrap(tokens: list) -> list:
    """Strip VAR=value prefixes and wrapper programs (env/time/sudo/...)."""
    i = 0
    n = len(tokens)
    while i < n:
        tok = tokens[i]
        if re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", tok):
            i += 1
            continue
        if _basename(tok) in _WRAPPER_PROGRAMS:
            i += 1
            while i < n and (tokens[i].startswith("-") or re.match(r"^[A-Za-z_]\w*=", tokens[i])):
                i += 1
            continue
        break
    return tokens[i:]


def _roots_and_segments(command: str) -> list:
    """[(program, args, raw_segment)] for every parsed command segment."""
    out = []
    segments, _, _ = _split_segments(command)
    for seg in segments:
        core = _unwrap(_tokens(seg))
        if not core:
            continue
        out.append((_basename(core[0]), core[1:], seg))
    return out


def command_roots(command: str) -> list:
    """Root program names of every segment (best effort, policy metadata)."""
    return [prog for prog, _args, _seg in _roots_and_segments(command)]


def expected_nonzero(code, command: str) -> str:
    """Label when exit ``code`` is a normal negative result, else ''.

    ``grep``/``find`` exit 1 means "no match" and ``diff`` exit 1 means
    "differences found" — these are answers, not failures.
    """
    try:
        if int(code or 0) != 1:
            return ""
    except Exception:
        return ""
    for prog, args, _seg in _roots_and_segments(command):
        if prog in _NEGATIVE_EXIT_LABELS:
            return _NEGATIVE_EXIT_LABELS[prog]
        if prog == "git":
            sub = _git_subcommand(args)
            if sub in _NEGATIVE_GIT_SUBCMDS:
                return f"git {sub}: {_NEGATIVE_GIT_SUBCMDS[sub]}"
    return ""


def _extract_substitutions(command: str) -> list:
    """Inner command strings from ``$(...)`` and backticks (one level)."""
    subs = []
    for m in re.finditer(r"\$\((.*?)\)|`([^`]*)`", command or "", re.DOTALL):
        inner = m.group(1) or m.group(2) or ""
        if inner.strip():
            subs.append(inner.strip())
    return subs


def _rm_danger(args: list):
    flags = [a for a in args if a.startswith("-")]
    targets = [a for a in args if not a.startswith("-")]
    recursive = any("r" in f.lower().lstrip("-") for f in flags) or any(
        f.lower() == "--recursive" for f in flags
    )
    for t in targets:
        s = t.strip().strip("'\"")
        # System locations are protected even without -r (rm -f /etc/hosts).
        if re.match(
            r"^(/(etc|usr|bin|sbin|boot|dev|proc|sys|var|home|root|opt)(/|$)"
            r"|/[a-z]/(Windows|Program Files|ProgramData)(/|$)"
            r"|[A-Za-z]:[\\/]?(Windows|Program Files( \(x86\))?|ProgramData))",
            s, re.IGNORECASE,
        ):
            return f"delete under a protected system path: '{s}'"
        if not recursive:
            continue
        if s in ("", "/", "~", "~/", "$HOME", "${HOME}", ".", "..", "./", "*"):
            return f"recursive delete of '{s or '.'}'"
        if re.match(r"^[A-Za-z]:[\\/]?$", s) or re.match(r"^/[A-Za-z]/?$", s) or s.startswith("/*"):
            return f"recursive delete of '{s}'"
        if s.startswith("~/") or re.match(r"^\$(HOME|\{HOME\})(/|$)", s):
            return f"recursive delete under home: '{s}'"
    return None


def _first_positional(args: list, value_flags=frozenset()):
    """First non-flag arg, skipping flags that consume a value (git -C dir ...)."""
    i = 0
    while i < len(args):
        tok = args[i]
        if tok in value_flags:
            i += 2
            continue
        if tok.startswith("-"):
            i += 1
            continue
        return tok, i
    return "", -1


def _git_subcommand(args: list) -> str:
    return _first_positional(args, _GIT_VALUE_FLAGS)[0]


def _git_danger(args: list):
    sub = _git_subcommand(args)
    if sub == "push":
        return "git push is blocked in run_command — use the deploy gate (github_apply_gate)"
    if sub == "reset" and "--hard" in args:
        return "git reset --hard discards uncommitted work"
    if sub == "clean" and any(re.match(r"^-[a-z]*f", a) for a in args):
        return "git clean removes untracked files"
    if sub == "checkout" and "--" in args and "." in args:
        return "git checkout -- . discards working-tree changes"
    return None


_SIMPLE_DANGER = {
    "shutdown": "system shutdown",
    "diskpart": "disk partitioning",
    "regedit": "registry editor",
    "chkdsk": "filesystem check/repair",
    "sfc": "system file checker",
}


def _is_secret_path(token) -> bool:
    """True for .env / SSH / AWS / gh credential paths (OpenCode-style default)."""
    q = str(token or "").strip("'\"").replace("\\", "/").lower()
    base = q.rsplit("/", 1)[-1]
    if base == ".env" or (base.startswith(".env.") and not base.endswith(".example")):
        return True
    padded = q if q.startswith("/") else "/" + q
    for frag in _SECRET_PATH_FRAGMENTS:
        if frag in padded:
            return True
    return padded.endswith("/.ssh") or padded.endswith("/.aws")


def _secret_read_reason(prog: str, args: list):
    """Block shell reads of credential files; CMD_ALLOW_SECRET_READS=1 disables."""
    if os.environ.get("CMD_ALLOW_SECRET_READS", "").lower() in ("1", "true", "yes"):
        return None
    if prog in _READ_FILE_PROGRAMS:
        check = args
    elif prog in _GREP_LIKE_PROGRAMS:
        # First positional is the pattern, not a file operand.
        idx = next((i for i, a in enumerate(args) if not a.startswith("-")), None)
        check = args[idx + 1:] if idx is not None else []
    else:
        return None
    for a in check:
        if _is_secret_path(a):
            return f"reading secret file '{a}' is blocked (set CMD_ALLOW_SECRET_READS=1 to override)"
    return None


def _danger(prog: str, args: list, raw: str):
    """Return a reason string when this program+args is destructive, else None."""
    reason = _secret_read_reason(prog, args)
    if reason:
        return reason
    if prog == "rm":
        return _rm_danger(args)
    if prog == "git":
        return _git_danger(args)
    if prog in _SIMPLE_DANGER:
        return _SIMPLE_DANGER[prog]
    if prog == "dd" and any(str(a).lower().startswith("of=/dev/") for a in args):
        return "raw write to a device"
    if prog == "net" and args and args[0].lower() in ("user", "localgroup"):
        return "account management"
    if prog == "reg" and args and args[0].lower() in ("delete", "add"):
        return "registry modification"
    if prog in ("taskkill",) and any(a.lower() in ("/f", "-f") for a in args):
        return "force-killing processes"
    if prog in ("pkill", "kill") and any(a in ("-9", "-KILL", "-SIGKILL") for a in args):
        return "SIGKILL process termination"
    if prog == "format" and args and re.match(r"^[A-Za-z]:", args[0]):
        return "drive format"
    return None


def check_command(command: str, _depth: int = 0) -> PolicyResult:
    """Full destructive-command check. ``allowed=False`` means veto."""
    if not command or not command.strip():
        return PolicyResult(allowed=True)
    text = command[:_MAX_PARSE_CHARS]
    low = text.lower()

    for pat in GLOBAL_DENY_SUBSTRINGS:
        if pat in low:
            return PolicyResult(False, pat, f"blocked pattern '{pat}'")
    for pat in _extra_deny_patterns():
        if pat in low:
            return PolicyResult(False, pat, f"blocked by CMD_DENY_EXTRA: '{pat}'")

    if _depth < _MAX_SUBST_DEPTH:
        for sub in _extract_substitutions(text):
            inner = check_command(sub, _depth + 1)
            if not inner.allowed:
                return inner

    segments, has_redir, parse_ok = _split_segments(text)
    roots = []
    for seg in segments:
        core = _unwrap(_tokens(seg))
        if not core:
            continue
        prog, args = _basename(core[0]), core[1:]
        roots.append(prog)
        reason = _danger(prog, args, seg)
        if reason:
            return PolicyResult(False, prog, reason, roots=roots)
        if prog in _SUBSHELL_PROGRAMS:
            joined = " ".join(args).lower()
            for pat in SUBSHELL_DENY_SUBSTRINGS:
                if pat in joined:
                    return PolicyResult(False, prog, f"nested shell contains '{pat}'", roots=roots)
            for tok in args:
                if _is_secret_path(tok):
                    return PolicyResult(False, prog, f"nested shell reads secret file '{tok}'", roots=roots)
    return PolicyResult(True, roots=roots, has_redirection=has_redir, parse_ok=parse_ok)


def is_destructive_shell(command: str) -> bool:
    """Plan-mode semantics: True when the command is a write/side effect.

    Anything not proven read-only is treated as destructive (conservative:
    the old allowlist vetoed unknown commands).
    """
    ok, _reason = is_read_only(command)
    return not ok


def is_read_only(command: str):
    """Return (True, "") for commands on the strict read-only allowlist."""
    if not command or not command.strip():
        return True, ""
    text = command[:_MAX_PARSE_CHARS]
    segments, has_redir, parse_ok = _split_segments(text)
    if has_redir:
        return False, "output redirection writes to a file"
    if not parse_ok:
        return False, "unbalanced quoting cannot be verified read-only"
    for seg in segments:
        core = _unwrap(_tokens(seg))
        if not core:
            continue
        prog, args = _basename(core[0]), core[1:]
        if prog in _SUBSHELL_PROGRAMS:
            return False, f"nested shell '{prog}' is not read-only"
        if prog in ("python", "python3", "py"):
            if args and args[0] == "-c" and len(args) > 1:
                if _PY_WRITE_HINTS.search(" ".join(args[1:])):
                    return False, "python -c contains write operations"
                continue
            return False, "python execution is not verifiably read-only"
        if prog == "git":
            sub, idx = _first_positional(args, _GIT_VALUE_FLAGS)
            if sub in _GIT_READ_SUBCMDS:
                tail = [a for a in args[idx + 1:] if not a.startswith("-")]
                if sub == "branch" and tail:
                    return False, "git branch with arguments may write"
                if sub == "remote" and tail and tail[0] not in ("show", "get-url"):
                    return False, f"git remote {tail[0]} may write"
                if sub == "config" and not any(
                    a in ("--get", "--get-all", "--get-regexp", "--list", "-l") for a in args
                ):
                    return False, "git config without a read flag may write"
                continue
            return False, f"git {sub or '(none)'} may write"
        if prog == "find" and any(a in ("-delete", "-exec", "-execdir", "-ok") for a in args):
            return False, "find with -delete/-exec is not read-only"
        if prog not in _READ_ONLY_PROGRAMS:
            return False, f"'{prog}' is not on the read-only allowlist"
    return True, ""


def is_server_like(command: str) -> bool:
    """Heuristic: this command starts a long-lived dev server (auto-background)."""
    if not command:
        return False
    for prog, args, _seg in _roots_and_segments(command):
        if prog in _SERVER_PROGRAMS:
            if prog in ("next", "nuxt", "astro", "ng"):
                sub = next((a for a in args if not a.startswith("-")), "")
                if sub not in _SERVER_SUBCOMMANDS:
                    continue
            return True
        if prog in ("npm", "pnpm", "yarn", "bun"):
            sub, idx = _first_positional(args, _NPM_VALUE_FLAGS)
            if sub in ("run", "exec"):
                tail = [a for a in args[idx + 1:] if not a.startswith("-")]
                token = tail[0] if tail else ""
            else:
                token = sub
            base = str(token).split(":")[0].lower()
            if base in _SERVER_SUBCOMMANDS or base in _SERVER_PROGRAMS:
                return True
            continue
        if prog == "npx":
            token, _ = _first_positional(args, _NPM_VALUE_FLAGS)
            if token.lower() in _SERVER_PROGRAMS:
                return True
            continue
        if prog in ("python", "python3"):
            if "-m" in args and any(
                a in ("http.server", "uvicorn", "flask", "gunicorn", "streamlit")
                for a in args
            ):
                return True
            if any(a == "runserver" for a in args):
                return True
            continue
        if prog in ("flask", "gunicorn", "uvicorn"):
            return True
        if prog in ("docker", "docker-compose") and "up" in args:
            return True
        if any(a == "--watch" for a in args):
            return True
    return False


def needs_credentials(command: str) -> bool:
    """True when the command likely authenticates (gh/git/curl/package managers).

    Used to scope GH_TOKEN/GITHUB_TOKEN injection to the commands that need it
    instead of every shell in the session.
    """
    if not command:
        return False
    return bool(_CRED_PROGRAMS_RE.search(command))
