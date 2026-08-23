"""MainTool.editFile — the edit_file pipeline and its helpers.

Holds the edit_file tool and its pipeline: edit_file (single replace /
batch SEARCH/REPLACE / create-overwrite via _write_file), the batch-diff
machinery (_apply_diff, apply_search_replace, apply_unified_diff), and the
edit-result snippet builder (_edit_snippet). tools.py state helpers
(permissions, VFS, sanitization, cache) are imported lazily inside the
functions to avoid an import cycle.
"""
import os
import re
from ._shared import _log_tool


def _edit_snippet(file_path: str, before: str, after: str, context: int = 5) -> str:
    """Return the NEW file content around the first changed region (±context
    lines, with line numbers) so the model sees its edit in context without
    re-reading the file (the post-edit re-read loop killer)."""
    try:
        import difflib
        b = (before or "").splitlines()
        a = (after or "").splitlines()
        sm = difflib.SequenceMatcher(None, b, a)
        changed = [(i1, i2, j1, j2) for tag, i1, i2, j1, j2 in sm.get_opcodes() if tag != "equal"]
        if not changed:
            return ""
        _, _, j1, j2 = changed[0]
        lo = max(0, j1 - context)
        hi = min(len(a), j2 + context)
        out = []
        if lo > 0:
            out.append(f"... ({lo} lines above, unchanged) ...")
        out += [f"{i + 1}: {l}" for i, l in enumerate(a[lo:hi], start=lo)]
        if hi < len(a):
            out.append(f"... ({len(a) - hi} lines below, unchanged) ...")
        return "```\n" + "\n".join(out) + "\n```"
    except Exception:
        return ""


def apply_search_replace(content: str, diff: str) -> tuple[str, list[str]]:
    # Normalize line endings to \n for internal processing
    normalized_content = content.replace('\r\n', '\n')
    normalized_diff = diff.replace('\r\n', '\n')
    
    pattern = re.compile(
        r'<<<<<<< SEARCH\n(.*?)\n=======\n(.*?)\n>>>>>>> REPLACE',
        re.DOTALL
    )
    blocks = pattern.findall(normalized_diff)
    if not blocks:
        return "", ["No SEARCH/REPLACE blocks found in diff. Use <<<<<<< SEARCH\\n[old]\\n=======\\n[new]\\n>>>>>>> REPLACE format."]
        
    errors = []
    for old, new in blocks:
        count = normalized_content.count(old)
        if count == 0:
            # Try fuzzy match fallback: ignore trailing whitespace and normalize line endings
            old_lines = [line.rstrip() for line in old.split('\n')]
            content_lines = [line.rstrip() for line in normalized_content.split('\n')]
            
            # Find sequence of lines in content_lines that match old_lines
            match_index = -1
            match_count = 0
            n_old = len(old_lines)
            n_content = len(content_lines)
            
            for start_idx in range(n_content - n_old + 1):
                window = content_lines[start_idx : start_idx + n_old]
                if all(w == o for w, o in zip(window, old_lines)):
                    match_index = start_idx
                    match_count += 1
                    
            if match_count == 1:
                # Reconstruct the matched block from the original content to replace it
                orig_lines_all = normalized_content.split('\n')
                original_matched_lines = orig_lines_all[match_index : match_index + n_old]
                original_matched_block = '\n'.join(original_matched_lines)
                
                # Indentation adjustment
                first_matched_line = orig_lines_all[match_index]
                first_old_line = old.split('\n')[0] if old.split('\n') else ""
                
                matched_indent = len(first_matched_line) - len(first_matched_line.lstrip())
                old_indent = len(first_old_line) - len(first_old_line.lstrip())
                
                indent_diff = matched_indent - old_indent
                adjusted_new = new
                if indent_diff != 0:
                    new_lines_adjusted = []
                    for new_line in new.split('\n'):
                        if new_line.strip():
                            if indent_diff > 0:
                                new_lines_adjusted.append(' ' * indent_diff + new_line)
                            else:
                                leading_spaces = len(new_line) - len(new_line.lstrip())
                                strip_len = min(abs(indent_diff), leading_spaces)
                                new_lines_adjusted.append(new_line[strip_len:])
                        else:
                            new_lines_adjusted.append(new_line)
                    adjusted_new = '\n'.join(new_lines_adjusted)
                    
                normalized_content = normalized_content.replace(original_matched_block, adjusted_new)
            else:
                if match_count > 1:
                    errors.append(f"Fuzzy match found multiple matches ({match_count}) in file. Please make the search block more specific.")
                else:
                    errors.append(f"Search block not found in file. Ensure exact match including indentation:\n{old}")
        elif count > 1:
            errors.append(f"Search block matches multiple times ({count}) in file. Make the search block more specific.")
        else:
            normalized_content = normalized_content.replace(old, new)
            
    if errors:
        return "", errors
        
    if '\r\n' in content:
        final_content = normalized_content.replace('\n', '\r\n')
    else:
        final_content = normalized_content
    return final_content, []


