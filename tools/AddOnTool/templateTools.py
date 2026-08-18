"""AddOnTool template rendering, catalog, and synchronization tools.

Template state is local to this module. Workspace, cache, and GitHub helpers
are imported lazily inside functions through the tools.py facade.
"""
import json
import os
import re
import time
from typing import Optional

# This module lives two package levels below the legacy tools.py facade.
_TEMPLATES_DIR = os.path.join(
    os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__)))),
    "templates",
)

_SYNC_STAMP = ".sync.json"

_TEMPLATE_CATEGORIES = ("stack", "feature", "component", "snippet", "project",
                        "config", "routine", "spec")

def _template_dir(name: str) -> Optional[str]:
    """Locate a template folder (one level deep: templates/<level>/<name>/)."""
    name = os.path.basename(str(name))
    for entry in sorted(os.listdir(_TEMPLATES_DIR)):
        if entry in (".sync.json", "README.md") or not os.path.isdir(os.path.join(_TEMPLATES_DIR, entry)):
            continue
        candidate = os.path.join(_TEMPLATES_DIR, entry, name)
        if os.path.isfile(os.path.join(candidate, "template.json")):
            return candidate
    return None

def _template_meta(name: str) -> Optional[dict]:
    tdir = _template_dir(name)
    if tdir is None:
        return None
    meta_path = os.path.join(tdir, "template.json")
    try:
        with open(meta_path, encoding="utf-8") as f:
            return json.load(f)
    except Exception as e:
        return {"error": str(e)}

def _list_template_names() -> list:
    """List all template names across level folders (templates/<level>/<name>/)."""
    if not os.path.isdir(_TEMPLATES_DIR):
        return []
    names = []
    for entry in sorted(os.listdir(_TEMPLATES_DIR)):
        level_dir = os.path.join(_TEMPLATES_DIR, entry)
        if entry in (".sync.json", "README.md") or not os.path.isdir(level_dir):
            continue
        for name in sorted(os.listdir(level_dir)):
            if os.path.isfile(os.path.join(level_dir, name, "template.json")):
                names.append(name)
    return names

_env_cache = {}

def _render_env(delims: Optional[list] = None):
    from jinja2 import StrictUndefined
    from jinja2.sandbox import SandboxedEnvironment
    key = tuple(delims or ())
    env = _env_cache.get(key)
    if env is None:
        env = SandboxedEnvironment(
            keep_trailing_newline=True,
            trim_blocks=True,
            lstrip_blocks=True,
            undefined=StrictUndefined,
            variable_start_string=(delims[0] if delims else "{{"),
            variable_end_string=(delims[1] if delims and len(delims) > 1 else "}}"),
            block_start_string=(delims[2] if delims and len(delims) > 2 else "{%"),
            block_end_string=(delims[3] if delims and len(delims) > 3 else "%}"),
        )
        _env_cache[key] = env
    return env

def _render_str(text: str, variables: dict, delims: Optional[list] = None) -> str:
    """Render a Jinja2 string with optional custom delimiters (sandboxed)."""
    return _render_env(delims).from_string(text).render(**variables)

def _has_template_tokens(text: str, delims: Optional[list]) -> bool:
    """True if `text` contains template tokens under the given delimiter set."""
    if delims and len(delims) >= 2:
        if delims[0] in text:
            return True
        return len(delims) > 2 and delims[2] in text
    return "{{" in text or "{%" in text

def _coerce_variable(name: str, declared_type: str, value, errors: list):
    """Coerce a template variable to its declared type; record errors on failure."""
    declared_type = (declared_type or "string").lower()
    if declared_type == "string":
        return value if isinstance(value, str) else str(value)
    if declared_type == "int":
        if isinstance(value, bool):
            value = int(value)
        if isinstance(value, int):
            return value
        if isinstance(value, str):
            try:
                return int(value.strip())
            except ValueError:
                pass
        errors.append(f"variable '{name}' expects int, got {value!r}")
        return None
    if declared_type == "bool":
        if isinstance(value, bool):
            return value
        if isinstance(value, (int, float)):
            return bool(value)
        if isinstance(value, str):
            low = value.strip().lower()
            if low in ("1", "true", "yes", "on"):
                return True
            if low in ("0", "false", "no", "off"):
                return False
        errors.append(f"variable '{name}' expects bool, got {value!r}")
        return None
    if declared_type == "list":
        if isinstance(value, list):
            return value
        if isinstance(value, str):
            return [p.strip() for p in value.split(",") if p.strip()]
        errors.append(f"variable '{name}' expects list, got {value!r}")
        return None
    return value

