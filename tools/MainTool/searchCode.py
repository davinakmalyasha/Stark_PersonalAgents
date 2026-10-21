"""MainTool.searchCode — the search_code tool.

Regex content search with three answer styles (lines / files / count),
filename search, and ranked multi-pattern search.

Behavior:
- Binary files are skipped (sniffed, not guessed by extension).
- Matching runs on a bounded line slice; display lines are trimmed.
- Paths in results are workspace-relative.
- Content results are bounded: max_matches shown inline (default 100), the
  full collected list is written to /scratch/large_tool_results/ with a
  pointer when the shown budget is exceeded.
- Results annotate the enclosing symbol by default; snippet="function"
  returns the enclosing function body (deduped per function).
- Multi-pattern search scans the tree once (not once per pattern).

Shared walk/binary/symbol helpers live in `_filewalk.py` and `_symbols.py`.
tools.py state helpers (shared_state, WORKSPACE, enforce_fs_permission,
_sanitize_path) are imported lazily inside the functions to avoid an import
cycle.
"""
import glob as globmod
import os
import re
from collections import deque
from ._filewalk import (
    IGNORED_DIRS,
    LINE_SCAN_CHARS,
    LINE_SHOW_CHARS,
    display_path,
    glob_ok,
    iter_files,
    open_text,
    parse_globs,
    resolve_root,
)
from ._shared import _log_tool
from ._symbols import annotation_for, enclosing, span_lines

_MATCHES_SHOWN = 100
_MATCHES_COLLECT = 1000
_FILES_LIMIT = 1000
_MATCHES_PER_FILE = 25
_CONTEXT_MAX = 5
_SNIPPET_MAX_LINES = 40
_SNIPPET_MAX_CHARS = 2000


def _build_matcher(pattern: str, ignore_case: bool, word: bool, fixed_strings: bool):
    """Return match_fn(probe)->bool for one pattern."""
    if fixed_strings and not word:
        needle = pattern.lower() if ignore_case else pattern
        if ignore_case:
            return lambda s, n=needle: n in s.lower()
        return lambda s, n=needle: n in s
    src = re.escape(pattern) if fixed_strings else pattern
    if word:
        src = rf"(?<!\w)(?:{src})(?!\w)"
    flags = re.IGNORECASE if ignore_case else 0
    regex = re.compile(src, flags)
    return lambda s, rx=regex: rx.search(s) is not None


def _scan_lines(path: str, match_fn, context: int, per_file_cap: int, collect_left: int):
    """Scan one file for matches with optional context.

    Returns (records, match_count, per_file_stopped) where each record is
    ``(line_no, text, is_match)``.
    """
    records = []
    matches = 0
    per_file_stopped = False
    prev = deque(maxlen=context) if context else None
    after_left = 0
    last_emitted = 0
    line_no = 0
    handle = open_text(path)
    if handle is None:
        return records, 0, False
    try:
        for line in handle:
            line_no += 1
            if per_file_cap and matches >= per_file_cap:
                per_file_stopped = True
                break
            if collect_left and matches >= collect_left:
                break
            body = line.rstrip("\r\n")
            shown = body[:LINE_SHOW_CHARS]
            is_match = bool(match_fn(body[:LINE_SCAN_CHARS]))
            if is_match:
                matches += 1
                if prev is not None:
                    for ln, text in prev:
                        if ln > last_emitted:
                            records.append((ln, text, False))
                            last_emitted = ln
                records.append((line_no, shown, True))
                last_emitted = line_no
                after_left = context
            elif after_left > 0:
                if line_no > last_emitted:
                    records.append((line_no, shown, False))
                    last_emitted = line_no
                after_left -= 1
            if prev is not None:
                prev.append((line_no, shown))
    finally:
        try:
            handle.close()
        except Exception:
            pass
    return records, matches, per_file_stopped


def _scan_count(path: str, match_fn) -> int:
    count = 0
    handle = open_text(path)
    if handle is None:
        return 0
    try:
        for line in handle:
            if match_fn(line.rstrip("\r\n")[:LINE_SCAN_CHARS]):
                count += 1
    finally:
        try:
            handle.close()
        except Exception:
            pass
    return count


