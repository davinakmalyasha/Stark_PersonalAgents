"""
Repository Map Generator — Compiles codebases into a token-efficient signature map.
Uses tree-sitter AST parsing for Python, PHP, TypeScript, and JavaScript.
Falls back to robust regex signature extraction for Dart and parse failures.
"""
import os
import re
import sys
from typing import List, Tuple, Dict, Set, Optional

# Reconfigure console output encoding to UTF-8
try:
    sys.stdout.reconfigure(encoding='utf-8')
except Exception:
    pass

# Try importing tree_sitter
try:
    import tree_sitter_languages as tsl
    from tree_sitter import Parser, Node
    HAS_TREE_SITTER = True
except ImportError:
    HAS_TREE_SITTER = False


def _extract_ast_signature(node: 'Node', content_lines: List[str], lang: str, end_limit: int = None) -> str:
    """Extract complete signature line(s) from a tree-sitter AST node.

    end_limit bounds the forward scan at the node's own end row so sibling
    declarations on later lines can never merge into one signature.
    """
    start_row = node.start_point[0]
    end_row = start_row
    limit = min(end_limit, len(content_lines)) if end_limit is not None else len(content_lines)

    # Scan lines to find definition boundaries
    if lang in ("typescript", "tsx", "javascript", "php"):
        for r in range(start_row, limit):
            line = content_lines[r]
            if '{' in line:
                end_row = r
                break
            end_row = r
    elif lang == "python":
        for r in range(start_row, limit):
            line = content_lines[r]
            cleaned = line.split('#')[0].strip()
            if cleaned.endswith(':'):
                end_row = r
                break
            end_row = r
            
    sig_lines = content_lines[start_row:end_row + 1]
    
    # Strip brackets/bodies for C-like languages
    if lang in ("typescript", "tsx", "javascript", "php") and sig_lines:
        last_line = sig_lines[-1]
        brace_idx = last_line.find('{')
        if brace_idx != -1:
            sig_lines[-1] = last_line[:brace_idx].rstrip()
            
    # Strip trailing colon for Python
    if lang == "python" and sig_lines:
        last_line = sig_lines[-1]
        if last_line.rstrip().endswith(':'):
            sig_lines[-1] = last_line.rstrip()[:-1].rstrip()
            
    return " ".join(line.strip() for line in sig_lines)


def _extract_symbol_doc(content_lines: List[str], start_row: int, lang: str) -> str:
    """One-line description for a symbol: docstring (Python) or leading comment
    (JS/TS/PHP). Used to enrich the code map with a short 'what it does' note."""
    try:
        if lang == "python":
            for i in range(start_row + 1, min(start_row + 4, len(content_lines))):
                line = content_lines[i].strip()
                if line.startswith(('"""', "'''")):
                    doc = line.strip('"\' ')
                    return doc[:120] if doc else ""
                if line.startswith('"') or line.startswith("'"):
                    return line.strip('"\' ')[:120]
                if line.startswith("#"):
                    return line.lstrip("#").strip()[:120]
                if not line:
                    continue
                break
        else:
            for i in range(start_row - 1, max(-1, start_row - 4), -1):
                line = content_lines[i].strip()
                if line.startswith("//"):
                    return line.lstrip("/").strip()[:120]
                if line.startswith("/*") or line.startswith("*"):
                    return line.lstrip("/*").strip().lstrip("*").strip()[:120]
                if not line:
                    continue
                break
    except Exception:
        pass
    return ""


def _extract_ast_signatures(content: str, lang: str, with_lines: bool = False) -> list:
    """Extract signatures using tree-sitter AST parsing.

    Returns (sig, depth, doc) tuples, or (sig, depth, doc, line_no) when
    with_lines=True (1-based definition line — for jump-to-line navigation).
    """
    if not HAS_TREE_SITTER:
        raise ImportError("tree-sitter-languages is not installed.")
        
    parser = tsl.get_parser(lang)
    content_bytes = content.encode('utf-8')
    tree = parser.parse(content_bytes)
    content_lines = content.splitlines()
    
    signatures: list = []
    
    def traverse(node: 'Node', depth: int = 0) -> None:
        node_type = node.type
        is_target = False
        
        if lang == "python":
            is_target = node_type in ("class_definition", "function_definition")
        elif lang in ("typescript", "tsx", "javascript"):
            is_target = node_type in (
                "class_declaration", "method_definition", "function_declaration",
                "interface_declaration", "type_alias_declaration", "property_signature",
            )
        elif lang == "php":
            is_target = node_type in ("class_declaration", "method_declaration")
            
        if is_target:
            start_row = node.start_point[0]
            if node.type == "property_signature":
                # Single-line nodes — take the node's own byte span instead of
                # the line scan (several symbols can share one line).
                sig = content_bytes[node.start_byte:node.end_byte].decode('utf-8', errors="replace").strip()
                line_no = content_bytes[:node.start_byte].count(b"\n") + 1
            else:
                sig = _extract_ast_signature(node, content_lines, lang, end_limit=node.end_point[0])
                line_no = start_row + 1
            doc = _extract_symbol_doc(content_lines, start_row, lang)
            if with_lines:
                signatures.append((sig, depth, doc, line_no))
            else:
                signatures.append((sig, depth, doc))
            for child in node.children:
                traverse(child, depth + 1)
        else:
            for child in node.children:
                traverse(child, depth)
                
    traverse(tree.root_node)
    return signatures