def apply_unified_diff(content: str, diff: str) -> tuple[str, list[str]]:
    lines = content.replace('\r\n', '\n').split('\n')
    diff_lines = diff.replace('\r\n', '\n').split('\n')
    
    hunks = []
    current_hunk = None
    
    for line in diff_lines:
        if line.startswith('@@'):
            if current_hunk:
                hunks.append(current_hunk)
            m = re.match(r'@@ -(\d+),?(\d+)? \+(\d+),?(\d+)? @@', line)
            if m:
                old_start = int(m.group(1))
                current_hunk = {'old_start': old_start, 'lines': []}
            else:
                current_hunk = None
        elif current_hunk is not None:
            if line.startswith('-') or line.startswith('+') or line.startswith(' '):
                current_hunk['lines'].append(line)
                
    if current_hunk:
        hunks.append(current_hunk)
        
    if not hunks:
        return "", ["No unified diff hunks found (missing @@ header or matching lines)."]
        
    hunks.sort(key=lambda h: h['old_start'], reverse=True)
    
    errors = []
    new_lines = list(lines)
    for hunk in hunks:
        start_idx = hunk['old_start'] - 1
        hunk_old_lines = [l[1:] for l in hunk['lines'] if l.startswith('-') or l.startswith(' ')]
        hunk_new_lines = [l[1:] for l in hunk['lines'] if l.startswith('+') or l.startswith(' ')]
        
        match = True
        for i, old_line in enumerate(hunk_old_lines):
            target_idx = start_idx + i
            if target_idx >= len(new_lines) or new_lines[target_idx] != old_line:
                match = False
                break
                
        if not match:
            found = False
            for offset in range(-10, 11):
                test_idx = start_idx + offset
                if test_idx < 0:
                    continue
                match = True
                for i, old_line in enumerate(hunk_old_lines):
                    target_idx = test_idx + i
                    if target_idx >= len(new_lines) or new_lines[target_idx] != old_line:
                        match = False
                        break
                if match:
                    start_idx = test_idx
                    found = True
                    break
            if not found:
                errors.append(f"Hunk starting at line {hunk['old_start']} failed to match context lines.")
                continue
                
        new_lines[start_idx : start_idx + len(hunk_old_lines)] = hunk_new_lines
        
    if errors:
        return "", errors
    return '\n'.join(new_lines), []


def _apply_diff(file_path: str, diff: str) -> str:
    """Apply a unified diff or search-replace block to a file."""
    from tools import _sanitize_path, _invalidate_file_cache
    path = _sanitize_path(file_path)
    if not os.path.isfile(path):
        return f"Error: File not found: {file_path}"
        
    try:
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            content = f.read()
            
        if "<<<<<<< SEARCH" in diff:
            new_content, errors = apply_search_replace(content, diff)
        else:
            new_content, errors = apply_unified_diff(content, diff)
            
        if errors:
            return f"Error: Failed to apply diff to {file_path}:\n" + "\n".join(errors)

        with open(path, "w", encoding="utf-8") as f:
            f.write(new_content)
        _invalidate_file_cache(path)
            
        lines = new_content.count("\n") + 1
        output = f"[OK] Applied diff to {file_path} ({lines} lines)"
            
        _log_tool("apply_diff", {"file_path": file_path, "diff": diff}, output)
        return output
    except Exception as e:
        return f"Error applying diff to {file_path}: {e}"


_PATCH_MARKER_RE = re.compile(r'^\*\*\* (Add File|Update File|Delete File|Move to): (.*?)\s*$')
_PATCH_KIND_MAP = {"Add File": "add", "Update File": "update", "Delete File": "delete"}