def _match_prefix(fpath: str, line_no: int, annotate: bool) -> str:
    if not annotate:
        return ""
    ann = annotation_for(fpath, line_no)
    return ann + " " if ann else ""


def _format_record(fpath: str, line_no: int, text: str, is_match: bool, base: str,
                   annotate: bool) -> str:
    rel = display_path(fpath, base)
    sep = ":" if is_match else "-"
    prefix = _match_prefix(fpath, line_no, annotate) if is_match else ""
    return f"{rel}{sep}{line_no}{sep} {prefix}{text}"


def _limit_records(records: list, show: int) -> list:
    """Slice records to the first `show` matches, keeping trailing context."""
    out = []
    seen = 0
    i = 0
    while i < len(records):
        out.append(records[i])
        if records[i][3]:
            seen += 1
            if seen >= show:
                j = i + 1
                while j < len(records) and not records[j][3]:
                    out.append(records[j])
                    j += 1
                break
        i += 1
    return out


def _render_snippets(records: list, base: str, annotate: bool, limit_matches: int):
    """Function-body rendering: one body per function, matches marked inside.

    Returns (lines, emitted_match_count).
    """
    by_file = {}
    order = []
    text_by_line = {}
    for fpath, line_no, text, is_match in records:
        if not is_match:
            continue
        text_by_line[(fpath, line_no)] = text
        if fpath not in by_file:
            by_file[fpath] = []
            order.append(fpath)
        by_file[fpath].append(line_no)

    out = []
    emitted = 0
    for fpath in order:
        if emitted >= limit_matches:
            break
        rel = display_path(fpath, base)
        match_set = set(by_file[fpath])
        emitted_spans = set()
        for line_no in by_file[fpath]:
            if emitted >= limit_matches:
                break
            sym = enclosing(fpath, line_no)
            if sym is None:
                prefix = _match_prefix(fpath, line_no, annotate)
                out.append(f"{rel}:{line_no}: {prefix}{text_by_line.get((fpath, line_no), '')}")
                emitted += 1
                continue
            span_key = (sym.get("start"), sym.get("end"))
            if span_key in emitted_spans:
                continue
            emitted_spans.add(span_key)
            body, truncated = span_lines(
                fpath, sym.get("start") or line_no, sym.get("end") or line_no,
                _SNIPPET_MAX_LINES, _SNIPPET_MAX_CHARS,
            )
            if not body:
                prefix = _match_prefix(fpath, line_no, annotate)
                out.append(f"{rel}:{line_no}: {prefix}{text_by_line.get((fpath, line_no), '')}")
                emitted += 1
                continue
            for ln, text in body:
                is_match = ln in match_set
                sep = ":" if is_match else "-"
                prefix = _match_prefix(fpath, ln, annotate) if is_match else ""
                out.append(f"{rel}{sep}{ln}{sep} {prefix}{text}")
                if is_match:
                    emitted += 1
            if truncated:
                out.append(f"{rel}-{sym.get('end')}- ... [function continues: {truncated} more lines]")
    return out, emitted


def _offload_match_list(lines: list) -> str:
    """Persist the full match list to VFS scratch; '' when the write fails."""
    try:
        from tools._runtime import _write_offload_file
        return _write_offload_file("\n".join(lines), "search_code")
    except Exception:
        return ""


def _find_filenames(pattern: str, search_root: str, include: list, exclude: list,
                    single_file: str = None) -> list:
    """Filename search: glob or substring, honoring include/exclude filters."""
    pattern = pattern.replace("\\", "/")
    has_glob = any(ch in pattern for ch in "*?[")
    found = []
    try:
        if single_file:
            name = os.path.basename(single_file)
            if has_glob:
                if globmod.fnmatch.fnmatch(name, os.path.basename(pattern)):
                    found.append(single_file)
            elif pattern.lower() in name.lower():
                found.append(single_file)
            return found
        if has_glob:
            full_pattern = os.path.join(search_root, pattern)
            for p in globmod.glob(full_pattern, recursive=True):
                if not os.path.isfile(p):
                    continue
                rel_parts = os.path.relpath(p, search_root).replace("\\", "/").split("/")
                if any(seg in IGNORED_DIRS for seg in rel_parts[:-1]):
                    continue
                rel = os.path.relpath(p, search_root)
                if not glob_ok(os.path.basename(p), rel, include, exclude):
                    continue
                found.append(p)
                if len(found) >= _FILES_LIMIT:
                    break
        else:
            needle = pattern.lower()
            for p in iter_files(search_root, include, exclude):
                name = os.path.basename(p)
                rel = os.path.relpath(p, search_root)
                if needle in name.lower() or needle in rel.replace("\\", "/").lower():
                    found.append(p)
                    if len(found) >= _FILES_LIMIT:
                        break
        found.sort()
    except Exception:
        pass
    return found


