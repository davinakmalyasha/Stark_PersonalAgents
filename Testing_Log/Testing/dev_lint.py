"""
Local Code Quality Engine — Pillars 63, 75, 96.
Catches and fixes syntax/import issues BEFORE expensive LLM correction loops.

Covers:
  Pillar 63: Local AST Diff Validation for Self-Correction
  Pillar 75: Local AST-Driven Import Cleanup & Syntax Refactoring
  Pillar 96: Zero-Shot Local Regex-Based Code Patching (Fast-Path Healing)
"""
import ast
import os
import re
import subprocess
import tempfile
from typing import Optional


# ── Pillar 63 + 75: AST Validation & Import Cleanup ──────────────────────────

def run_syntax_check(code: str) -> dict:
    """Validate Python syntax via ast.parse. Returns {"valid": bool, "error": str|None}."""
    try:
        ast.parse(code)
        return {"valid": True, "error": None}
    except SyntaxError as e:
        return {"valid": False, "error": f"Line {e.lineno}: {e.msg}"}


def _find_executable(name: str) -> Optional[str]:
    """Locate an executable in PATH or common Windows/Python locations."""
    import shutil
    # Try PATH first
    found = shutil.which(name)
    if found:
        return found
    # Try common Python script locations on Windows
    candidates = [
        os.path.join(os.path.dirname(os.sys.executable), f"{name}.exe"),
        os.path.join(os.path.dirname(os.sys.executable), "Scripts", f"{name}.exe"),
        os.path.join(os.path.dirname(os.sys.executable), f"{name}.cmd"),
    ]
    for c in candidates:
        if os.path.isfile(c):
            return c
    return None


def run_local_lint(code: str, file_path: str = "") -> dict:
    """
    Run ruff/autoflake on code to auto-fix imports and formatting.
    Gracefully degrades if tools are not installed.

    Returns: {"clean_code": str, "fixes_applied": [str], "issues_remaining": [str]}
    """
    fixes_applied: list[str] = []
    issues_remaining: list[str] = []

    # Write code to a temp file with the correct extension
    suffix = os.path.splitext(file_path)[1] if file_path else ".py"
    with tempfile.NamedTemporaryFile(mode="w", suffix=suffix, delete=False, encoding="utf-8") as f:
        f.write(code)
        tmp_path = f.name

    try:
        # ── Step 1: ruff check --fix ──
        ruff = _find_executable("ruff")
        if ruff:
            try:
                result = subprocess.run(
                    [ruff, "check", "--fix", "--quiet", tmp_path],
                    capture_output=True, text=True, timeout=30
                )
                if result.returncode == 0 and result.stdout:
                    fixes_applied.append(f"ruff: {result.stdout.strip()}")
            except (subprocess.TimeoutExpired, FileNotFoundError):
                pass

        # ── Step 2: autoflake (remove unused imports/variables) ──
        autoflake = _find_executable("autoflake")
        if autoflake:
            try:
                result = subprocess.run(
                    [autoflake, "--in-place", "--remove-unused-variables",
                     "--remove-all-unused-imports", tmp_path],
                    capture_output=True, text=True, timeout=30
                )
                if result.stdout:
                    fixes_applied.append(f"autoflake: {result.stdout.strip()}")
            except (subprocess.TimeoutExpired, FileNotFoundError):
                pass

        # ── Read back the cleaned code ──
        with open(tmp_path, "r", encoding="utf-8") as f:
            clean_code = f.read()

        # ── Step 3: Final syntax check on cleaned code ──
        syntax = run_syntax_check(clean_code)
        if not syntax["valid"]:
            issues_remaining.append(syntax["error"])

    finally:
        try:
            os.unlink(tmp_path)
        except OSError:
            pass

    return {
        "clean_code": clean_code if fixes_applied else code,
        "fixes_applied": fixes_applied,
        "issues_remaining": issues_remaining,
    }


# ── Pillar 96: Regex Fast-Path Code Patching ─────────────────────────────────

