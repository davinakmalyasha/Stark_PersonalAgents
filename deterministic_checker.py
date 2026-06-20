"""
Stage 2: Fast Deterministic Cascade — Reflexion Pattern.

Cascade of deterministic checks ordered by cost. Each stage is a hard gate:
  Stage 2a: File existence    (near-zero cost, catches missing deliverables)
  Stage 2b: Syntax validation  (ast.parse, catches typos/parse errors)
  Stage 2c: Lint auto-fix      (ruff, catches import/format issues)
  Stage 2d: Basic schema       (regex-based structural checks)

RULES (from production Reflexion pattern):
  1. Deterministic checker FIRST — 1000× cheaper than LLM critic.
  2. AUTO-FIX where possible (syntax, imports) to save a critic round.
  3. Only unfixable issues escalate to Stage 3 LLM Critic.
  4. If ALL checks pass → ship immediately (no LLM critic needed).

Returns structured findings compatible with the Critic node.
"""

import ast
import os
import re
import subprocess
import tempfile
from dataclasses import dataclass, field
from typing import Optional


# ═══════════════════════════════════════════════════════════════════════════════
# Data Model
# ═══════════════════════════════════════════════════════════════════════════════

@dataclass
class Finding:
    """A single deterministic finding — compatible with Critic output schema."""
    file: str
    line: int = 0
    severity: str = "medium"  # low | medium | high | critical
    category: str = ""        # syntax | lint | import | missing_file | schema
    issue: str = ""
    fix: str = ""
    auto_fixed: bool = False


@dataclass
class Stage2Result:
    """Structured result from the deterministic cascade."""
    passed: bool = True
    files_checked: list[str] = field(default_factory=list)
    findings: list[Finding] = field(default_factory=list)
    auto_fixes_applied: list[str] = field(default_factory=list)
    needs_critic: bool = False
    summary: str = ""


# ═══════════════════════════════════════════════════════════════════════════════
# Stage 2a: File Existence
# ═══════════════════════════════════════════════════════════════════════════════

def check_file_existence(file_paths: list[str]) -> list[Finding]:
    """Verify all tracked files actually exist on disk."""
    findings: list[Finding] = []
    for fpath in file_paths:
        if not os.path.isfile(fpath):
            findings.append(Finding(
                file=fpath,
                line=0,
                severity="critical",
                category="missing_file",
                issue=f"Expected file was not created: {fpath}",
                fix=f"Create the file using write_file tool at path: {fpath}",
            ))
    return findings


# ═══════════════════════════════════════════════════════════════════════════════
# Stage 2b: Syntax Validation (ast.parse)
# ═══════════════════════════════════════════════════════════════════════════════

def check_syntax(file_paths: list[str]) -> list[Finding]:
    """Validate Python syntax on all tracked .py files."""
    findings: list[Finding] = []
    for fpath in file_paths:
        if not fpath.endswith(".py"):
            continue
        if not os.path.isfile(fpath):
            continue
        try:
            with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                code = f.read()
            ast.parse(code)
        except SyntaxError as e:
            findings.append(Finding(
                file=fpath,
                line=e.lineno or 0,
                severity="critical",
                category="syntax",
                issue=f"SyntaxError at line {e.lineno}: {e.msg}",
                fix=(
                    f"In {fpath}, line {e.lineno}: fix the syntax error — "
                    f"check for missing colons, unmatched brackets, or invalid indentation. "
                    f"Context: {e.text.strip() if e.text else 'unknown'}"
                ),
            ))
    return findings


# ═══════════════════════════════════════════════════════════════════════════════
# Stage 2c: Lint Auto-Fix (ruff)
# ═══════════════════════════════════════════════════════════════════════════════

def _find_executable(name: str) -> Optional[str]:
    """Locate an executable in PATH or common locations."""
    import shutil
    found = shutil.which(name)
    if found:
        return found
    candidates = [
        os.path.join(os.path.dirname(os.sys.executable), f"{name}.exe"),
        os.path.join(os.path.dirname(os.sys.executable), "Scripts", f"{name}.exe"),
    ]
    for c in candidates:
        if os.path.isfile(c):
            return c
    return None