def _resolve_root(path: str, ctx):
    """Backward-compatible alias for the shared resolver."""
    return resolve_root(path, ctx)


def search_code(ctx: object = None, pattern: str = "", patterns: list = None, path: str = "",
                glob: str = "", mode: str = "content", output: str = "lines",
                ignore_case: bool = False, context: int = 0,
                max_matches: int = _MATCHES_SHOWN, annotate: bool = True,
                snippet: str = "line", smart_case: bool = False, word: bool = False,
                fixed_strings: bool = False) -> str:
    """Search code: regex content search ('content') or filename search ('files').

    Content answers come in three styles via ``output``: 'lines' (matching
    lines), 'files' (matching paths), 'count' (matches per file). Matching
    lines are annotated with their enclosing symbol unless ``annotate=False``;
    ``snippet="function"`` returns the enclosing function body instead of the
    single line. Multi-pattern ranked search via ``patterns=[...]``. ``ctx`` is
    the optional typed WorkspaceDeps (registry passes it when the signature
    starts with ctx/deps/run_ctx); legacy callers omit it.
    """
    norm_mode = str(mode or "content").strip().lower()
    norm_output = str(output or "lines").strip().lower()
    if norm_output not in ("lines", "files", "count"):
        norm_output = "lines"
    snippet_mode = str(snippet or "line").strip().lower()
    if snippet_mode not in ("line", "function"):
        snippet_mode = "line"
    try:
        ctx_n = int(context or 0)
    except Exception:
        ctx_n = 0
    ctx_n = max(0, min(ctx_n, _CONTEXT_MAX))
    try:
        show = int(max_matches or _MATCHES_SHOWN)
    except Exception:
        show = _MATCHES_SHOWN
    show = max(1, min(show, _MATCHES_COLLECT))

    use_ignore_case = bool(ignore_case)
    if not use_ignore_case and smart_case:
        try:
            probe = str(pattern or "")
            if probe and probe.lower() == probe:
                use_ignore_case = True
        except Exception:
            pass

    # Multi-pattern ranked search
    if patterns is not None and isinstance(patterns, list) and any(
        isinstance(p, str) and p.strip() for p in patterns
    ):
        clean_patterns = [str(p).strip() for p in patterns if isinstance(p, str) and str(p).strip()]
        flags = re.IGNORECASE if use_ignore_case else 0
        return _search_multi(clean_patterns, path, glob, norm_mode, ctx, flags)
    if patterns is not None and isinstance(patterns, str) and patterns.strip():
        return search_code(ctx=ctx, pattern=patterns.strip(), path=path, glob=glob,
                           mode=norm_mode, output=norm_output, ignore_case=use_ignore_case,
                           context=ctx_n, max_matches=show, annotate=annotate,
                           snippet=snippet_mode, smart_case=False, word=word,
                           fixed_strings=fixed_strings)
    if not pattern or not str(pattern).strip():
        return "Error: search_code requires pattern or patterns"
    pattern = str(pattern).strip()

    root, single_file, base, error = _resolve_root(path, ctx)
    if error:
        return error
    include, exclude = parse_globs(glob)

    if norm_mode == "files":
        found = _find_filenames(pattern, root, include, exclude, single_file)
        if not found:
            return f"No files found matching: {pattern}"
        limit_reached = len(found) >= _FILES_LIMIT
        label = f"{len(found)}+" if limit_reached else str(len(found))
        output_str = f"Found {label} files:\n" + "\n".join(
            display_path(p, base) for p in found
        )
        if limit_reached:
            output_str += f"\n(limit {_FILES_LIMIT} reached — narrow with path/glob)"
        _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob, "mode": norm_mode},
                  output_str[:500])
        return output_str

    try:
        match_fn = _build_matcher(pattern, use_ignore_case, bool(word), bool(fixed_strings))
    except re.error as e:
        return f"Error: Invalid regex pattern: {e}"

    if norm_output == "count":
        items = []
        total = 0
        for fpath in iter_files(root, include, exclude, single_file):
            count = _scan_count(fpath, match_fn)
            if count:
                items.append((fpath, count))
                total += count
        if not items:
            return f"No matches found for pattern: {pattern}"
        items.sort(key=lambda kv: (-kv[1], display_path(kv[0], base)))
        shown, overflow = items[:_FILES_LIMIT], items[_FILES_LIMIT:]
        file_unit = "file" if len(items) == 1 else "files"
        header = f"Found {len(items)} {file_unit}, {_matches_label(total)}:"
        pointer = ""
        if overflow:
            full_path = _offload_match_list(
                [f"{display_path(p, base)}: {c}" for p, c in items]
            )
            pointer = f" (showing first {_FILES_LIMIT}; full list: {full_path})" if full_path \
                else f" (showing first {_FILES_LIMIT})"
        output_str = header + pointer + "\n" + "\n".join(
            f"{display_path(p, base)}: {c}" for p, c in shown
        )
        _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob, "mode": norm_mode},
                  output_str[:500])
        return output_str

    if norm_output == "files":
        items = []
        total = 0
        for fpath in iter_files(root, include, exclude, single_file):
            count = _scan_count(fpath, match_fn)
            if count:
                items.append(fpath)
                total += count
        if not items:
            return f"No matches found for pattern: {pattern}"
        limit_reached = len(items) >= _FILES_LIMIT
        shown = items[:_FILES_LIMIT]
        label = f"{len(items)}+" if limit_reached else str(len(items))
        pointer = ""
        if limit_reached:
            full_path = _offload_match_list([display_path(p, base) for p in items])
            pointer = f" (full list: {full_path})" if full_path else ""
        file_unit = "file" if len(items) == 1 else "files"
        output_str = (f"Found {label} {file_unit} containing matches "
                      f"({_matches_label(total)}){pointer}:\n"
                      + "\n".join(display_path(p, base) for p in shown))
        _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob, "mode": norm_mode},
                  output_str[:500])
        return output_str

    # output == "lines"
    scan_context = 0 if snippet_mode == "function" else ctx_n
    records = []
    total_matches = 0
    limit_reached = False
    per_file_stopped = []
    for fpath in iter_files(root, include, exclude, single_file):
        collect_left = _MATCHES_COLLECT - total_matches
        file_records, matches, stopped = _scan_lines(
            fpath, match_fn, scan_context, _MATCHES_PER_FILE, collect_left
        )
        if stopped:
            per_file_stopped.append(fpath)
        records.extend((fpath, ln, text, is_match) for ln, text, is_match in file_records)
        total_matches += matches
        if total_matches >= _MATCHES_COLLECT:
            limit_reached = True
            break

    if not records:
        return f"No matches found for pattern: {pattern}"

    total_label = f"{total_matches}+" if limit_reached else str(total_matches)
    unit = "match" if total_matches == 1 else "matches"
    overflow = total_matches > show
    pointer = ""
    if overflow:
        full_path = _offload_match_list(
            [_format_record(*r, base, False) for r in records]
        )
        pointer = f"; full list: {full_path}" if full_path else ""

    if snippet_mode == "function":
        if overflow:
            shown_lines, emitted = _render_snippets(records, base, annotate, show)
            header = (f"Found {total_label} {unit} (showing first {emitted}{pointer} — "
                      f"narrow with path/glob):")
        else:
            shown_lines, emitted = _render_snippets(records, base, annotate, total_matches)
            header = f"Found {total_label} {unit}:"
        output_str = header + "\n" + "\n".join(shown_lines)
    else:
        if overflow:
            shown_records = _limit_records(records, show)
            header = (f"Found {total_label} {unit} (showing first {show}{pointer} — "
                      f"narrow with path/glob):")
        else:
            shown_records = records
            header = f"Found {total_label} {unit}:"
        output_str = header + "\n" + "\n".join(
            _format_record(*r, base, annotate) for r in shown_records
        )

    if per_file_stopped:
        names = ", ".join(display_path(p, base) for p in per_file_stopped[:3])
        more = f" (+{len(per_file_stopped) - 3} more)" if len(per_file_stopped) > 3 else ""
        output_str += (f"\n[per-file limit {_MATCHES_PER_FILE} reached in: {names}{more} — "
                       f"narrow the pattern or search those files directly]")
    _log_tool("search_code", {"pattern": pattern, "path": path, "glob": glob, "mode": norm_mode},
              output_str[:500])
    return output_str