def fast_fix_common_issues(code: str) -> tuple:
    """
    Apply deterministic regex fixes for common Python syntax issues.
    These are safe, single-pass transformations that avoid LLM correction turns.

    Returns: (fixed_code: str, fixes_descriptions: list[str])
    """
    fixes: list[str] = []
    lines = code.splitlines()

    # ── Fix 1: Missing closing parens/braces/brackets ──
    open_count = code.count("(") - code.count(")")
    if open_count > 0 and open_count <= 3:
        code = code.rstrip() + ")" * open_count + "\n"
        fixes.append(f"Added {open_count} missing closing parenthesis/parentheses")
    open_count = code.count("[") - code.count("]")
    if open_count > 0 and open_count <= 3:
        code = code.rstrip() + "]" * open_count + "\n"
        fixes.append(f"Added {open_count} missing closing bracket(s)")
    open_count = code.count("{") - code.count("}")
    if open_count > 0 and open_count <= 3:
        code = code.rstrip() + "}" * open_count + "\n"
        fixes.append(f"Added {open_count} missing closing brace(s)")

    # ── Fix 2: Trailing comma before closing bracket/paren on its own line ──
    # Pattern: ,\n] or ,\n) or ,\n} — remove the trailing comma
    code, n = re.subn(r",(\s*\n\s*[\]\)}])", r"\1", code)
    if n:
        fixes.append(f"Removed {n} trailing comma(s) before closing bracket/paren/brace")

    # ── Fix 3: Bare except: → except Exception: ──
    code, n = re.subn(r'\bexcept\s*:', 'except Exception:', code)
    if n:
        fixes.append(f"Fixed {n} bare except(s) → except Exception:")

    # ── Fix 4: Python 2-style print statement → print() function ──
    # Only fix if it looks like Python 2 print: `print "string"` (no parens)
    new_lines = []
    fixed_prints = 0
    for line in lines:
        stripped = line.lstrip()
        if stripped.startswith("print ") and not stripped.startswith("print("):
            indent = line[:len(line) - len(stripped)]
            content = stripped[6:].strip()
            line = f'{indent}print({content})'
            fixed_prints += 1
        new_lines.append(line)
    if fixed_prints:
        code = "\n".join(new_lines)
        fixes.append(f"Fixed {fixed_prints} Python 2-style print statement(s)")

    # ── Fix 5: Mixed tab/space indentation → spaces ──
    has_tabs = any("\t" in l for l in lines)
    has_spaces = any(l.startswith("    ") or l.startswith("  ") for l in lines if l.strip())
    if has_tabs and has_spaces:
        new_lines = []
        for line in lines:
            stripped = line.lstrip("\t")
            leading_tabs = len(line) - len(stripped)
            new_lines.append("    " * leading_tabs + stripped)
        code = "\n".join(new_lines)
        fixes.append("Normalized mixed tab/space indentation to spaces")

    # ── Fix 6: Missing 'self' in method signatures ──
    # Pattern: `def method_name(...)` inside a class where first arg is not self
    # Conservative: only fix if there's clear indentation and class context
    new_lines = []
    in_class = False
    class_indent = 0
    fixed_self = 0
    for line in lines:
        stripped = line.strip()
        indent_len = len(line) - len(line.lstrip())

        if stripped.startswith("class ") and stripped.endswith(":"):
            in_class = True
            class_indent = indent_len
        elif in_class and indent_len <= class_indent and stripped:
            in_class = False

        if in_class and indent_len > class_indent:
            # Match: def method_name(  — but not def __init__ or def method_name(self
            m = re.match(r'def\s+(__\w+__|\w+)\s*\(', stripped)
            if m:
                method_name = m.group(1)
                # Get the argument list
                arg_start = stripped.index("(")
                # Check if first arg is 'self'
                inner = stripped[arg_start + 1:]
                # Find first argument
                first_arg = inner.split(",")[0].strip()
                if first_arg and first_arg != "self" and first_arg != "cls":
                    if not method_name.startswith("__") or method_name == "__init__":
                        # Insert self
                        new_stripped = stripped[:arg_start + 1] + "self, " + inner
                        line = " " * indent_len + new_stripped
                        fixed_self += 1

        new_lines.append(line)
    if fixed_self:
        code = "\n".join(new_lines)
        fixes.append(f"Added missing 'self' parameter to {fixed_self} method(s)")

    # ── Fix 7: Empty loops with 'pass' ──
    # Pattern: `def foo():\n    ` with no body
    fixed_pass = 0
    new_lines = []
    for i, line in enumerate(lines):
        stripped = line.rstrip()
        if stripped.endswith(":") and (
            stripped.strip().startswith("def ") or
            stripped.strip().startswith("class ") or
            stripped.strip().startswith("if ") or
            stripped.strip().startswith("elif ") or
            stripped.strip().startswith("else:") or
            stripped.strip().startswith("for ") or
            stripped.strip().startswith("while ") or
            stripped.strip().startswith("try:") or
            stripped.strip().startswith("except")
        ):
            # Check next line has content
            if i + 1 >= len(lines) or not lines[i + 1].strip():
                indent = len(line) - len(line.lstrip()) + 4
                new_lines.append(line)
                new_lines.append(" " * indent + "pass  # auto-inserted by dev_lint")
                fixed_pass += 1
                continue
        new_lines.append(line)
    if fixed_pass:
        code = "\n".join(new_lines)
        fixes.append(f"Inserted 'pass' in {fixed_pass} empty block(s)")

    return code, fixes


# ── Combined entry point for developer_agent.py integration ──────────────────

def lint_and_fix(code: str, file_path: str = "") -> dict:
    """
    Full pipeline: syntax check → fast regex fixes → ruff/autoflake → final syntax check.
    Called after Developer generates code, before sending to Tester.

    Returns:
        {"code": str, "original_valid": bool, "final_valid": bool,
         "fast_fixes": [str], "lint_fixes": [str], "issues_remaining": [str]}
    """
    result = {
        "code": code,
        "original_valid": False,
        "final_valid": False,
        "fast_fixes": [],
        "lint_fixes": [],
        "issues_remaining": [],
    }

    # Step 1: Initial syntax check
    syntax = run_syntax_check(code)
    result["original_valid"] = syntax["valid"]

    # Step 2: Fast regex fixes (always safe, no external deps)
    code, fast_fixes = fast_fix_common_issues(code)
    result["fast_fixes"] = fast_fixes

    # Step 3: ruff/autoflake cleanup (if available)
    lint_result = run_local_lint(code, file_path)
    code = lint_result["clean_code"]
    result["lint_fixes"] = lint_result["fixes_applied"]
    result["issues_remaining"] = lint_result["issues_remaining"]

    # Step 4: Final syntax check
    syntax = run_syntax_check(code)
    result["final_valid"] = syntax["valid"]
    if not syntax["valid"] and syntax["error"] not in result["issues_remaining"]:
        result["issues_remaining"].append(syntax["error"])

    result["code"] = code
    return result
