"""MainTool.astSearch — structural search over Python source.

Warehouse tool: implemented, exported and schema-registered, but not assigned
to any agent scope. Enable it for a specialist by adding "ast_search" to that
spec's tools list.

Python-only, stdlib ``ast``, zero dependencies. Rules answer structure
questions text search cannot: call sites, imports, subclasses, decorated
definitions, bare excepts, long functions, parameter-heavy functions, and
missing docstrings.
"""
import ast
from ._filewalk import display_path, iter_files, parse_globs, read_text, resolve_root
from ._shared import _log_tool
from ._symbols import annotation_for

_RESULTS_SHOWN = 100
_RESULTS_COLLECT = 1000

_RULES = (
    "call", "import", "class_bases", "decorator",
    "bare_except", "long_function", "many_params", "no_docstring",
)
_NAME_RULES = frozenset({"call", "import", "class_bases", "decorator"})


def _dotted(node) -> str:
    parts = []
    cur = node
    while isinstance(cur, ast.Attribute):
        parts.append(cur.attr)
        cur = cur.value
    if isinstance(cur, ast.Name):
        parts.append(cur.id)
        return ".".join(reversed(parts))
    return ""


def _name_matches(dotted: str, target: str) -> bool:
    if not dotted or not target:
        return False
    if "." in target:
        return dotted == target or dotted.endswith("." + target)
    return dotted.split(".")[-1] == target


def _collect(tree, rule: str, name: str, min_lines: int, max_params: int):
    hits = []
    if rule == "call":
        for node in ast.walk(tree):
            if isinstance(node, ast.Call):
                dotted = _dotted(node.func)
                if _name_matches(dotted, name):
                    hits.append((node.lineno, f"{dotted}()"))
    elif rule == "import":
        for node in ast.walk(tree):
            if isinstance(node, ast.Import):
                for alias in node.names:
                    if _name_matches(alias.name, name):
                        hits.append((node.lineno, f"import {alias.name}"))
            elif isinstance(node, ast.ImportFrom):
                module = node.module or ""
                for alias in node.names:
                    full = f"{module}.{alias.name}" if module else alias.name
                    if _name_matches(full, name) or _name_matches(module, name):
                        detail = f"from {module} import {alias.name}" if module else f"import {alias.name}"
                        hits.append((node.lineno, detail))
    elif rule == "class_bases":
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                for base in node.bases:
                    dotted = _dotted(base)
                    if _name_matches(dotted, name):
                        hits.append((node.lineno, f"class {node.name}({dotted})"))
    elif rule == "decorator":
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                for dec in node.decorator_list:
                    target = dec.func if isinstance(dec, ast.Call) else dec
                    dotted = _dotted(target)
                    if _name_matches(dotted, name):
                        kind = "class" if isinstance(node, ast.ClassDef) else "def"
                        hits.append((node.lineno, f"@{dotted} on {kind} {node.name}"))
    elif rule == "bare_except":
        for node in ast.walk(tree):
            if isinstance(node, ast.ExceptHandler) and node.type is None:
                hits.append((node.lineno, "except:"))
    elif rule == "long_function":
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                end = getattr(node, "end_lineno", None) or node.lineno
                length = end - node.lineno + 1
                if length >= min_lines:
                    hits.append((node.lineno, f"def {node.name} ({length} lines)"))
    elif rule == "many_params":
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                args = node.args
                count = len(args.posonlyargs) + len(args.args) + len(args.kwonlyargs)
                if count > max_params:
                    hits.append((node.lineno, f"def {node.name} ({count} params)"))
    elif rule == "no_docstring":
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
                if ast.get_docstring(node) is None:
                    kind = "class" if isinstance(node, ast.ClassDef) else "def"
                    hits.append((node.lineno, f"{kind} {node.name} (no docstring)"))
    hits.sort(key=lambda h: h[0])
    return hits


def ast_search(ctx: object = None, rule: str = "", name: str = "", path: str = "",
               glob: str = "", min_lines: int = 60, max_params: int = 5,
               max_results: int = _RESULTS_SHOWN) -> str:
    """Structural search over Python files. Rules:

    - call: call sites of `name` (function/method, dotted names allowed)
    - import: imports of `name` (module or symbol)
    - class_bases: classes subclassing `name`
    - decorator: definitions decorated with `name`
    - bare_except: `except:` without an exception type
    - long_function: functions at least `min_lines` lines (default 60)
    - many_params: functions with more than `max_params` parameters (default 5)
    - no_docstring: functions/classes without a docstring
    """
    rule = str(rule or "").strip().lower()
    if rule not in _RULES:
        return f"Error: rule must be one of: {', '.join(_RULES)}"
    target = str(name or "").strip()
    if rule in _NAME_RULES and not target:
        return f"Error: rule '{rule}' requires name"
    try:
        show = int(max_results or _RESULTS_SHOWN)
    except Exception:
        show = _RESULTS_SHOWN
    show = max(1, min(show, _RESULTS_COLLECT))
    try:
        min_lines = int(min_lines or 60)
    except Exception:
        min_lines = 60
    try:
        max_params = int(max_params or 5)
    except Exception:
        max_params = 5

    root, single_file, base, error = resolve_root(path, ctx)
    if error:
        return error
    include, exclude = parse_globs(glob)

    records = []
    total_hits = 0
    limit_reached = False
    for fpath in iter_files(root, include, exclude, single_file):
        if not fpath.lower().endswith(".py"):
            continue
        text = read_text(fpath)
        if text is None:
            continue
        try:
            tree = ast.parse(text, filename=fpath)
        except (SyntaxError, ValueError):
            continue
        for line_no, detail in _collect(tree, rule, target, min_lines, max_params):
            records.append((fpath, line_no, detail))
            total_hits += 1
            if total_hits >= _RESULTS_COLLECT:
                limit_reached = True
                break
        if limit_reached:
            break

    if not records:
        suffix = f" (name='{target}')" if target else ""
        return f"No matches found for rule '{rule}'{suffix}"

    lines = []
    for fpath, line_no, detail in records:
        rel = display_path(fpath, base)
        ann = annotation_for(fpath, line_no)
        prefix = ann + " " if ann else ""
        lines.append(f"{rel}:{line_no}: {prefix}{detail}".rstrip())

    total_label = f"{total_hits}+" if limit_reached else str(total_hits)
    if total_hits > show:
        full_path = ""
        try:
            from tools._runtime import _write_offload_file
            full_path = _write_offload_file("\n".join(lines), "ast_search")
        except Exception:
            full_path = ""
        pointer = f"; full list: {full_path}" if full_path else ""
        header = f"Found {total_label} matches (showing first {show}{pointer}):"
        lines = lines[:show]
    else:
        header = f"Found {total_label} matches:"

    output_str = header + "\n" + "\n".join(lines)
    _log_tool("ast_search", {"rule": rule, "name": target, "path": path, "glob": glob},
              output_str[:500])
    return output_str