def _parse_patch_sections(diff: str):
    """Split a patch into (kind, path, body) sections.

    Supports opencode-style markers ('*** Add File: p', '*** Update File: p',
    '*** Delete File: p', '*** Move to: p') and standard 'diff -u' headers
    ('--- a/p' / '+++ b/p'). A 'Move to' marker attaches to the preceding
    Update File section (kind 'move', body = (dest, hunks)). Returns [] when
    the diff carries no file headers (plain single-file unified diff or
    SEARCH/REPLACE body).
    """
    lines = diff.replace("\r\n", "\n").split("\n")
    sections = []
    cur_kind = None
    cur_path = None
    cur_body = []
    move_dest = None

    def flush():
        nonlocal cur_kind, cur_path, cur_body, move_dest
        if cur_kind is None:
            return
        body = "\n".join(cur_body).rstrip("\n")
        if cur_kind == "update" and move_dest:
            sections.append(("move", cur_path, (move_dest, body)))
        elif cur_kind == "update" and not body.strip():
            sections.append(("noop", cur_path, ""))
        else:
            sections.append((cur_kind, cur_path, body))
        cur_kind = None
        cur_path = None
        cur_body = []
        move_dest = None

    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        m = _PATCH_MARKER_RE.match(line)
        if m:
            kind, path = m.group(1), m.group(2).strip()
            if kind == "Move to":
                move_dest = path
            else:
                flush()
                cur_kind = _PATCH_KIND_MAP[kind]
                cur_path = path
            i += 1
            continue
        if line.startswith("--- ") and i + 1 < n and lines[i + 1].startswith("+++ "):
            flush()
            new_path = lines[i + 1][4:].strip()
            if new_path.startswith("a/") or new_path.startswith("b/"):
                new_path = new_path[2:]
            cur_kind = "update"
            cur_path = new_path
            i += 2
            continue
        if cur_kind is not None:
            cur_body.append(line)
        i += 1
    flush()
    return sections


def _apply_multi_file_patch(diff: str) -> str:
    """Apply a multi-file patch (opencode-style markers or diff -u headers)."""
    from tools import enforce_fs_permission, _check_and_request_approval, _sanitize_path, _invalidate_file_cache, _increment_cache_version
    sections = _parse_patch_sections(diff)
    if not sections:
        return ("Error: No patch file headers found. Use '*** Add File: path', '*** Update File: path', "
                "'*** Delete File: path', or '--- a/path' / '+++ b/path' headers.")
    _increment_cache_version()
    results = []
    for kind, raw_path, body in sections:
        try:
            if kind == "noop":
                results.append(f"[SKIP] {raw_path}: no hunks in section")
                continue
            path = _sanitize_path(raw_path)
            if kind == "add":
                enforce_fs_permission("write", path)
                _check_and_request_approval(path, "write")
                os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
                with open(path, "w", encoding="utf-8") as f:
                    f.write(body)
                _invalidate_file_cache(path)
                results.append(f"[OK] Added {raw_path} ({body.count(chr(10)) + 1} lines)")
            elif kind == "delete":
                enforce_fs_permission("write", path)
                _check_and_request_approval(path, "edit")
                if not os.path.isfile(path):
                    results.append(f"[ERROR] Delete failed: file not found: {raw_path}")
                    continue
                os.remove(path)
                _invalidate_file_cache(path)
                results.append(f"[OK] Deleted {raw_path}")
            elif kind == "move":
                dest_raw, hunk_body = body
                dest = _sanitize_path(dest_raw)
                enforce_fs_permission("write", path)
                enforce_fs_permission("write", dest)
                _check_and_request_approval(dest, "edit")
                if not os.path.isfile(path):
                    results.append(f"[ERROR] Move failed: file not found: {raw_path}")
                    continue
                with open(path, "r", encoding="utf-8", errors="replace") as f:
                    content = f.read()
                if hunk_body.strip():
                    if "<<<<<<< SEARCH" in hunk_body:
                        content, errors = apply_search_replace(content, hunk_body)
                    else:
                        content, errors = apply_unified_diff(content, hunk_body)
                    if errors:
                        results.append(f"[ERROR] Move {raw_path} -> {dest_raw}:\n" + "\n".join(errors))
                        continue
                os.makedirs(os.path.dirname(dest) or ".", exist_ok=True)
                with open(dest, "w", encoding="utf-8") as f:
                    f.write(content)
                os.remove(path)
                _invalidate_file_cache(path)
                _invalidate_file_cache(dest)
                results.append(f"[OK] Moved {raw_path} -> {dest_raw}")
            else:  # update
                enforce_fs_permission("write", path)
                _check_and_request_approval(path, "edit")
                if not os.path.isfile(path):
                    results.append(f"[ERROR] Update failed: file not found: {raw_path}")
                    continue
                with open(path, "r", encoding="utf-8", errors="replace") as f:
                    content = f.read()
                if "<<<<<<< SEARCH" in body:
                    new_content, errors = apply_search_replace(content, body)
                else:
                    new_content, errors = apply_unified_diff(content, body)
                if errors:
                    results.append(f"[ERROR] Update failed for {raw_path}:\n" + "\n".join(errors))
                    continue
                with open(path, "w", encoding="utf-8") as f:
                    f.write(new_content)
                _invalidate_file_cache(path)
                lines = new_content.count("\n") + 1
                results.append(f"[OK] Updated {raw_path} ({lines} lines)")
        except Exception as e:
            results.append(f"[ERROR] {raw_path}: {e}")
    output = "\n".join(results)
    _log_tool("apply_diff", {"file_path": "multi-file patch", "diff": diff}, output[:500])
    return output


