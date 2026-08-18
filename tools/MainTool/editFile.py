"""MainTool.editFile — the edit_file pipeline and its helpers.

Holds the edit_file tool and its pipeline: edit_file (single replace /
batch SEARCH/REPLACE / create-overwrite via _write_file), the batch-diff
machinery (_apply_diff, apply_search_replace, apply_unified_diff), and the
edit-result snippet builder (_edit_snippet). tools.py state helpers
(permissions, VFS, sanitize, snapshot, cache) are imported lazily inside the
functions to avoid an import cycle.
"""
import os
import re
from ._shared import _log_tool, _validate_syntax


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
    from tools import _sanitize_path, _snapshot_file, _invalidate_file_cache
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

        _snapshot_file(path)  # C11: undo primitive
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_content)
        _invalidate_file_cache(path)
            
        # Validate syntax
        syntax_err = _validate_syntax(path)
        lines = new_content.count("\n") + 1
        if syntax_err:
            output = f"[WARNING] Applied diff to {file_path} successfully, but syntax checks failed:\n{syntax_err}"
        else:
            output = f"[OK] Applied diff to {file_path} ({lines} lines)"
            
        _log_tool("apply_diff", {"file_path": file_path, "diff": diff}, output)
        return output
    except Exception as e:
        return f"Error applying diff to {file_path}: {e}"


def _write_file(file_path: str, content: str) -> str:
    """Write content to a file, creating parent directories if needed."""
    from tools import enforce_fs_permission, _increment_cache_version, _check_and_request_approval, shared_state, WORKSPACE, get_vfs_router, _sanitize_path, _snapshot_file, _invalidate_file_cache
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
            _snapshot_file(norm_path)  # C11: undo primitive
            with open(norm_path, "w", encoding="utf-8") as f:
                f.write(content)
            _invalidate_file_cache(norm_path)
            lines = content.count("\n") + 1
            syntax_err = _validate_syntax(norm_path)
            if syntax_err:
                output = f"[WARNING] Wrote {file_path} ({lines} lines, {len(content)} chars) but syntax checks failed:\n{syntax_err}"
            else:
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
        _snapshot_file(path)  # C11: undo primitive
        with open(path, "w", encoding="utf-8") as f:
            f.write(content)
        _invalidate_file_cache(path)
        lines = content.count("\n") + 1
        
        # Validate syntax
        syntax_err = _validate_syntax(path)
        if syntax_err:
            output = f"[WARNING] Wrote {file_path} ({lines} lines, {len(content)} chars) successfully, but syntax checks failed:\n{syntax_err}"
        else:
            output = f"[OK] Wrote {file_path} ({lines} lines, {len(content)} chars)"
            
        _log_tool("write_file", {"file_path": file_path, "content": content}, output)
        return output
    except Exception as e:
        return f"Error writing {file_path}: {e}"


def edit_file(file_path: str, old_string: str = None, new_string: str = None, diff: str = None) -> str:
    """Edit a file via single search/replace OR batch SEARCH/REPLACE blocks."""
    from tools import enforce_fs_permission, _increment_cache_version, _check_and_request_approval, shared_state, WORKSPACE, get_vfs_router, _sanitize_path, _snapshot_file, _invalidate_file_cache
    enforce_fs_permission("write", file_path)
    _increment_cache_version()
    _check_and_request_approval(file_path, "edit")
    
    # Route to batch mode if diff is provided
    if diff:
        return _apply_diff(file_path, diff)
        
    # Single edit mode — require both old_string and new_string
    if not old_string or new_string is None:
        # Create / full-overwrite mode: old_string omitted, new_string = full content.
        # (Empty-string old_string does NOT count — guards against accidental overwrites.)
        if old_string is None and new_string is not None:
            return _write_file(file_path, new_string)
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
        _snapshot_file(path)  # C11: undo primitive
        with open(path, "w", encoding="utf-8") as f:
            f.write(new_content)
        _invalidate_file_cache(path)
        
        # Validate syntax
        syntax_err = _validate_syntax(path)
        # Rich result: show the edited region (±10 lines) so the model never
        # needs to re-read the file to "see" its edit (the re-read loop killer).
        edit_note = _edit_snippet(file_path, content, new_content)
        if syntax_err:
            output = f"[WARNING] Edited {file_path} (1 replacement) successfully, but syntax checks failed:\n{syntax_err}"
        else:
            output = f"[OK] Edited {file_path} (1 replacement)\n{edit_note}"
            
        _log_tool("edit_file", {"file_path": file_path, "old_string": old_string, "new_string": new_string}, output)
        return output
    except Exception as e:
        return f"Error editing {file_path}: {e}"