def _fuzzy_template_hint(name: str) -> str:
    """Cheap error hint for unknown template names (never dumps the full catalog)."""
    import difflib
    matches = difflib.get_close_matches(str(name), _list_template_names(), n=5, cutoff=0.4)
    if matches:
        return f" — did you mean: {', '.join(matches)}?"
    return f" — no close match; try template_list(query='{name}') to search the catalog."

def _safe_dest_path(target_dir: str, dest_rel: str) -> Optional[str]:
    """Resolve dest_rel under target_dir; None if it escapes (traversal/absolute)."""
    root = os.path.abspath(target_dir)
    joined = os.path.abspath(os.path.join(target_dir, dest_rel))
    if joined == root or not joined.startswith(root + os.sep):
        return None
    return joined

def apply_template(template: str, target_dir: str = "", variables: dict = None,
                   overwrite: bool = False, dry_run: bool = False,
                   quiet: bool = True) -> str:
    """Render a template into target_dir (default: current workspace).

    Stacks (templates with "uses") recursively apply their referenced
    templates first (in order), then their own files. Cycle-safe, deduped.

    SAFETY: two-phase — every file renders BEFORE anything is written, so a
    mid-stack failure leaves nothing partially applied. Existing files are
    SKIPPED by default (re-applies are safe); pass overwrite=True to replace
    them (or set "if_exists": "overwrite|skip|error" per file in metadata).

    dry_run=True renders everything and reports what WOULD be written.

    quiet=True (default) returns a compact summary: composed count + written/
    skipped/overwritten file lists + errors. quiet=False adds the per-template
    nesting, TODO markers and conventions notes (for stacks, ~2-3K tokens).

    Returns a compact summary: composed templates, files written/skipped,
    TODO markers, conventions notes.
    """
    from tools import _HARNESS_ROOT, enforce_fs_permission, shared_state
    if not target_dir:
        target_dir = str(shared_state.get("project_path") or _HARNESS_ROOT)
    target_dir = os.path.normpath(str(target_dir))
    if not os.path.isdir(target_dir):
        return f"Error: target_dir '{target_dir}' does not exist."
    plan, visited, warnings, errors = [], set(), [], []
    lines = _apply_template_inner(template, dict(variables or {}), plan, visited, 0,
                                  warnings, errors)
    body = "\n".join(lines)
    if errors:
        return (f"Error: template '{template}' not applied — {len(errors)} problem(s).\n"
                f"{body}\n" + "\n".join(errors[:8]))

    # ── Conflict detection within one composition (same dest, two templates) ──
    by_dest, final_plan = {}, []
    for entry in plan:
        if entry["dest_rel"] in by_dest:
            warnings.append(f"! conflict: '{entry['dest_rel']}' written by "
                            f"'{by_dest[entry['dest_rel']]}' and '{entry['template']}' — second skipped")
            continue
        by_dest[entry["dest_rel"]] = entry["template"]
        final_plan.append(entry)

    # ── Commit phase (only after the WHOLE composition rendered) ──
    created, skipped, overwritten, todos = [], [], [], []
    for entry in final_plan:
        dest_path = _safe_dest_path(target_dir, entry["dest_rel"])
        if dest_path is None:
            errors.append(f"! unsafe dest '{entry['dest_rel']}' (escapes {target_dir})")
            continue
        exists = os.path.exists(dest_path)
        mode = entry.get("if_exists") or ("overwrite" if overwrite else "skip")
        if exists and mode == "error":
            errors.append(f"! {entry['dest_rel']} already exists (if_exists=error)")
            continue
        if exists and mode == "skip":
            skipped.append(entry["dest_rel"])
            continue
        todos += entry["todos"]
        if dry_run:
            created.append(entry["dest_rel"] + ("  [overwrite]" if exists else ""))
            continue
        try:
            enforce_fs_permission("write", dest_path)
            os.makedirs(os.path.dirname(dest_path) or ".", exist_ok=True)
            with open(dest_path, "w", encoding="utf-8", newline="\n") as f:
                f.write(entry["content"])
            created.append(entry["dest_rel"])
            if exists:
                overwritten.append(entry["dest_rel"])
        except Exception as e:
            errors.append(f"! write {entry['dest_rel']}: {e}")

    # ── Summary ──
    # quiet (default): flat counts + file lists only. verbose: nesting + TODOs + notes.
    summary = []
    if dry_run:
        summary.append(f"[DRY RUN — nothing written] preview of '{template}' -> {target_dir}")
    elif len(visited) > 1:
        summary.append(f"Composed {len(visited)} templates into {target_dir}")
    if created:
        summary.append("\n".join(created))
    if skipped:
        summary.append(
            (f"~ skipped ({len(skipped)}) — already exists:" if len(skipped) <= 6
             else f"~ skipped ({len(skipped)}) — already exists (first few):")
            + "\n" + "\n".join(f"  ~ {d}" for d in skipped[:6]))
    if overwritten:
        summary.append(f"~ overwritten ({len(overwritten)}):\n"
                       + "\n".join(f"  ~ {d}" for d in overwritten))
    for w in warnings:
        summary.append(w)
    if errors:
        summary.append(f"! errors ({len(errors)}):\n" + "\n".join(f"  ! {e}" for e in errors[:8]))
    out = "\n".join(summary)
    if quiet:
        return out
    out = body + (("\n" + out) if out else "")
    if todos:
        out += f"\nTODO ({len(todos)}):\n" + "\n".join(f"  * {t}" for t in todos[:10])
    notes = _template_meta(template)
    notes = notes.get("notes") if notes else None
    if notes:
        out += f"\nNOTES: {str(notes)[:400]}"
    return out

