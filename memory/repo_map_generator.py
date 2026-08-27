"""
Repository Map Generator — Compiles codebases into a token-efficient signature map.
Uses tree-sitter AST parsing for Python, PHP, TypeScript, and JavaScript.
Falls back to robust regex signature extraction for Dart and parse failures.
"""
import itertools
import os
import re
import subprocess
import sys
from typing import Dict, List, Optional, Set, Tuple

# Reconfigure console output encoding to UTF-8
try:
    sys.stdout.reconfigure(encoding='utf-8')
except Exception:
    pass

# Try importing tree_sitter
try:
    import tree_sitter_languages as tsl
    from tree_sitter import Node, Parser
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
# The full map is CACHED (byte-stable per project) so it must be:
#   * deterministic — bytes depend ONLY on the files (no task keywords)
#   * compact       — max navigation value per token
#   * replaceable   — a checkpoint replaces the one canonical snapshot from the
#     current codebase; transient projectMap+ events never enter this file


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


# Generic low-architecture-value helpers (imported everywhere, say little
# about where to edit) get their ranking score multiplied by
# MAP_UTILITY_PENALTY. Matched on stem segments (token boundaries) so real
# logic modules like "chat_context" never match while "db_config",
# "sync_helpers" or "app_constants" always do. A file also counts as a
# utility when a WHOLE parent directory is a generic bucket ("src/types/",
# "lib/utils/"), catching type barrels like "types/index.ts".
_UTILITY_STEM_RE = re.compile(
    r"(?:^|_)(?:utils?|helpers?|loggers?|logging|constants?|consts?|"
    r"settings?|configurations?|config|types?)(?:_|$)"
)
_UTILITY_SEGMENT_RE = re.compile(
    r"^(?:utils?|helpers?|loggers?|logging|constants?|consts?|"
    r"settings?|configurations?|config|types?)$"
)


def _is_utility_file(path: str) -> bool:
    name = os.path.basename(path).lower()
    if name.startswith("__init__."):
        return True
    stem = os.path.splitext(name)[0]
    if _UTILITY_STEM_RE.search(stem):
        return True
    segments = [
        seg for seg in os.path.dirname(path).replace("\\", "/").split("/") if seg
    ]
    return any(_UTILITY_SEGMENT_RE.match(seg) for seg in segments)


# Delimiter injected into `git log --pretty=format:` output so per-commit
# file lists can be split deterministically while parsing co-change history.
_GIT_COMMIT_SEP = "@@C@@"