def _extract_regex_signatures(content: str, ext: str, with_lines: bool = False) -> list:
    """Extract signatures using fallback regex matching (e.g. for Dart or broken code).

    Returns (sig, depth, doc) tuples, or (sig, depth, doc, line_no) when
    with_lines=True.
    """
    signatures: list = []
    lines = content.splitlines()
    
    for idx, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("//") or stripped.startswith("import ") or stripped.startswith("part "):
            continue
            
        indent = len(line) - len(line.lstrip())
        depth = indent // 4
        
        def _next_doc() -> str:
            for j in range(idx + 1, min(idx + 3, len(lines))):
                nxt = lines[j].strip()
                if nxt.startswith(('"""', "'''")):
                    return nxt.strip('"\' ')[:120]
                if nxt.startswith('"') or nxt.startswith("'"):
                    return nxt.strip('"\' ')[:120]
                if nxt.startswith("#"):
                    return nxt.lstrip("#").strip()[:120]
                if not nxt:
                    continue
                break
            return ""
        
        def _add(sig: str) -> None:
            if with_lines:
                signatures.append((sig, depth, _next_doc(), idx + 1))
            else:
                signatures.append((sig, depth, _next_doc()))
        
        # Dart signature patterns
        if ext == ".dart":
            if stripped.startswith("class ") or " class " in stripped:
                _add(stripped.split('{')[0].strip())
            elif '(' in stripped and ')' in stripped and ('{' in stripped or stripped.endswith(';')):
                if not any(k in stripped for k in ["if", "for", "while", "switch", "catch"]):
                    _add(stripped.split('{')[0].strip())
                    
        # Python fallback patterns
        elif ext == ".py":
            if stripped.startswith("class ") or stripped.startswith("def ") or stripped.startswith("async def "):
                _add(stripped.split(':')[0].strip())
                
        # PHP fallback patterns
        elif ext == ".php":
            if stripped.startswith("class ") or " class " in stripped:
                _add(stripped.split('{')[0].strip())
            elif "function " in stripped:
                _add(stripped.split('{')[0].strip())
                
        # JavaScript/TypeScript fallback patterns
        elif ext in (".js", ".jsx", ".ts", ".tsx"):
            if "class " in stripped or "interface " in stripped or "type " in stripped:
                _add(stripped.split('{')[0].strip())
            elif "function " in stripped or "=>" in stripped or ("(" in stripped and ")" in stripped and "{" in stripped):
                if not any(k in stripped for k in ["if", "for", "while", "switch", "catch"]):
                    _add(stripped.split('{')[0].strip())
                    
    return signatures