def _apply_template_inner(template: str, variables: dict, plan: list, visited: set,
                          depth: int, warnings: list, errors: list) -> list:
    """Recursive core: resolve the composition and push planned files to `plan`.

    Plan entries: {template, dest_rel, content, if_exists, todos}. Pure planning —
    nothing touches disk; apply_template commits after the WHOLE tree rendered.
    """
    indent = "  " * depth
    meta = _template_meta(template)
    if meta is None:
        hint = _fuzzy_template_hint(template)
        errors.append(f"{indent}! unknown template '{template}'{hint}")
        return [f"{indent}! unknown template '{template}'{hint}"]
    if "error" in meta:
        msg = f"{indent}! template '{template}' has invalid template.json: {meta['error']}"
        errors.append(msg)
        return [msg]
    if template in visited:
        return [f"{indent}! skip {template}: already applied (dedupe)"]
    visited.add(template)

    lines = [f"{indent}[{template}] ({meta.get('category', '?')})"]

    # ── 1. Defaults FIRST — so use-level overrides can reference them ({{ x }}) ──
    declared = {}
    for v in meta.get("variables", []):
        name = v.get("name", "")
        if not name:
            continue
        declared[name] = v
        if name not in variables and not v.get("required") and "default" in v:
            variables[name] = v.get("default")

    missing = [v["name"] for v in meta.get("variables", [])
               if v.get("required") and v["name"] not in variables]
    if missing:
        msg = (lines[0] + f"\n{indent}  ! missing required variables: {', '.join(missing)}"
               + f"\n{indent}    (declared by '{template}' — pass them in variables, "
               + f"see template_list(\"{template}\"))")
        errors.append(msg)
        return [msg]

    # ── 2. Coerce/validate provided values against declared types ──
    for name, v in declared.items():
        if name not in variables:
            continue
        coerced = _coerce_variable(name, v.get("type", "string"), variables[name], errors)
        if coerced is None:
            continue  # error already recorded
        variables[name] = coerced

    # ── 3. Compose referenced templates (stacks) — parent delimiters for overrides ──
    delims = meta.get("delimiters")
    for use in meta.get("uses", []):
        if isinstance(use, str):
            use_name, use_vars = use, {}
        else:
            use_name = use.get("template", "")
            use_vars = dict(use.get("variables") or {})
        rendered_use_vars = {}
        for k, v in use_vars.items():
            if isinstance(v, str) and _has_template_tokens(v, delims):
                try:
                    rendered_use_vars[k] = _render_str(v, variables, delims)
                except Exception:
                    rendered_use_vars[k] = v
            else:
                rendered_use_vars[k] = v
        merged = dict(variables)
        merged.update(rendered_use_vars)
        lines += _apply_template_inner(use_name, merged, plan, visited, depth + 1,
                                       warnings, errors)

    # ── 4. Own files (render only — write happens in apply_template) ──
    tdir = _template_dir(template)
    base = os.path.join(tdir, "files") if tdir else ""
    if not base or not os.path.isdir(base):
        lines.append(f"{indent}  (no own files — pure composition)")
    else:
        created_here, todos = [], []
        for fspec in meta.get("files", []):
            src = os.path.join(base, fspec["src"])
            if not os.path.isfile(src):
                errors.append(f"{indent}  ! {template}: missing source file: {fspec['src']}")
                continue
            try:
                with open(src, encoding="utf-8") as f:
                    content = f.read()
            except Exception as e:
                errors.append(f"{indent}  ! {template}: read {fspec['src']}: {e}")
                continue
            try:
                rendered = _render_str(content, variables, delims)
                dest_rel = _render_str(fspec["dest"], variables, delims)
            except Exception as e:
                errors.append(f"{indent}  ! {template}: render {fspec['src']}: {e}")
                continue
            dest_rel = os.path.normpath(dest_rel)
            if not dest_rel or dest_rel in (".", ".."):
                errors.append(f"{indent}  ! {template}: empty/unsafe dest for {fspec['src']}")
                continue
            file_todos = []
            for line in rendered.splitlines():
                if "TODO" in line:
                    marker = line.strip()[:120]
                    if marker not in file_todos:
                        file_todos.append(marker)
            plan.append({
                "template": template,
                "dest_rel": dest_rel,
                "content": rendered,
                "if_exists": fspec.get("if_exists"),
                "todos": file_todos,
            })
            created_here.append(dest_rel)
        for c in created_here:
            lines.append(f"{indent}  + {c}")
        if todos:
            lines.append(f"{indent}  TODO ({len(todos)}):")
            lines += [f"{indent}    * {t}" for t in todos[:10]]

    # ── 5. Unused-variable warnings (typo detection) — caller scope only, so
    # inherited stack variables (flowing through merged dicts) never spam ──
    if depth == 0:
        for name in variables:
            if name not in declared:
                warnings.append(f"{indent}  ! unused variable '{name}' (not declared by '{template}')")
    return lines