class RepoMapGenerator:
    """Build a small plain-English navigation guide from the import graph.

    The map is intentionally not a source listing. It describes major areas,
    a handful of high-value files, and direct relationships between files.
    Live files and search_code remain authoritative.

    Key files are ranked by PageRank over the import graph with generic
    utilities demoted; full snapshots may additionally carry git co-change
    hints. Both features are deterministic for a given repository state, so
    the map stays byte-stable per project. Nothing here is keyed to specific
    project names: ranking uses graph structure only.
    """

    # Cross-ecosystem conventional entry-point basenames (stem only, any
    # supported extension). Used ONLY as an eligibility fallback so a
    # conventional-but-unconnected entry file can occupy a Key-files slot;
    # ranking itself never sees names.
    ENTRY_STEMS = frozenset({"__main__", "main", "app", "server", "index"})

    IGNORED_DIRS = {
        # Dependency / vendor
        "node_modules", "vendor", "bower_components", "venv", ".venv", "venv312",
        # VCS / tooling / IDE
        ".git", "__pycache__", ".deep_agents", ".claude", ".pytest_cache",
        ".vscode", ".idea", ".antigravity", ".agents", "skills", "scratch",
        # Build output
        "dist", "build", "out", "obj", "target", ".next", ".nuxt",
        ".svelte-kit", ".parcel-cache", "storybook-static", ".turbo",
        # Temp / cache / logs / coverage / reports
        "logs", "log", "tmp", "temp", "coverage", ".cache", ".gradle",
        ".terraform", ".dart_tool", "test-results", "playwright-report",
        "storage", "public",
    }
    SUPPORTED_EXTS = {".py", ".php", ".ts", ".tsx", ".js", ".jsx", ".dart"}
    MAP_MARKER = "projectMap:"

    def __init__(self, project_path: str) -> None:
        self.project_path = os.path.normpath(os.path.abspath(project_path))
        self.max_chars = max(2000, int(os.environ.get("MAP_MAX_CHARS", "6000")))

    def _files(self) -> list[str]:
        files: list[str] = []
        for root, dirs, names in os.walk(self.project_path):
            dirs[:] = sorted(d for d in dirs if d not in self.IGNORED_DIRS)
            for name in names:
                if os.path.splitext(name)[1].lower() not in self.SUPPORTED_EXTS:
                    continue
                path = os.path.relpath(os.path.join(root, name), self.project_path)
                files.append(path.replace("\\", "/"))
        return sorted(files)[:int(os.environ.get("MAP_MAX_FILES", "600"))]

    def _imports(self, rel_path: str) -> list[str]:
        try:
            with open(os.path.join(self.project_path, rel_path), "r", encoding="utf-8", errors="replace") as f:
                source = f.read()
        except Exception:
            return []

        targets: list[str] = []
        # Python module imports.
        targets.extend(re.findall(r"(?m)^\s*from\s+([A-Za-z_][\w.]*)\s+import\s", source))
        targets.extend(re.findall(r"(?m)^\s*import\s+([A-Za-z_][\w.]*)", source))
        # Relative JS/TS/PHP/Dart imports and includes.
        targets.extend(re.findall(
            r"(?:from|import|require|include|include_once)\s*(?:\([^)]*)?\s*['\"]([^'\"]+)['\"]",
            source,
        ))
        return list(dict.fromkeys(targets))

    @staticmethod
    def _candidate_paths(source: str, target: str) -> list[str]:
        target = target.replace("\\", "/")
        if target.startswith("."):
            base = os.path.dirname(source).replace("\\", "/")
            stem = os.path.normpath(os.path.join(base, target)).replace("\\", "/")
        elif re.match(r"^[A-Za-z_][\w.]*$", target):
            stem = target.replace(".", "/")
        else:
            return []
        ext = os.path.splitext(stem)[1].lower()
        candidates = [stem]
        if not ext:
            candidates.extend(stem + e for e in (".py", ".ts", ".tsx", ".js", ".jsx", ".php", ".dart"))
        candidates.extend(os.path.join(stem, "__init__.py").replace("\\", "/") for _ in [0])
        candidates.extend(os.path.join(stem, "index" + e).replace("\\", "/") for e in (".ts", ".tsx", ".js", ".jsx"))
        return candidates

    def _graph(self, files: list[str]) -> dict[str, set[str]]:
        known = set(files)
        graph = {path: set() for path in files}
        for source in files:
            for target in self._imports(source):
                for candidate in self._candidate_paths(source, target):
                    if candidate in known and candidate != source:
                        graph[source].add(candidate)
                        break
        return graph

    def topology_snapshot(self) -> dict[str, list[str]]:
        """Return the current file-relation graph for change detection.

        The project map is a navigation graph, so body-only edits must not
        create map updates. A snapshot changes only when a supported source
        file appears/disappears or its resolved outgoing relationships change.
        """
        files = self._files()
        graph = self._graph(files)
        return {path: sorted(graph.get(path, set())) for path in files}

    def generate_delta(self, changes: dict[str, list[str]]) -> str:
        """Describe changed topology as compact current adjacency lines."""
        files = self._files()
        graph = self._graph(files)
        current = set(files)
        lines: list[str] = []
        for status, prefix in (("added", "+"), ("modified", "~"), ("removed", "-")):
            for path in sorted(changes.get(status, [])):
                if status == "removed" or path not in current:
                    lines.append(f"{prefix} {path} -> none [removed]")
                    continue
                links = sorted(graph.get(path, set()))[:6]
                targets = ", ".join(links) if links else "none"
                lines.append(f"{prefix} {path} -> {targets}")
        return "\n".join(lines)

    def _pagerank_scores(self, files: list[str], graph: dict[str, set[str]]) -> dict[str, float]:
        """PageRank over the import graph with generic utilities demoted.

        Deterministic: fixed damping/iterations/convergence, sorted node
        order, uniform reset mass. No name-based seeding — entry points and
        coordinators win on graph structure alone (they import many modules
        and are imported by launchers/tests). Generic helpers are multiplied
        by MAP_UTILITY_PENALTY (default 0.4; 1 disables demotion) because
        near-universal inbound imports otherwise elect logger/config/utils
        files that carry little architectural signal. The multiplier biases
        ORDERING only; MAP_MAX_UTILITY_FILES (see generate_map) is the hard
        guarantee that utilities cannot crowd out logic hubs.
        """
        nodes = sorted(files)
        n = len(nodes)
        if not n:
            return {}
        index = {p: i for i, p in enumerate(nodes)}
        out_links = [[index[t] for t in sorted(graph.get(p, ()))] for p in nodes]
        try:
            penalty = max(0.0, min(1.0, float(os.environ.get("MAP_UTILITY_PENALTY", "0.4"))))
        except ValueError:
            penalty = 0.4

        reset = [1.0 / n] * n

        ranks = [1.0 / n] * n
        for _ in range(50):
            dangling = sum(ranks[i] for i in range(n) if not out_links[i])
            nxt = [(1 - 0.85) * reset[j] + 0.85 * dangling / n for j in range(n)]
            for i in range(n):
                targets = out_links[i]
                if targets:
                    share = 0.85 * ranks[i] / len(targets)
                    for j in targets:
                        nxt[j] += share
            delta = sum(abs(a - b) for a, b in zip(nxt, ranks))
            ranks = nxt
            if delta < 1e-9:
                break

        def _weight(path: str) -> float:
            name = os.path.basename(path).lower()
            # Package markers (__init__) and nested barrels (index.*) are
            # pure plumbing: near-universal inbound imports with zero logic,
            # so they never compete for a Key-file slot. A ROOT-level
            # index.* survives — it is often the runtime entry point.
            if name.startswith("__init__."):
                return 0.0
            if os.path.splitext(name)[0] == "index" and "/" in path:
                return 0.0
            return penalty if _is_utility_file(path) else 1.0

        return {p: ranks[index[p]] * _weight(p) for p in nodes}

    def _cochange_pairs(self, files: list[str]) -> list[tuple[str, str]]:
        """Git co-change pairs as raw (a, b) tuples.

        Counts file pairs committed together over the last
        MAP_COCHANGE_COMMITS commits (default 200); pairs changed together in
        at least MAP_COCHANGE_MIN_COUNT commits (default 2 — young repos have
        short histories; raise it to demand stronger evidence), capped at
        MAP_COCHANGE_MAX lines (default 4). Any failure — missing .git,
        missing git binary, timeout, non-zero exit — degrades silently to []
        so the map stays purely import-derived. Computed only on full-map
        builds: topology snapshots and deltas intentionally never contain
        these lines, so mid-session commits cannot emit map events.
        """
        try:
            limit = int(os.environ.get("MAP_COCHANGE_COMMITS", "200"))
            min_count = int(os.environ.get("MAP_COCHANGE_MIN_COUNT", "2"))
            max_pairs = int(os.environ.get("MAP_COCHANGE_MAX", "4"))
        except ValueError:
            limit, min_count, max_pairs = 200, 2, 4
        try:
            proc = subprocess.run(
                ["git", "log", f"-n{max(1, limit)}", "--name-only",
                 "--pretty=format:" + _GIT_COMMIT_SEP],
                cwd=self.project_path,
                capture_output=True, text=True, errors="replace", timeout=10,
                check=False,
            )
        except Exception:
            return []
        if proc.returncode != 0 or not proc.stdout:
            return []

        known = set(files)
        counts: dict[tuple[str, str], int] = {}

        def flush(block: set[str]) -> None:
            paths = {p.strip().replace("\\", "/") for p in block}
            paths &= known
            # Skip oversized commits (bulk imports/merges co-change nothing).
            if 2 <= len(paths) <= 50:
                for a, b in itertools.combinations(sorted(paths), 2):
                    counts[(a, b)] = counts.get((a, b), 0) + 1

        block: set[str] = set()
        marker = _GIT_COMMIT_SEP
        for line in proc.stdout.splitlines():
            stripped = line.strip()
            if stripped == marker:
                flush(block)
                block = set()
            elif stripped:
                block.add(stripped)
        flush(block)

        ranked = sorted(
            ((count, pair) for pair, count in counts.items() if count >= max(1, min_count)),
            key=lambda item: (-item[0], item[1]),
        )
        return [pair for _, pair in ranked[:max(0, max_pairs)]]

    @staticmethod
    def _role(path: str) -> str:
        lower = path.lower()
        name = os.path.basename(lower)
        stem = os.path.splitext(name)[0]
        # Only root-level conventional names count as THE application entry
        # point; a nested "index.ts" is just some module's barrel.
        if stem == "__main__" or ("/" not in path and stem in RepoMapGenerator.ENTRY_STEMS):
            return "entry point"
        if "/tests/" in f"/{lower}" or "/test/" in f"/{lower}":
            return "tests"
        if "/tools/" in f"/{lower}":
            return "tools"
        if "/templates/" in f"/{lower}":
            return "templates"
        if "/agents/" in f"/{lower}":
            return "agents"
        return "source module"

    def _bounded(self, lines: list[str]) -> str:
        out: list[str] = []
        size = 0
        for line in lines:
            addition = len(line) + (1 if out else 0)
            if size + addition > self.max_chars - 36:
                omitted = len(lines) - len(out)
                out.append(f"[truncated: {omitted} lines omitted; use search_code]")
                break
            out.append(line)
            size += addition
        return "\n".join(out)

    @staticmethod
    def _min_unique_refs(paths: set[str]) -> dict[str, str]:
        """Smallest root-relative tail per path that stays unambiguous.

        Used only for cross-scope references. The grammar is exact: `./x`
        means a file directly in the repo root, so ONLY single-segment paths
        earn the "./" prefix; deeper files always carry at least one real
        directory segment (`tools/x.py`, never `./x.py`). Tails grow until
        unique across the whole reference vocabulary; full paths are unique
        by construction, so this always terminates.
        """
        ordered = sorted(paths)
        max_depth = max((len(p.split("/")) for p in ordered), default=0)
        tables = []
        for k in range(1, max_depth + 1):
            counts: dict[str, int] = {}
            for p in ordered:
                tail = "/".join(p.split("/")[-k:])
                counts[tail] = counts.get(tail, 0) + 1
            tables.append(counts)
        refs: dict[str, str] = {}
        for p in ordered:
            parts = p.split("/")
            # A deep file must keep at least one directory segment; a bare
            # basename with "./" is reserved for true repo-root files.
            min_k = 1 if len(parts) == 1 else 2
            for k in range(min_k, len(parts) + 1):
                tail = "/".join(parts[-k:])
                if tables[k - 1][tail] == 1:
                    refs[p] = f"./{tail}" if len(parts) == 1 else tail
                    break
        return refs

    @staticmethod
    def _group_edges(
        items: list[tuple[str, str, list[str]]], cross_refs: dict[str, str]
    ) -> list[str]:
        """Render edges grouped under `<dir>/` scope headers.

        A bare filename lives inside the current scope; a token containing
        `/` or starting with `./` is repo-root-relative. Repo-root files form
        the implicit root scope and get no header. Every directory pays its
        header once — all its edges merge at the scope's first appearance in
        the ranked stream, so priority survives truncation. Grammar is
        project-agnostic: extensions are kept so same-stem files in
        different languages never collide.
        """
        scoped: dict[str, list[tuple[str, str, list[str]]]] = {}
        order: list[str] = []
        for primary, arrow, secondaries in items:
            scope = os.path.dirname(primary)
            if scope not in scoped:
                scoped[scope] = []
                order.append(scope)
            scoped[scope].append((primary, arrow, secondaries))
        out: list[str] = []
        for scope in order:
            if scope:
                out.append(f"{scope}/")
            for primary, arrow, secondaries in scoped[scope]:
                refs = []
                for sec in secondaries:
                    if os.path.dirname(sec) == scope:
                        refs.append(os.path.basename(sec))
                    else:
                        refs.append(cross_refs.get(sec, sec))
                # Defensive: an empty ref can only mean a bug upstream —
                # drop it rather than emit a dangling comma.
                refs = [r for r in refs if r]
                out.append(f"{os.path.basename(primary)}{arrow}{','.join(refs)}")
        return out

    def generate_map(self) -> str:
        """Return a bounded, deterministic relationship-oriented map."""
        try:
            files = self._files()
        except Exception as e:
            return f"{self.MAP_MARKER}\nUnable to scan workspace: {e}"
        if not files:
            return f"{self.MAP_MARKER}\nNo supported source modules found."

        graph = self._graph(files)
        incoming = {path: 0 for path in files}
        for targets in graph.values():
            for target in targets:
                incoming[target] += 1

        scores = self._pagerank_scores(files, graph)

        # Areas carry PageRank mass from CONNECTED files only: a directory
        # full of disconnected scripts contributes nothing an agent can
        # navigate, so it sinks below the display cut without name rules.
        area_mass: dict[str, float] = {}
        for path in files:
            if not (incoming[path] or graph[path]):
                continue
            area = path.split("/", 1)[0] if "/" in path else "root"
            area_mass[area] = area_mass.get(area, 0.0) + scores.get(path, 0.0)
        ranked = sorted(files, key=lambda p: (-scores.get(p, 0.0), p))

        def _entry_like(path: str) -> bool:
            stem = os.path.splitext(os.path.basename(path))[0].lower()
            return stem in self.ENTRY_STEMS

        # Hard guarantee on top of the score multiplier: however extreme the
        # fan-in, generic utilities can never hold more than
        # MAP_MAX_UTILITY_FILES of the 12 Key-files slots.
        try:
            max_utilities = max(0, int(os.environ.get("MAP_MAX_UTILITY_FILES", "3")))
        except ValueError:
            max_utilities = 3
        eligible = [p for p in ranked if incoming[p] or graph[p] or _entry_like(p)]
        key_files: list[str] = []
        utilities = 0
        for path in eligible:
            if _is_utility_file(path):
                if utilities >= max_utilities:
                    continue
                utilities += 1
            key_files.append(path)
            if len(key_files) == 12:
                break
        if not key_files:
            key_files = files[:12]

        top_areas = sorted(area_mass.items(), key=lambda item: (-item[1], item[0]))[:8]
        key_set = set(key_files)

        def _area_of(path: str) -> str:
            return path.split("/", 1)[0] if "/" in path else "root"

        # Connections: spend slots on navigation value, not alphabetical
        # luck. Sources lead with hub files, then edges crossing area
        # boundaries (an agent can guess co-located siblings; boundary edges
        # are what a map exists for), then PageRank. Targets lead with hub
        # files so the strongest architectural chains appear first.
        def _source_rank(source: str) -> tuple:
            cross = sum(1 for t in graph[source] if _area_of(t) != _area_of(source))
            return (
                0 if source in key_set else 1,
                -cross,
                -scores.get(source, 0.0),
                source,
            )

        def _target_rank(target: str) -> tuple:
            return (0 if target in key_set else 1, -scores.get(target, 0.0), target)

        ranked_sources = sorted(
            (source for source in graph if graph[source]), key=_source_rank
        )[:24]

        conn_items = [
            (source, ">", sorted(graph[source], key=_target_rank)[:6])
            for source in ranked_sources
        ]
        co_items = [
            (left, "<->", [right]) for left, right in self._cochange_pairs(files)
        ]

        # Cross-scope references share one minimized vocabulary so a file is
        # spelled identically everywhere it appears in the map.
        cross_vocab: set[str] = set()
        for items in (co_items, conn_items):
            for primary, _, secondaries in items:
                scope_dir = os.path.dirname(primary)
                cross_vocab.update(
                    s for s in secondaries if os.path.dirname(s) != scope_dir
                )
        cross_refs = self._min_unique_refs(cross_vocab)

        body: list[str] = ["Areas: " + "; ".join(area for area, _ in top_areas), "Key files:"]
        for path in key_files:
            role = self._role(path)
            # The default label carries no signal — omit it entirely.
            body.append(f"- {path}" + (f" — {role}" if role != "source module" else ""))
        if co_items:
            body.append("Co-change hints:")
            body.extend(self._group_edges(co_items, cross_refs))
        body.append("Connections:")
        if conn_items:
            body.extend(self._group_edges(conn_items, cross_refs))
        else:
            body.append("No internal connections; use search_code.")
        return self._bounded([self.MAP_MARKER, *body])