def _write_file(file_path: str, content: str) -> str:
    """Write content to a file, creating parent directories if needed."""
    from tools import enforce_fs_permission, _increment_cache_version, _check_and_request_approval, shared_state, WORKSPACE, get_vfs_router, _sanitize_path, _invalidate_file_cache
    enforce_fs_permission("write", file_path)
    _increment_cache_version()
    _check_and_request_approval(file_path, "write")

    # â”€â”€ Resolve the actual write target â”€â”€
    norm_path = os.path.normpath(os.path.abspath(file_path))

    # Direct filesystem write for absolute paths within the active workspace.
    # Bypasses VFS routing which strips path prefixes and loses the root directory.
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    if norm_path.lower().startswith(active_ws.lower()):
        os.makedirs(os.path.dirname(norm_path) or ".", exist_ok=True)
        try:
            with open(norm_path, "w", encoding="utf-8") as f:
                f.write(content)
            _invalidate_file_cache(norm_path)
            lines = content.count("\n") + 1
            output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            _log_tool("write_file", {"file_path": file_path, "content": content}, output)
            return output
        except Exception as e:
            return f"Error writing {file_path}: {e}"

    # â”€â”€ VFS Composite Routing (for virtual paths like /workspace/, /scratch/) â”€â”€
    vfs_path = file_path.replace("\\", "/")
    try:
        vfs_router = get_vfs_router()
        res = vfs_router.write(vfs_path, content)
        if not res.error:
            lines = content.count("\n") + 1
            output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            _log_tool("write_file", {"file_path": file_path, "content": content}, output)
            return output
    except Exception:
        pass

    path = _sanitize_path(file_path)
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    try:
        with open(path, "w", encoding="utf-8") as f:
            f.write(content)
        _invalidate_file_cache(path)
        lines = content.count("\n") + 1
        output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            
        _log_tool("write_file", {"file_path": file_path, "content": content}, output)
        return output
    except Exception as e:
        return f"Error writing {file_path}: {e}"