def template_list(template: str = "", category: str = "", query: str = "") -> str:
    """Template tool: catalog + ranked search + per-template source view in one.

    template_list()                 → catalog (name, category, description).
    template_list(query="table")    → ranked search: regex / words / code symbols /
                                      semantic (embedding) match, with why-it-matched.
    template_list(category="feature") → filter by level/category.
    template_list("feature-crud")   → detailed view: variables, recipe, file map,
                                      example call, conventions notes.
    """
    if template:
        return _template_source_view(template)
    if (query or "").strip():
        return _template_search(query, category)
    rows = []
    for name in _list_template_names():
        meta = _template_meta(name)
        if meta is None or "error" in meta:
            continue
        cat = meta.get("category", "")
        if category and cat != category:
            continue
        rows.append(f"{name} [{cat}] — {meta.get('description', '')}")
    if not rows:
        cat_txt = f" in category '{category}'" if category else ""
        return f"No templates{cat_txt} found in {_TEMPLATES_DIR}."
    sync_txt = ""
    sync = os.path.join(_TEMPLATES_DIR, _SYNC_STAMP)
    if os.path.isfile(sync):
        try:
            with open(sync, encoding="utf-8") as f:
                sync_txt = " | synced: " + str(json.load(f).get("commit", "?"))[:8]
        except Exception:
            pass
    return f"Templates ({len(rows)}):{sync_txt}\n" + "\n".join(rows)

def _template_source_view(template: str) -> str:
    """Return a template's variables, recipe, file map, and conventions notes."""
    meta = _template_meta(template)
    if meta is None:
        return (f"Error: unknown template '{template}'."
                + _fuzzy_template_hint(template))
    if "error" in meta:
        return f"Error: template '{template}' has invalid template.json: {meta['error']}"
    lines = [f"Template: {template} [{meta.get('category', '')}]"]
    lines.append(f"Description: {meta.get('description', '')}")
    if meta.get("uses"):
        lines.append("Composes (recipe):")
        for use in meta.get("uses", []):
            use_name = use if isinstance(use, str) else use.get("template", "?")
            use_meta = _template_meta(use_name)
            lines.append(f"  -> {use_name} [{use_meta.get('category', '?') if use_meta else '?'}] "
                         f"{use_meta.get('description', '')[:80] if use_meta else ''}")
    for v in meta.get("variables", []):
        req = "required" if v.get("required") else f"default: {v.get('default', '')}"
        lines.append(f"  var {v['name']} ({v.get('type', 'string')}, {req}): {v.get('description', '')}")
    if meta.get("files"):
        lines.append("Writes files to:")
        for fspec in meta.get("files", []):
            lines.append(f"  + {fspec['dest']}")
    sample = meta.get("sample_vars")
    if sample:
        pairs = []
        for k, v in sample.items():
            if isinstance(v, str):
                pairs.append(f"{k}={v!r}")
            else:
                pairs.append(f"{k}={json.dumps(v)}")
        lines.append(f"Example: apply_template(\"{template}\", variables={{{', '.join(pairs)}}})")
    notes = meta.get("notes")
    if notes:
        lines.append(f"NOTES: {str(notes)[:500]}")
    return "\n".join(lines)

_template_index = {"key": None, "entries": None}

_template_embeddings = {"key": None, "vecs": None}

def _embedding_model_available() -> bool:
    """Cheap check: does a usable bge-large-en-v1.5 model.onnx exist on disk?
    Avoids triggering a 1.3GB fastembed download when it's absent."""
    from tools import _HARNESS_ROOT
    try:
        cache_root = os.environ.get(
            "FASTEMBED_CACHE_PATH",
            os.path.join(_HARNESS_ROOT, ".deep_agents", "fastembed_cache"),
        )
        repo = os.path.join(cache_root, "models--qdrant--bge-large-en-v1.5-onnx", "blobs")
        if not os.path.isdir(repo):
            return False
        for fn in os.listdir(repo):
            try:
                if os.path.getsize(os.path.join(repo, fn)) > 50_000_000:
                    return True
            except OSError:
                continue
        return False
    except Exception:
        return False