def _matches_label(n: int) -> str:
    return f"{n} match" if n == 1 else f"{n} matches"


def _search_multi(patterns: list, path: str = "", glob: str = "", mode: str = "content",
                  ctx: object = None, flags: int = 0) -> str:
    """Ranked multi-pattern search: files ranked by patterns matched, then hits.

    Content mode scans the tree once for all patterns; files mode ranks
    filename matches per pattern.
    """
    root, single_file, base, error = _resolve_root(path, ctx)
    if error:
        return error
    include, exclude = parse_globs(glob)

    if mode == "files":
        file_scores = {}
        file_why = {}
        for pat in patterns:
            for p in _find_filenames(pat, root, include, exclude, single_file):
                rel = display_path(p, base)
                file_scores[rel] = file_scores.get(rel, 0) + 10
                file_why.setdefault(rel, []).append(pat)
        if not file_scores:
            return f"No files found for patterns: {', '.join(patterns)}"
        ranked = sorted(file_scores.items(), key=lambda x: (-x[1], x[0]))[:20]
        out = f"Found {len(ranked)} files ranked for {len(patterns)} patterns:\n"
        for rel, score in ranked:
            why = ", ".join(file_why[rel][:3])
            out += f"  {rel} [score:{score} why:{why}]\n"
        _log_tool("search_code", {"patterns": patterns, "path": path, "mode": mode}, out[:500])
        return out.strip()

    compiled = []
    for pat in patterns:
        try:
            compiled.append((pat, re.compile(pat, flags)))
        except re.error:
            continue
    if not compiled:
        return f"Error: Invalid regex patterns: {', '.join(patterns)}"

    file_hits = {}
    for fpath in iter_files(root, include, exclude, single_file):
        handle = open_text(fpath)
        if handle is None:
            continue
        try:
            for i, line in enumerate(handle, 1):
                body = line.rstrip("\r\n")
                probe = body[:LINE_SCAN_CHARS]
                hits_here = [pat for pat, rx in compiled if rx.search(probe)]
                if hits_here:
                    rec = file_hits.setdefault(
                        fpath, {"count": 0, "patterns": set(), "lines": []}
                    )
                    rec["count"] += 1
                    for pat in hits_here:
                        rec["patterns"].add(pat)
                    if len(rec["lines"]) < 3:
                        rec["lines"].append(f"{i}: {body[:LINE_SHOW_CHARS]}")
        finally:
            try:
                handle.close()
            except Exception:
                pass

    if not file_hits:
        return f"No matches found for patterns: {', '.join(patterns)}"
    ranked = sorted(
        file_hits.items(),
        key=lambda kv: (-len(kv[1]["patterns"]), -kv[1]["count"], display_path(kv[0], base)),
    )[:15]
    out = f"Found {len(file_hits)} files ranked for {len(patterns)} patterns:\n"
    for fpath, info in ranked:
        rel = display_path(fpath, base)
        why = ", ".join(sorted(info["patterns"])[:3])
        out += f"\n{rel} [hits:{info['count']} patterns:{len(info['patterns'])} why:{why}]\n"
        for ln in info["lines"][:2]:
            out += f"  {ln}\n"
    _log_tool("search_code", {"patterns": patterns, "path": path, "mode": mode}, out[:500])
    return out.strip()