def _run_verify(verify: str, file_path: str, diff: str = None) -> str:
    """Run verify command after edit. Returns [VERIFY] block or empty."""
    if not verify or not str(verify).strip():
        return ""
    v = str(verify).strip()
    import subprocess, os, sys
    try:
        if v == "py_compile":
            targets = []
            if diff and _parse_patch_sections(diff):
                for kind, raw_path, body in _parse_patch_sections(diff):
                    if kind in ("add", "update", "move"):
                        # For move, body is (dest, hunks), raw_path is old, dest is new
                        if kind == "move" and isinstance(body, tuple):
                            targets.append(body[0])
                        else:
                            targets.append(raw_path)
            else:
                targets = [file_path]
            out_lines = []
            for t in targets:
                if not t or not t.strip():
                    continue
                # Only py_compile .py files
                if not t.strip().endswith(".py"):
                    continue
                cmd = [sys.executable, "-m", "py_compile", t.strip()]
                r = subprocess.run(cmd, capture_output=True, text=True, timeout=10)
                if r.returncode == 0:
                    out_lines.append(f"  {t}: OK")
                else:
                    out_lines.append(f"  {t}: FAIL\n{r.stderr.strip()[:500] or r.stdout.strip()[:500]}")
            if not out_lines:
                return "\n[VERIFY py_compile] No .py targets"
            status = "passed" if all("OK" in l for l in out_lines) else "failed"
            return f"\n[VERIFY py_compile {status}]\n" + "\n".join(out_lines)
        elif v in ("tsc", "tsc --noEmit", "tsc --noEmit --skipLibCheck"):
            cmd = v if v.startswith("tsc") else "tsc --noEmit"
            # Prefer npx tsc if available
            shell_cmd = f"npx {cmd}" if "npx" not in cmd else cmd
            r = subprocess.run(shell_cmd, shell=True, capture_output=True, text=True, timeout=30)
            if r.returncode == 0:
                return f"\n[VERIFY {v} passed]"
            return f"\n[VERIFY {v} failed]\n{r.stdout.strip()[:1000]}\n{r.stderr.strip()[:1000]}"
        else:
            # Custom shell command
            r = subprocess.run(v, shell=True, capture_output=True, text=True, timeout=30)
            out = (r.stdout.strip() + "\n" + r.stderr.strip()).strip()[:1500]
            status = "passed" if r.returncode == 0 else f"failed (exit {r.returncode})"
            return f"\n[VERIFY {v} {status}]\n{out}" if out else f"\n[VERIFY {v} {status}]"
    except Exception as e:
        return f"\n[VERIFY {v} error: {e}]"