def run_ruff_fix(file_paths: list[str], project_path: str) -> tuple[list[str], list[Finding]]:
    """Run ruff --fix on tracked files. Returns (fixes_applied, remaining_findings)."""
    fixes_applied: list[str] = []
    remaining_findings: list[Finding] = []

    ruff = _find_executable("ruff")
    if not ruff:
        return fixes_applied, remaining_findings

    py_files = [f for f in file_paths if f.endswith(".py") and os.path.isfile(f)]
    if not py_files:
        return fixes_applied, remaining_findings

    for fpath in py_files:
        try:
            # Try --fix first (auto-fix safe issues)
            result = subprocess.run(
                [ruff, "check", fpath, "--fix", "--quiet"],
                cwd=project_path,
                capture_output=True, text=True, timeout=15,
            )
            if result.returncode == 0:
                if result.stdout.strip():
                    fixes_applied.append(f"{fpath}: ruff auto-fixed issues")

            # Now check for remaining issues (failures that ruff can't auto-fix)
            result2 = subprocess.run(
                [ruff, "check", fpath, "--output-format", "text", "--quiet"],
                cwd=project_path,
                capture_output=True, text=True, timeout=10,
            )
            if result2.returncode != 0:
                lines = result2.stdout.strip().splitlines()
                for line in lines[:8]:  # Cap at 8 findings per file
                    finding = _parse_ruff_line(line, fpath)
                    if finding:
                        remaining_findings.append(finding)
        except (subprocess.TimeoutExpired, FileNotFoundError, OSError):
            pass

    return fixes_applied, remaining_findings


def _parse_ruff_line(line: str, fpath: str) -> Optional[Finding]:
    """Parse a ruff output line into a Finding. Format: file:line:col: code message"""
    match = re.match(r'.+:(\d+):(\d+):\s*(\w+)\s+(.+)', line)
    if match:
        lineno = int(match.group(1))
        code = match.group(3)
        message = match.group(4)
        severity = "high" if code.startswith("F") else "medium"
        return Finding(
            file=fpath,
            line=lineno,
            severity=severity,
            category="lint",
            issue=f"[{code}] {message}",
            fix=f"In {fpath}, line {lineno}: {message} (ruff rule {code})",
        )
    return None


# ═══════════════════════════════════════════════════════════════════════════════
# Stage 2d: Basic Schema / Structure Checks (regex-based)
# ═══════════════════════════════════════════════════════════════════════════════

# Patterns that signal "not production-ready"
_STUB_PATTERNS = [
    (r'^\s*(pass|\.\.\.)\s*$', "stub", "Placeholder body — function/class does nothing"),
    (r'#\s*TODO', "stub", "Unresolved TODO comment"),
    (r'#\s*FIXME', "stub", "Unresolved FIXME comment"),
    (r'raise\s+NotImplementedError', "stub", "NotImplementedError — incomplete implementation"),
    (r'return\s+None\s*$', "stub", "Function returns None unconditionally (possible stub)"),
]

# Patterns that often signal bugs in specific tech stacks.
# Each entry: (regex, category, issue, severity). Severity is per-pattern so
# context-dependent hints (e.g. innerHTML) don't hard-fail correct code the way
# genuinely dangerous patterns (eval, document.write) should.
_TECH_PATTERNS = {
    ".py": [
        (r'except\s*:', "bare_except", "Bare except: — catches everything including SystemExit", "high"),
        (r'except\s+Exception\s*:', "broad_except", "Broad except Exception — consider specific exceptions", "medium"),
        (r'f["\'].*?\{.*?\}.*?["\'].*?execute|f["\'].*?\{.*?\}.*?["\'].*?sql', "sql_injection", "Possible SQL injection via f-string in execute/sql call", "high"),
        (r'os\.system\(.*?\{', "command_injection", "Possible command injection via os.system with f-string", "high"),
        (r'subprocess\.(call|run|Popen)\(.*?shell\s*=\s*True', "shell_injection", "subprocess with shell=True — possible injection risk", "high"),
    ],
    ".js": [
        (r'document\.write\s*\(', "bad_practice", "document.write() is deprecated", "high"),
        (r'eval\s*\(', "eval", "eval() is dangerous — use JSON.parse or structured clone", "high"),
        (r'innerHTML\s*=', "xss_risk", "innerHTML assignment — only an XSS risk if assigned UNTRUSTED input; static HTML is fine", "medium"),
    ],
    ".ts": [
        (r':\s*any\b', "no_any", "Type 'any' — defeats type checking", "medium"),
        (r'as\s+any\b', "no_any", "Cast to 'any' — defeats type checking", "medium"),
    ],
}


def check_schema(file_paths: list[str]) -> list[Finding]:
    """Regex-based structural checks for stubs, TODOs, and anti-patterns."""
    findings: list[Finding] = []
    for fpath in file_paths:
        if not os.path.isfile(fpath):
            continue
        ext = os.path.splitext(fpath)[1]
        try:
            with open(fpath, "r", encoding="utf-8", errors="replace") as f:
                lines = f.readlines()
        except Exception:
            continue

        # Check stub patterns
        for i, line in enumerate(lines, start=1):
            for pattern, category, issue in _STUB_PATTERNS:
                if re.search(pattern, line, re.IGNORECASE):
                    findings.append(Finding(
                        file=fpath, line=i, severity="medium",
                        category=category, issue=issue,
                        fix=f"In {fpath}, line {i}: replace stub/placeholder with real implementation",
                    ))

        # Check tech-specific anti-patterns (per-pattern severity)
        if ext in _TECH_PATTERNS:
            for i, line in enumerate(lines, start=1):
                for pattern, category, issue, severity in _TECH_PATTERNS[ext]:
                    if re.search(pattern, line, re.IGNORECASE):
                        findings.append(Finding(
                            file=fpath, line=i, severity=severity,
                            category=category, issue=issue,
                            fix=f"In {fpath}, line {i}: {issue}. Refactor with safer alternative.",
                        ))

    return findings