def _embedding_model_warm() -> bool:
    """True only if the embedding model is ALREADY loaded in this process —
    template search never pays the 1.3GB cold-start load; it rides along when
    search_codebase (or anything else) warmed the model."""
    try:
        import embedding_service as es
        return es._model is not None
    except Exception:
        return False

def _extract_symbols(file_path: str) -> list:
    """Extract meaningful identifiers from a .j2 source (AST for python, regex otherwise)."""
    try:
        with open(file_path, encoding="utf-8") as f:
            text = f.read()
    except OSError:
        return []
    stripped = re.sub(r"\{\{.*?\}\}", " ", text, flags=re.DOTALL)
    stripped = re.sub(r"\{%.*?%\}", " ", stripped, flags=re.DOTALL)
    stripped = re.sub(r"\{#.*?#\}", " ", stripped, flags=re.DOTALL)
    symbols = set()
    if file_path.endswith(".py.j2"):
        try:
            import ast
            tree = ast.parse(stripped)
            for node in ast.walk(tree):
                if isinstance(node, ast.Import):
                    for a in node.names:
                        symbols.add(a.name.split(".")[0])
                elif isinstance(node, ast.ImportFrom):
                    if node.module:
                        symbols.add(node.module.split(".")[0])
                    for a in node.names:
                        symbols.add(a.name)
                elif isinstance(node, (ast.FunctionDef, ast.ClassDef)):
                    symbols.add(node.name)
            return sorted(symbols)
        except SyntaxError:
            pass
    for m in re.finditer(r"^\s*(?:import|from)\s+([\w.]+)", stripped, re.MULTILINE):
        symbols.add(m.group(1).split(".")[0])
    for m in re.finditer(r"\b(?:function|class|def)\s+([A-Za-z_]\w*)", stripped):
        symbols.add(m.group(1))
    for m in re.finditer(r"\b(?:const|let|var)\s+([A-Z][A-Za-z0-9_]*)", stripped):
        symbols.add(m.group(1))
    for m in re.finditer(r"export\s+(?:default\s+)?(?:function\s+|class\s+)?([A-Za-z_]\w*)",
                         stripped):
        symbols.add(m.group(1))
    return sorted(symbols)

def _template_index_key():
    """Cheap freshness stamp: newest mtime among template metadata + sources."""
    try:
        stamps = [os.path.getmtime(_TEMPLATES_DIR)]
        for root, _dirs, fns in os.walk(_TEMPLATES_DIR):
            for fn in fns:
                if fn.endswith((".json", ".j2")):
                    stamps.append(os.path.getmtime(os.path.join(root, fn)))
        return max(stamps)
    except OSError:
        return None

def _build_template_index() -> dict:
    """Lazy, mtime-checked index: metadata + uses + variables + dests + symbols."""
    key = _template_index_key()
    if _template_index["key"] == key and _template_index["entries"] is not None:
        return _template_index["entries"]
    entries = {}
    for name in _list_template_names():
        meta = _template_meta(name)
        if meta is None or "error" in meta:
            continue
        tdir = _template_dir(name)
        symbols = []
        fdir = os.path.join(tdir, "files")
        if os.path.isdir(fdir):
            for fn in sorted(os.listdir(fdir)):
                if fn.endswith(".j2"):
                    symbols += _extract_symbols(os.path.join(fdir, fn))
        entries[name] = {
            "name": name,
            "category": meta.get("category", ""),
            "description": meta.get("description", ""),
            "notes": str(meta.get("notes") or ""),
            "uses": [u if isinstance(u, str) else u.get("template", "")
                     for u in meta.get("uses", [])],
            "variables": [v.get("name", "") for v in meta.get("variables", [])],
            "dests": [f.get("dest", "") for f in meta.get("files", [])],
            "symbols": symbols,
        }
    _template_index["key"] = key
    _template_index["entries"] = entries
    return entries

def _template_search_blob(e: dict) -> str:
    return " ".join([e["name"], e["category"], e["description"], e["notes"],
                     " ".join(e["symbols"]), " ".join(e["dests"])])