def edit_file(file_path: str, old_string: str = None, new_string: str = None,
              diff: str = None, verify: str = None) -> str:
    """Edit via single search/replace, batch SEARCH/REPLACE blocks, unified diff, or multi-file patch.

    verify is OPT-IN: pass "py_compile", "tsc", or a shell command to run a
    check after a successful write. Omitted = no verification (the DONE-gate
    nudge and the CI gate own verification).
    """
    from tools import enforce_fs_permission, _increment_cache_version, _check_and_request_approval, shared_state, WORKSPACE, get_vfs_router, _sanitize_path, _invalidate_file_cache

    enforce_fs_permission("write", file_path)
    _increment_cache_version()
    _check_and_request_approval(file_path, "edit")
    
    # Route to batch mode if diff is provided
    if diff:
        if _parse_patch_sections(diff):
            res = _apply_multi_file_patch(diff)
            if verify and res.startswith("[OK]"):
                res += _run_verify(verify, file_path, diff)
            return res
        res = _apply_diff(file_path, diff)
        if verify and res.startswith("[OK]"):
            res += _run_verify(verify, file_path, diff)
        return res
        
    # Single edit mode — require both old_string and new_string
    if not old_string or new_string is None:
        # Create / full-overwrite mode: old_string omitted, new_string = full content.
        # (Empty-string old_string does NOT count — guards against accidental overwrites.)
        if old_string is None and new_string is not None:
            res = _write_file(file_path, new_string)
            if verify and res.startswith("[OK]"):
                res += _run_verify(verify, file_path)
            return res
        return "Error: edit_file requires (old_string + new_string) for single edit, (diff) for batch SEARCH/REPLACE blocks, or (new_string only) to create/overwrite a file."
    if old_string == new_string:
        return "Error: old_string and new_string are identical."

    # â”€â”€ Direct filesystem edit for absolute workspace paths â”€â”€
    norm_path = os.path.normpath(os.path.abspath(file_path))
    active_ws = shared_state.get("project_path") or WORKSPACE
    active_ws = os.path.normpath(os.path.abspath(active_ws))
    if norm_path.lower().startswith(active_ws.lower()):
        path = norm_path
    else:
        # â”€â”€ VFS Composite Routing (for virtual paths) â”€â”€
        vfs_path = file_path.replace("\\", "/")
        try:
            vfs_router = get_vfs_router()
            res = vfs_router.edit(vfs_path, old_string, new_string)
            if not res.error:
                output = f"[OK] Edited {file_path} successfully"
                _log_tool("edit_file", {"file_path": file_path, "old_string": old_string, "new_string": new_string}, output)
                return output
        except Exception:
            pass
        path = _sanitize_path(file_path)
    if not os.path.isfile(path):
        return f"Error: File not found: {file_path}"

    try:
        with open(path, "r", encoding="utf-8") as f:
            content = f.read()
        if old_string not in content:
            # Try fuzzy match fallback: ignore trailing whitespace and normalize line endings
            normalized_content = content.replace('\r\n', '\n')
            normalized_old = old_string.replace('\r\n', '\n')
            normalized_new = new_string.replace('\r\n', '\n')
            
            old_lines = [line.rstrip() for line in normalized_old.split('\n')]
            content_lines = [line.rstrip() for line in normalized_content.split('\n')]
            
            match_index = -1
            match_count = 0
            n_old = len(old_lines)
            n_content = len(content_lines)
            
            for start_idx in range(n_content - n_old + 1):
                window = content_lines[start_idx : start_idx + n_old]
                if all(w == o for w, o in zip(window, old_lines)):
                    match_index = start_idx
                    match_count += 1
                    
            if match_count == 1:
                # We found exactly one match!
                orig_lines_all = normalized_content.split('\n')
                original_matched_lines = orig_lines_all[match_index : match_index + n_old]
                original_matched_block = '\n'.join(original_matched_lines)
                
                # Indentation adjustment
                first_matched_line = orig_lines_all[match_index]
                first_old_line = normalized_old.split('\n')[0] if normalized_old.split('\n') else ""
                
                matched_indent = len(first_matched_line) - len(first_matched_line.lstrip())
                old_indent = len(first_old_line) - len(first_old_line.lstrip())
                
                indent_diff = matched_indent - old_indent
                adjusted_new = normalized_new
                if indent_diff != 0:
                    new_lines_adjusted = []
                    for new_line in normalized_new.split('\n'):
                        if new_line.strip():
                            if indent_diff > 0:
                                new_lines_adjusted.append(' ' * indent_diff + new_line)
                            else:
                                leading_spaces = len(new_line) - len(new_line.lstrip())
                                strip_len = min(abs(indent_diff), leading_spaces)
                                new_lines_adjusted.append(new_line[strip_len:])
                        else:
                            new_lines_adjusted.append(new_line)
                    adjusted_new = '\n'.join(new_lines_adjusted)
                    
                new_content = normalized_content.replace(original_matched_block, adjusted_new)
                if '\r\n' in content:
                    new_content = new_content.replace('\n', '\r\n')
            else:
                if match_count > 1:
                    return f"Error: Fuzzy match found multiple matches ({match_count}) in {file_path}. Make old_string more specific."
                else:
                    # M1 edit-failure contract: give the model a precise on-disk
                    # anchor (line range + nearest snippet) so it can retry blind
                    # with exact text instead of guessing.
                    content_lines_all = normalized_content.split('\n')
                    total_lines = len(content_lines_all)
                    if match_index >= 0:
                        anchor_line_number = match_index + 1
                        anchor_line = max(0, match_index - 1)
                    else:
                        # no fuzzy match either — anchor at the file head
                        anchor_line_number = 1
                        anchor_line = 0
                    anchor_snippet = "\n".join(content_lines_all[anchor_line:anchor_line + 5])[:400]
                    return (
                        f"Error: old_string not found in {file_path}. The text must match exactly.\n"
                        f"[HINT] File has {total_lines} lines. Nearest anchor: line {anchor_line_number}. "
                        f"On-disk content around line {anchor_line_number}:\n"
                        f"```\n{anchor_snippet}\n```\n"
                        f"Read the file with read_file(file_path, offset={max(1, anchor_line_number - 10)}, limit=30) "
                        f"to get the exact lines, then retry with the verbatim text."
                    )
        else:
            if old_string == new_string:
                return f"Error: old_string and new_string are identical."
            count = content.count(old_string)
            if count > 1:
                return f"Error: old_string matches {count} times in {file_path}. To prevent unintended edits, please include more surrounding context lines in old_string to make it unique."
            new_content = content.replace(old_string, new_string, 1)  # Replace first occurrence only
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_content)
        _invalidate_file_cache(path)
        
        # Rich result: show the edited region (±10 lines) so the model never
        # needs to re-read the file to "see" its edit (the re-read loop killer).
        edit_note = _edit_snippet(file_path, content, new_content)
        output = f"[OK] Edited {file_path} (1 replacement)\n{edit_note}"
        if verify:
            output += _run_verify(verify, file_path)
            
        _log_tool("edit_file", {"file_path": file_path, "old_string": old_string, "new_string": new_string}, output)
        return output
    except Exception as e:
        return f"Error editing {file_path}: {e}"
