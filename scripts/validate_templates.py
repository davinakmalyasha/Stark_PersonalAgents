"""Validate every template renders with its sample variables.

Stacks (templates with "uses") are validated recursively: every referenced
template must exist and render with the merged sample variables. Validation is
strict — a template without sample_vars FAILS (the README convention says every
template needs it).

Checks beyond rendering:
  - use-overrides referencing parent variables must be declared by the parent
  - rendered dest paths must not escape the target dir
  - no two files in one template may render to the same dest
  - template.json + files must be valid UTF-8 (no cp1252 mojibake)
  - variable specs must be well-formed (name/type/description, unique names)
  - delimiters must be a 4-element list when present
  - every required variable must be covered by sample_vars

Standalone (only needs jinja2) so CI stays light:
    pip install jinja2
    python scripts/validate_templates.py
"""
import json
import pathlib
import sys
import tempfile

from jinja2 import StrictUndefined
from jinja2.sandbox import SandboxedEnvironment

ROOT = pathlib.Path(__file__).resolve().parent.parent
TEMPLATES = ROOT / "templates"
ALLOWED_TYPES = ("string", "int", "bool", "list")

_env_cache = {}


def _render_env(delims: list | None = None):
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


def render_str(text: str, variables: dict, delims: list | None = None) -> str:
    """Render a Jinja2 string with optional custom delimiters (sandboxed)."""
    return _render_env(delims).from_string(text).render(**variables)


def _has_tokens(text: str, delims: list | None) -> bool:
    if delims and len(delims) >= 2:
        if delims[0] in text:
            return True
        return len(delims) > 2 and delims[2] in text
    return "{{" in text or "{%" in text


def find_template_dir(name: str) -> pathlib.Path:
    """Locate a template folder: templates/<level>/<name>/ (one level deep)."""
    for level_dir in sorted(TEMPLATES.iterdir()):
        if not level_dir.is_dir() or level_dir.name in (".sync.json",):
            continue
        candidate = level_dir / name
        if (candidate / "template.json").is_file():
            return candidate
    raise ValueError(f"missing template '{name}'")


def load_meta(name: str) -> dict:
    return json.loads((find_template_dir(name) / "template.json").read_text(encoding="utf-8"))


def fill_defaults(meta: dict, variables: dict) -> dict:
    for v in meta.get("variables", []):
        if v["name"] not in variables and not v.get("required") and "default" in v:
            variables[v["name"]] = v.get("default")
    return variables


def check_variable_schema(name: str, meta: dict, failures: list) -> None:
    """Validate variable declarations: well-formed, unique, allowed types."""
    seen = set()
    for v in meta.get("variables", []):
        vname = v.get("name", "")
        if not vname:
            failures.append(f"{name}: variable entry missing 'name'")
            continue
        if vname in seen:
            failures.append(f"{name}: duplicate variable '{vname}'")
        seen.add(vname)
        if not isinstance(v.get("description", ""), str) or not v.get("description"):
            failures.append(f"{name}: variable '{vname}' missing description")
        vtype = v.get("type", "string")
        if vtype not in ALLOWED_TYPES:
            failures.append(f"{name}: variable '{vname}' has invalid type {vtype!r}")
        if v.get("required") and "default" in v:
            failures.append(f"{name}: variable '{vname}' is required AND has a default")