def _template_search(query: str, category: str = "") -> str:
    """Ranked template search: regex / words / symbols / semantic, top 8 + why."""
    q = (query or "").strip()
    if not q:
        return template_list(category=category)
    entries = _build_template_index()

    is_regex = False
    rx = None
    if any(ch in q for ch in "|^$()*+[]?\\{}"):
        try:
            rx = re.compile(q)
            is_regex = True
        except re.error:
            is_regex = False

    words = [w.lower() for w in q.split() if not w.startswith("cat:")]
    cat_filter = next((w[4:].lower() for w in q.split() if w.startswith("cat:")), None)

    scored = []
    for e in entries.values():
        if category and e["category"] != category:
            continue
        if cat_filter and cat_filter not in e["category"].lower():
            continue
        score, why = 0, []
        hay = " ".join([e["name"], e["category"], e["description"]]).lower()
        symbols_txt = " ".join(e["symbols"]).lower()
        rest_txt = " ".join(e["dests"] + e["uses"] + e["variables"]).lower()
        if is_regex and rx:
            if rx.search(hay) or rx.search(symbols_txt) or rx.search(e["notes"].lower()):
                score += 100
                why.append("regex match")
        for w in words:
            if w in e["name"].lower():
                score += 60
                why.append(w)
            elif w in e["description"].lower():
                score += 25
                why.append(w)
            elif w in e["notes"].lower():
                score += 12
                why.append(w)
            elif any(w in s.lower() for s in e["symbols"]):
                score += 35
                why.append(f"symbol:{w}")
            elif w in rest_txt:
                score += 15
                why.append(w)
        if score:
            scored.append([score, e, list(dict.fromkeys(why))])

    # ── L3 semantic boost (only when the model is ALREADY loaded in this
    # process — e.g. search_codebase warmed it — so a template search never
    # stalls the agent loop on a 1.3GB model load) ──
    if not is_regex and words and _embedding_model_warm():
        try:
            from memory.embedding_service import embed_cached, cosine_similarity
            key = _template_index["key"]
            if _template_embeddings["key"] != key or _template_embeddings["vecs"] is None:
                from memory.embedding_service import embed_batch
                names = list(entries.keys())
                blobs = [_template_search_blob(entries[n]) for n in names]
                vecs = {n: v for n, v in zip(names, embed_batch(blobs))}
                _template_embeddings["key"] = key
                _template_embeddings["vecs"] = vecs
            qvec = embed_cached(q)
            vecs = _template_embeddings["vecs"]
            for item in scored:
                name = item[1]["name"]
                vec = vecs.get(name)
                if vec is None:
                    continue
                sim = cosine_similarity(qvec, vec)
                if sim >= 0.35:
                    item[0] += int(sim * 50)
                    item[2].append(f"sim {sim:.2f}")
        except Exception:
            pass

    scored.sort(key=lambda t: -t[0])
    top = scored[:8]
    if not top:
        return f"No templates match '{query}' — try a broader keyword."
    lines = [f"Template search '{query}' (top {len(top)}):"]
    for score, e, why in top:
        lines.append(f"  {e['name']} [{e['category']}] — {e['description'][:70]}")
        if why:
            lines.append(f"      why: {', '.join(why[:4])}")
    return "\n".join(lines)

def build_template_map() -> str:
    """Compact `## Template Map` for Zone 1 injection — WHAT exists, grouped by
    category, stale-safe (template_list() is the live source, like search_code)."""
    entries = _build_template_index()
    by_cat = {}
    for e in entries.values():
        by_cat.setdefault(e["category"], []).append(e)
    lines = [
        "## Template Map",
        "Stale-safe pointer (like the Repository Map) — template_list() shows live "
        "details. Naming = task type: comp-* component | feature-* vertical slice | "
        "project-*/stack-* scaffold | github-ci-* CI gates | config/routine/spec. "
        "Small helpers: write with edit_file — never template them.",
    ]
    for cat in sorted(by_cat):
        lines.append(f"[{cat}]")
        for e in sorted(by_cat[cat], key=lambda x: x["name"]):
            desc = (e["description"] or "")[:80]
            lines.append(f"  {e['name']} — {desc}")
    return "\n".join(lines)