# ═══════════════════════════════════════════════════════════════════════════════
# Stage 2: Full Cascade (entry point)
# ═══════════════════════════════════════════════════════════════════════════════

def run_deterministic_cascade(
    tracked_files: list[str],
    project_path: str,
) -> Stage2Result:
    """
    Run the full Stage 2 cascade: existence → syntax → lint → schema.

    Cascade rules:
    - Each stage runs regardless of prior failures (collect all findings).
    - Auto-fix is attempted at the lint stage.
    - Only unfixable findings escalate to Stage 3.

    Returns Stage2Result with passed=True if ALL checks are clean and NO findings remain.
    """
    result = Stage2Result()
    all_py_files = [f for f in tracked_files if f.endswith(".py")]

    # ── 2a: File Existence ────────────────────────────────────────────────
    missing = check_file_existence(tracked_files)
    result.findings.extend(missing)

    # ── 2b: Syntax Validation ─────────────────────────────────────────────
    syntax_findings = check_syntax(tracked_files)
    result.findings.extend(syntax_findings)

    # ── 2c: Lint Auto-Fix ─────────────────────────────────────────────────
    if all_py_files:
        lint_fixes, lint_findings = run_ruff_fix(all_py_files, project_path)
        result.auto_fixes_applied.extend(lint_fixes)
        result.findings.extend(lint_findings)

    # ── 2d: Schema / Structure Checks ─────────────────────────────────────
    schema_findings = check_schema(tracked_files)
    result.findings.extend(schema_findings)

    # ── Determine result ───────────────────────────────────────────────────
    result.files_checked = tracked_files

    # Separate critical/high/medium findings
    critical = [f for f in result.findings if f.severity == "critical"]
    high = [f for f in result.findings if f.severity == "high"]
    medium = [f for f in result.findings if f.severity == "medium"]
    low = [f for f in result.findings if f.severity == "low"]

    # Critical/high findings always mean FAIL
    # Medium findings that were auto-fixed are OK
    unfixed = [f for f in result.findings if not f.auto_fixed]

    if not unfixed:
        result.passed = True
        result.needs_critic = False
        result.summary = (
            f"Stage 2 PASSED: {len(tracked_files)} files checked. "
            f"{len(result.auto_fixes_applied)} auto-fix(es) applied. "
            f"No unfixed findings remain."
        )
    else:
        result.passed = False
        result.needs_critic = bool(critical or high or len(medium) > 3)
        result.summary = (
            f"Stage 2 FAILED: {len(unfixed)} unfixed finding(s) "
            f"({len(critical)} critical, {len(high)} high, {len(medium)} medium, {len(low)} low). "
            f"{len(result.auto_fixes_applied)} auto-fix(es) applied. "
            + (f"Escalating to Stage 3 Critic." if result.needs_critic else
               f"All remaining issues are low severity — proceeding.")
        )

    return result


# ═══════════════════════════════════════════════════════════════════════════════
# Quick check helpers (used inline, not part of cascade)
# ═══════════════════════════════════════════════════════════════════════════════

def quick_syntax_ok(file_path: str) -> bool:
    """Super-fast syntax check for a single file. Returns True if clean."""
    if not file_path.endswith(".py") or not os.path.isfile(file_path):
        return True
    try:
        with open(file_path, "r", encoding="utf-8", errors="replace") as f:
            ast.parse(f.read())
        return True
    except SyntaxError:
        return False


def files_exist(file_paths: list[str]) -> bool:
    """Check all files exist. Returns True if ALL found."""
    return all(os.path.isfile(f) for f in file_paths)


def format_findings_for_developer(findings: list[Finding], max_items: int = 10) -> str:
    """Format Stage 2 findings as a concise message for the developer agent."""
    if not findings:
        return ""

    critical = [f for f in findings if f.severity == "critical" and not f.auto_fixed]
    high = [f for f in findings if f.severity == "high" and not f.auto_fixed]
    medium = [f for f in findings if f.severity == "medium" and not f.auto_fixed]

    lines = ["[STAGE 2 — DETERMINISTIC CHECK FAILED]\n"]
    lines.append(f"Critical: {len(critical)}, High: {len(high)}, Medium: {len(medium)}\n")

    all_issues = critical + high + medium
    for f in all_issues[:max_items]:
        lines.append(f"  • {f.file}:{f.line or '?'} [{f.severity}] {f.issue}")
        if f.fix:
            lines.append(f"    Fix: {f.fix}")

    if len(all_issues) > max_items:
        lines.append(f"  ... and {len(all_issues) - max_items} more findings")

    return "\n".join(lines)