def validate_template(name: str, variables: dict, visited: set, failures: list) -> int:
    """Render one template (and its recipe) into a temp dir. Returns files rendered."""
    if name in visited:
        return 0
    visited.add(name)
    meta = load_meta(name)
    variables = fill_defaults(dict(meta), dict(variables))

    missing = [v["name"] for v in meta.get("variables", [])
               if v.get("required") and v["name"] not in variables]
    if missing:
        failures.append(f"{name}: missing required variables: {', '.join(missing)}")
        return 0

    delims = meta.get("delimiters")
    declared_names = {v.get("name") for v in meta.get("variables", [])}

    # ── Static checks ──
    for use in meta.get("uses", []):
        use_vars = {} if isinstance(use, str) else dict(use.get("variables") or {})
        for k, v in use_vars.items():
            if delims and isinstance(v, str) and ("{{" in v or "{%" in v):
                failures.append(
                    f"{name}: use-override '{k}' uses {{ }} but this template's "
                    f"delimiters are {delims} — write overrides with {delims[0]} {delims[1]}")
            if isinstance(v, str) and _has_tokens(v, delims):
                for var_ref in _referenced_vars(v, delims):
                    if var_ref not in declared_names:
                        failures.append(
                            f"{name}: use-override '{k}' references undeclared parent "
                            f"variable '{var_ref}' (defaults are filled before uses, "
                            f"but the name must still be declared)")

    rendered_dests = set()
    rendered = 0
    base = find_template_dir(name) / "files"
    if base.is_dir():
        tmp = pathlib.Path(tempfile.mkdtemp(prefix=f"tpl-{name}-"))
        for fspec in meta.get("files", []):
            src = base / fspec["src"]
            if not src.is_file():
                failures.append(f"{name}: missing source {fspec['src']}")
                continue
            content = src.read_text(encoding="utf-8")
            try:
                rendered_text = render_str(content, variables, delims)
                dest_rel = render_str(fspec["dest"], variables, delims)
            except Exception as e:
                failures.append(f"{name}: render {fspec['src']}: {e}")
                continue
            dest_rel = dest_rel.replace("\\", "/")
            if dest_rel.startswith("/") or dest_rel.startswith("../") or "..\\" in dest_rel:
                failures.append(f"{name}: dest '{dest_rel}' escapes the target dir")
                continue
            if dest_rel in rendered_dests:
                failures.append(f"{name}: duplicate rendered dest '{dest_rel}' "
                                f"(src {fspec['src']})")
                continue
            rendered_dests.add(dest_rel)
            dest = tmp / dest_rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(rendered_text, encoding="utf-8")
            rendered += 1

    # ── Recipe (uses) — rendered with the PARENT's delimiters for overrides ──
    for use in meta.get("uses", []):
        use_name = use if isinstance(use, str) else use.get("template", "")
        use_vars = {} if isinstance(use, str) else dict(use.get("variables") or {})
        rendered_use_vars = {}
        for k, v in use_vars.items():
            if isinstance(v, str) and _has_tokens(v, delims):
                try:
                    rendered_use_vars[k] = render_str(v, variables, delims)
                except Exception:
                    rendered_use_vars[k] = v
            else:
                rendered_use_vars[k] = v
        merged = dict(variables)
        merged.update(rendered_use_vars)
        try:
            rendered += validate_template(use_name, merged, visited, failures)
        except Exception as e:
            failures.append(f"{name}: recipe {use_name}: {e}")
    return rendered


def _referenced_vars(text: str, delims: list | None) -> set:
    """Names referenced inside template tokens ({{ name }} / [[ name ]])."""
    var_start = (delims[0] if delims and len(delims) >= 1 else "{{")
    var_end = (delims[1] if delims and len(delims) >= 2 else "}}")
    refs = set()
    idx = 0
    while True:
        s = text.find(var_start, idx)
        if s < 0:
            break
        e = text.find(var_end, s + len(var_start))
        if e < 0:
            break
        expr = text[s + len(var_start):e].strip()
        if expr and expr.replace("_", "").isalnum():
            refs.add(expr)
        idx = e + len(var_end)
    return refs


def main() -> int:
    failures = []
    total_rendered = 0
    meta_paths = sorted(TEMPLATES.glob("*/*/template.json"))
    for meta_path in meta_paths:
        name = meta_path.parent.name
        try:
            meta = json.loads(meta_path.read_text(encoding="utf-8"))
        except UnicodeDecodeError as e:
            failures.append(f"{name}: template.json is NOT valid UTF-8 (cp1252 mojibake?): {e}")
            continue
        except json.JSONDecodeError as e:
            failures.append(f"{name}: template.json is not valid JSON: {e}")
            continue

        # ── Strict: every template needs sample_vars (README convention) ──
        sample = meta.get("sample_vars")
        if not isinstance(sample, dict):
            failures.append(f"{name}: missing sample_vars (required for validation)")
            print(f"FAIL  {name}: missing sample_vars")
            continue

        # ── Encoding: all files must be valid UTF-8 ──
        base = meta_path.parent / "files"
        if base.is_dir():
            for f in base.iterdir():
                if f.is_file():
                    try:
                        f.read_text(encoding="utf-8")
                    except UnicodeDecodeError as e:
                        failures.append(f"{name}: file {f.name} is NOT valid UTF-8: {e}")

        # ── Metadata shape ──
        if not isinstance(meta.get("description"), str) or "\n" in meta.get("description", ""):
            failures.append(f"{name}: description must be a single line")
        if "category" not in meta:
            failures.append(f"{name}: missing category")
        check_variable_schema(name, meta, failures)
        if "delimiters" in meta and (not isinstance(meta["delimiters"], list)
                                     or len(meta["delimiters"]) != 4):
            failures.append(f"{name}: delimiters must be a 4-element list")

        # ── Required vars must be covered by sample_vars ──
        for v in meta.get("variables", []):
            if v.get("required") and v["name"] not in sample:
                failures.append(f"{name}: required variable '{v['name']}' missing from sample_vars")

        try:
            n = validate_template(name, dict(sample), set(), failures)
            total_rendered += n
            print(f"OK    {name}: {n} files rendered")
        except Exception as e:
            failures.append(f"{name}: {e}")
            print(f"FAIL  {name}: {e}")
    for failure in failures:
        print(f"FAIL  {failure}")
    template_count = len(meta_paths)
    print(f"\n{total_rendered} files rendered across {template_count} templates")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