def template_new(name: str, level: str, description: str, sample_vars: dict,
                 variables: list = None, files: list = None, uses: list = None,
                 notes: str = "") -> str:
    """Author a NEW template: templates/<level>/<name>/ with template.json + files/.

    Write the .j2 boilerplate files FIRST with edit_file into
    templates/<level>/<name>/files/ (dest defaults to the filename minus ".j2"
    when `files` is omitted). This tool writes the metadata and immediately
    validates that the new template renders cleanly with sample_vars.
    """
    import re as _re
    name = (name or "").strip()
    if not _re.fullmatch(r"[a-z0-9][a-z0-9-]*", name or ""):
        return f"Error: template name must be lowercase kebab-case, got {name!r}."
    level = (level or "").strip().lower()
    if level not in _TEMPLATE_CATEGORIES:
        return f"Error: level must be one of: {', '.join(_TEMPLATE_CATEGORIES)}."
    if _template_dir(name) is not None:
        return f"Error: template '{name}' already exists — edit it instead."
    if not isinstance(sample_vars, dict) or not sample_vars:
        return "Error: sample_vars is required (the CI renders templates with it)."
    tdir = os.path.join(_TEMPLATES_DIR, level, name)
    files_dir = os.path.join(tdir, "files")
    try:
        os.makedirs(files_dir, exist_ok=True)
    except Exception as e:
        return f"Error: cannot create {tdir}: {e}"

    file_specs = list(files or [])
    if not file_specs:
        for fn in sorted(os.listdir(files_dir)):
            if fn.endswith(".j2"):
                file_specs.append({"src": fn, "dest": fn[:-3]})
    if not file_specs and os.listdir(tdir):
        return ("Error: no files/ specified and none found — write .j2 boilerplate "
                "under templates/<level>/<name>/files/ first.")

    var_specs = []
    if variables:
        for v in variables:
            if isinstance(v, str):
                var_specs.append({"name": v, "type": "string", "required": False,
                                  "description": ""})
            elif isinstance(v, dict) and v.get("name"):
                var_specs.append(v)

    meta = {
        "name": name,
        "description": (description or "").strip(),
        "category": level,
        "variables": var_specs,
        "files": file_specs,
    }
    if uses:
        meta["uses"] = uses
    if notes:
        meta["notes"] = notes
    meta["sample_vars"] = sample_vars

    # ── Validate: metadata + render every file with sample_vars ──
    problems = []
    declared_names = {v.get("name") for v in var_specs}
    for v in var_specs:
        if v.get("type", "string") not in ("string", "int", "bool", "list"):
            problems.append(f"variable '{v['name']}': bad type {v.get('type')!r}")
    for k in sample_vars:
        if k not in declared_names:
            problems.append(f"sample_vars key '{k}' is not declared in variables")
    for fspec in file_specs:
        src = os.path.join(files_dir, fspec["src"])
        if not os.path.isfile(src):
            problems.append(f"missing source file: {fspec['src']}")
            continue
        try:
            with open(src, encoding="utf-8") as f:
                content = f.read()
            rendered = _render_str(content, dict(sample_vars), None)
            dest_rel = _render_str(fspec["dest"], dict(sample_vars), None)
            if _safe_dest_path(".", dest_rel) is None:
                problems.append(f"dest '{dest_rel}' escapes the target dir")
        except Exception as e:
            problems.append(f"render {fspec['src']}: {e}")

    try:
        with open(os.path.join(tdir, "template.json"), "w", encoding="utf-8",
                  newline="\n") as f:
            json.dump(meta, f, indent=2, ensure_ascii=False)
            f.write("\n")
    except Exception as e:
        return f"Error: cannot write template.json: {e}"

    if problems:
        return (f"[OK] template.json written to {os.path.join(tdir, 'template.json')} "
                f"but {len(problems)} problem(s) to fix:\n" + "\n".join(f"  ! {p}" for p in problems))
    return (f"[OK] Template '{name}' created ({level}) and renders cleanly — "
            f"{len(file_specs)} file(s).\n"
            f"Catalog: template_list() | details: template_list(\"{name}\")")

def template_pull(repo: str = "davinakmalyasha/agent-templates", ref: str = "main") -> str:
    """Sync templates/ from the agent-templates GitHub repo (hub -> local)."""
    from tools import _gh_api, _invalidate_tool_cache_all
    from base64 import b64decode
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    tree = _gh_api([f"repos/{repo}/git/trees/{ref}?recursive=1",
                    "--jq", r'[.tree[] | select(.type=="blob") | .path] | join("\n")'])
    if tree.startswith("Error"):
        return tree
    paths = [p for p in tree.splitlines() if p.strip()]
    if not paths:
        return f"No files found in {repo} @ {ref}."
    written, errors = [], []
    for p in paths:
        if p == _SYNC_STAMP:
            continue
        out = _gh_api([f"repos/{repo}/contents/{p}?ref={ref}", "--jq", ".content"])
        if out.startswith("Error"):
            errors.append(f"{p}: {str(out)[:120]}")
            continue
        try:
            content = b64decode(out.replace("\n", "")).decode("utf-8", "replace")
            dest = os.path.join(_TEMPLATES_DIR, p)
            os.makedirs(os.path.dirname(dest), exist_ok=True)
            with open(dest, "w", encoding="utf-8", newline="\n") as f:
                f.write(content)
            written.append(p)
        except Exception as e:
            errors.append(f"{p}: {e}")
    sha = ""
    head = _gh_api([f"repos/{repo}/git/ref/heads/{ref}", "--jq", ".object.sha"])
    if not head.startswith("Error"):
        sha = head.strip()[:12]
    try:
        with open(os.path.join(_TEMPLATES_DIR, _SYNC_STAMP), "w", encoding="utf-8") as f:
            json.dump({"repo": repo, "ref": ref, "commit": sha, "files": len(written)}, f)
    except Exception:
        pass
    _invalidate_tool_cache_all()  # template_list catalog changed
    out = f"Pulled {len(written)} files from {repo} @ {sha or ref} into templates/."
    if errors:
        out += f"\nErrors ({len(errors)}):\n" + "\n".join(f"  ! {e}" for e in errors[:10])
    return out