def get_file_signatures(file_path: str, ext: str, is_hot: bool = False, full_depth: bool = False, with_lines: bool = False) -> list:
    """Get signatures for a file using tree-sitter AST, falling back to regex.

    Returns (sig, depth, doc) tuples, or (sig, depth, doc, line_no) when
    with_lines=True.
    """
    if not os.path.isfile(file_path):
        return []
        
    try:
        with open(file_path, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
    except Exception:
        return []
        
    lang_map = {
        ".py": "python",
        ".php": "php",
        ".ts": "typescript",
        ".tsx": "tsx",
        ".js": "javascript",
        ".jsx": "javascript",
    }
    
    signatures: list = []
    
    # AST-First query
    if ext in lang_map and HAS_TREE_SITTER:
        try:
            signatures = _extract_ast_signatures(content, lang_map[ext], with_lines=with_lines)
        except Exception:
            # Fallback to regex on parser exception/corruption
            signatures = _extract_regex_signatures(content, ext, with_lines=with_lines)
    else:
        # Dart or non-tree-sitter extensions go straight to regex
        signatures = _extract_regex_signatures(content, ext, with_lines=with_lines)
        
    # Apply dynamic depth compression if the file is NOT hot and full depth
    # is not requested (the cached repo map requests full depth).
    if not is_hot and not full_depth:
        # Keep class declarations, top-level definitions, and their direct methods
        # (depth <= 1). Deeply nested closures/helpers (depth >= 2) are dropped to
        # keep the map token-cheap. Hot files keep full depth.
        if with_lines:
            signatures = [(sig, depth, doc, line) for sig, depth, doc, line in signatures if depth <= 1]
        else:
            signatures = [(sig, depth, doc) for sig, depth, doc in signatures if depth <= 1]
        
    return signatures


def get_file_signatures_with_lines(file_path: str, ext: str, full_depth: bool = True) -> List[Tuple[str, int, str, int]]:
    """(sig, depth, doc, line_no) tuples — 1-based definition lines, for
    jump-to-line navigation (view_signatures → read_file(offset=line))."""
    return get_file_signatures(file_path, ext, is_hot=False, full_depth=full_depth, with_lines=True)


# ── Compact map formatting ────────────────────────────────────────────────────
#
# The map is CACHED (byte-stable per project) so it must be:
#   * deterministic — bytes depend ONLY on the files (no task keywords)
#   * compact       — max navigation value per token
#   * append-only   — new files get appended at the BOTTOM; tail appends are
#     the only cache-safe mutation (everything above stays a prefix hit)


def _strip_kw(text: str) -> str:
    """Repeatedly strip leading language keywords from a signature line."""
    prev = None
    while prev != text:
        prev = text
        text = re.sub(
            r"^(?:async|static|export|default|public|private|protected|abstract|"
            r"final|readonly|sealed|global|declare|const|let|var|get|set|def|function|"
            r"void|int|double|num|bool|dynamic|string|late)\s+",
            "",
            text,
        )
    return text


def _split_top(s: str, sep: str = ",") -> List[str]:
    """Split on a separator, ignoring separators inside (), {}, []."""
    parts: List[str] = []
    cur: List[str] = []
    depth = 0
    for ch in s:
        if ch in "{[(":
            depth += 1
        elif ch in "}])":
            depth = max(0, depth - 1)
        if ch == sep and depth == 0:
            parts.append("".join(cur))
            cur = []
        else:
            cur.append(ch)
    if cur or parts:
        parts.append("".join(cur))
    return parts


def _strip_ann(p: str) -> str:
    """Cut a parameter at its type annotation / default value (depth-0 only)."""
    res: List[str] = []
    depth = 0
    for ch in p:
        if ch in "{[(":
            depth += 1
        elif ch in "}])":
            depth = max(0, depth - 1)
        if ch in ("=", ":") and depth == 0:
            break
        res.append(ch)
    return "".join(res).strip()


def _classify_sig(sig: str) -> str:
    """class / interface / type_alias / function / property from the raw signature."""
    t = _strip_kw(sig).lstrip()
    if t.startswith("class "):
        return "class"
    if t.startswith("interface "):
        return "interface"
    if t.startswith("type "):
        return "type_alias"
    return "function" if "(" in sig else "property"


def _compact_fn(sig: str) -> str:
    """Compact a function signature to `name(param1,param2)`."""
    lines = [ln for ln in sig.split("\n") if not ln.strip().startswith("@")]
    text = " ".join(ln.strip() for ln in lines if ln.strip())
    # Cut return-type annotations: `) -> T` (python) and `): T` (TS/PHP)
    mret = re.search(r"\)\s*(?::|->)[^{()]*$", text)
    if mret:
        text = text[:mret.start() + 1]
    name, params = None, ""
    mc = re.match(r"^constructor\s*\((.*)\)\s*$", text)
    if mc:
        name, params = "constructor", mc.group(1)
    else:
        t = _strip_kw(text)
        m = re.match(r"^([A-Za-z_$][\w$]*)\s*\((.*)\)\s*$", t, re.S)
        if m:
            name, params = m.group(1), m.group(2)
        else:
            # const x = (a) => ...  (regex-fallback arrows)
            m2 = re.match(r"^([\w$]+)\s*=\s*(?:\(([^)]*)\))\s*=>", t)
            if m2:
                name, params = m2.group(1), m2.group(2)
            else:
                return _strip_kw(text).strip().rstrip(";{}") or "?"
    cps = []
    for p in _split_top(params):
        p = p.strip()
        if not p:
            continue
        cps.append(p if p.startswith("*") or p.startswith("...") else _strip_ann(p))
    return f"{name}({','.join(cps)})"


def _compact_class(sig: str, kind: str) -> str:
    """Compact a class/interface to `Name{` or a type alias to `Name`."""
    text = _strip_kw(" ".join(ln.strip() for ln in sig.split("\n") if ln.strip()))
    if kind == "type_alias":
        m = re.match(r"^type\s+([A-Za-z_$][\w$]*)", text)
        return m.group(1) if m else (text.split("=", 1)[0].strip() or "?")
    m = re.match(r"^(?:class|interface)\s+([A-Za-z_$][\w$]*)", text)
    name = m.group(1) if m else (text.split(" ", 1)[0].strip() if text else "?")
    return name + "{"


def _compact_property(sig: str) -> str:
    """Compact a property/field signature to its bare name."""
    t = _strip_kw(sig).strip().rstrip(";").strip()
    res: List[str] = []
    depth = 0
    for ch in t:
        if ch in "{[(":
            depth += 1
        elif ch in "}])":
            depth = max(0, depth - 1)
        if ch in ("=", ":") and depth == 0:
            break
        res.append(ch)
    return "".join(res).strip() or "?"


def _format_compact(entries: List[Tuple[str, int, bool]]) -> str:
    """Join compact entries into one line, braces marking class/interface depth."""
    out: List[str] = []
    stack = 0
    for compact, depth, is_block in entries:
        while stack > depth:
            out.append("}")
            stack -= 1
        out.append(compact)
        if is_block:
            stack += 1
    out.extend("}" for _ in range(stack))
    return " ".join(out)


class RepoMapGenerator:
    """Orchestrates Repository Map compilation for the workspace.

    Deterministic, compact output: one line per file,
    `path: name(a,b) Class{member member}` — cache-stable per project.
    """

    def __init__(self, project_path: str) -> None:
        self.project_path = os.path.normpath(os.path.abspath(project_path))

    def generate_map(self) -> str:
        """Walk the directory, extract signatures, and format the compact map."""
        ignored_dirs = {
            "node_modules", "vendor", ".git", "venv", ".venv", "__pycache__",
            ".deep_agents", ".claude", "dist", "build", ".pytest_cache",
            ".vscode", ".idea", "storage", "public", "venv312", "scratch",
            ".antigravity"
        }
        
        supported_exts = {".py", ".php", ".ts", ".tsx", ".js", ".jsx", ".dart"}
        max_files = int(os.environ.get("MAP_MAX_FILES", "300"))
        
        # 1. Walk directory and group file paths
        files_to_process: List[str] = []
        try:
            for root, dirs, files in os.walk(self.project_path):
                dirs[:] = [d for d in dirs if d not in ignored_dirs]
                for file in files:
                    _, ext = os.path.splitext(file)
                    if ext.lower() in supported_exts:
                        rel_path = os.path.relpath(os.path.join(root, file), self.project_path).replace("\\", "/")
                        files_to_process.append(rel_path)
        except Exception as e:
            return f"Error walking directory: {e}"

        # Deterministic path order — cache-stable bytes across generations
        files_to_process.sort()
        files_to_process = files_to_process[:max_files]

        map_lines: List[str] = []
        for rel_path in files_to_process:
            _, ext = os.path.splitext(rel_path)
            abs_path = os.path.join(self.project_path, rel_path)
            signatures = get_file_signatures(abs_path, ext.lower(), is_hot=False, full_depth=True)
            if not signatures:
                continue

            entries: List[Tuple[str, int, bool]] = []
            for sig, depth, _doc in signatures:
                kind = _classify_sig(sig)
                if kind in ("class", "interface"):
                    entries.append((_compact_class(sig, kind), depth, True))
                elif kind == "type_alias":
                    entries.append((_compact_class(sig, "type_alias"), depth, False))
                elif kind == "function":
                    entries.append((_compact_fn(sig), depth, False))
                else:
                    entries.append((_compact_property(sig), depth, False))
            if not entries:
                continue
            map_lines.append(f"{rel_path}: {_format_compact(entries)}")

        if not map_lines:
            return "(No classes or functions found in workspace files)"
        return "\n".join(map_lines).strip()