def template_push(repo: str = "davinakmalyasha/agent-templates", ref: str = "main",
                  create: bool = True, prune: bool = False) -> str:
    """Publish local templates/ to the GitHub repo (local -> hub). Creates the
    repo if missing (create=True). With prune=True, also deletes hub files that
    no longer exist locally (keeps the hub a true mirror)."""
    from tools import _gh_api, _run_gh
    from base64 import b64encode
    if "/" not in repo:
        return "Error: repo must be 'owner/name'."
    exists = _gh_api([f"repos/{repo}", "--jq", ".full_name"])
    if exists.startswith("Error"):
        if not create:
            return f"Error: repo {repo} not found (create=False)."
        cr = _run_gh(["repo", "create", repo.split("/")[1], "--public",
                      "-d", "Agent template library — reusable Jinja2 boilerplate for autonomous development"])
        if cr.startswith("Error"):
            return cr
        for _ in range(5):
            time.sleep(2)
            chk = _gh_api([f"repos/{repo}", "--jq", ".full_name"])
            if not chk.startswith("Error"):
                break
    files = []
    for root, _dirs, fns in os.walk(_TEMPLATES_DIR):
        for fn in sorted(fns):
            if fn == _SYNC_STAMP:
                continue
            p = os.path.relpath(os.path.join(root, fn), _TEMPLATES_DIR).replace("\\", "/")
            files.append(p)
    pushed, errors, skipped = [], [], []
    total = len(files)
    for i, p in enumerate(files, 1):
        try:
            with open(os.path.join(_TEMPLATES_DIR, p), "rb") as f:
                content = f.read()
        except Exception as e:
            errors.append(f"{p}: {e}")
            continue
        encoded = b64encode(content).decode("ascii")
        sha = _gh_api([f"repos/{repo}/contents/{p}?ref={ref}", "--jq", ".sha"])
        if not sha.startswith("Error"):
            import hashlib
            local_sha = hashlib.sha1(b"blob " + str(len(content)).encode() + b"\0" + content).hexdigest()
            if sha.strip() == local_sha:
                skipped.append(p)
                continue
        args = ["-X", "PUT", f"repos/{repo}/contents/{p}",
                "-f", "message=template: update " + p,
                "-f", "content=" + encoded]
        if not sha.startswith("Error"):
            args += ["-f", "sha=" + sha.strip()]
        out = _gh_api(args)
        if out.startswith("Error") and "rate limit" in out.lower():
            time.sleep(30)
            out = _gh_api(args)  # one retry after secondary-rate-limit backoff
        if out.startswith("Error"):
            errors.append(f"{p}: {str(out)[:120]}")
        else:
            pushed.append(p)
            if i % 25 == 0 or i == total:
                print(f"  [{i}/{total}] pushed")
    out = f"Pushed {len(pushed)} files ({len(skipped)} unchanged, skipped) to {repo} @ {ref}."
    if prune:
        pruned = []
        tree = _gh_api([f"repos/{repo}/git/trees/{ref}?recursive=1",
                        "--jq", r'[.tree[] | select(.type=="blob") | .path] | join("\n")'])
        if not tree.startswith("Error"):
            local_paths = set(files)
            for rp in tree.splitlines():
                rp = rp.strip()
                if not rp or rp in ("README.md", _SYNC_STAMP) or rp in local_paths:
                    continue
                sha = _gh_api([f"repos/{repo}/contents/{rp}?ref={ref}", "--jq", ".sha"])
                if not sha.startswith("Error"):
                    del_out = _gh_api(["-X", "DELETE", f"repos/{repo}/contents/{rp}",
                                       "-f", "message=template: prune " + rp,
                                       "-f", "sha=" + sha.strip()])
                    if del_out.startswith("Error"):
                        out += f"\n  ! prune failed: {rp}: {str(del_out)[:120]}"
                    else:
                        pruned.append(rp)
        if pruned:
            out += f"\nPruned {len(pruned)} stale files: {', '.join(pruned[:10])}"
    if errors:
        out += "\nErrors:\n" + "\n".join(f"  ! {e}" for e in errors[:10])
    return out
