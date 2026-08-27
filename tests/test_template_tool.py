"""apply_template + template authoring tests — real tool, temp dirs only."""
import json
import os
import tempfile
from contextlib import contextmanager

from _util import (template_tools, tools_stub, temp_dir, write_file, Skip)

try:
    import jinja2  # noqa: F401
    import yaml  # noqa: F401
except ImportError:
    raise Skip("jinja2/pyyaml not installed")

tt = template_tools()
tools_stub().apply_template = tt.apply_template  # lazy `from tools import apply_template`


@contextmanager
def _workshop():
    """Temp dir that IS the template catalog — the real templates/ tree is
    never touched; _TEMPLATES_DIR is swapped for the duration."""
    with temp_dir() as tmp:
        orig = tt._TEMPLATES_DIR
        tt._TEMPLATES_DIR = tmp
        try:
            yield tmp
        finally:
            tt._TEMPLATES_DIR = orig


def _meta(tmp, **meta):
    name = meta["name"]
    tdir = os.path.join(tmp, "routine", name)
    write_file(os.path.join(tdir, "template.json"), json.dumps(meta))
    return tdir


def _simple_meta(tmp, files=None, **kw):
    name = kw.get("name", "test-tpl")
    meta = {
        "description": "test template",
        "category": "routine",
        "variables": kw.get("variables", []),
        "files": files or [{"src": "hello.txt.j2", "dest": "hello.txt"}],
        "sample_vars": kw.get("sample_vars", {"who": "world"}),
    }
    if kw.get("uses"):
        meta["uses"] = kw["uses"]
    if kw.get("delimiters"):
        meta["delimiters"] = kw["delimiters"]
    if kw.get("notes"):
        meta["notes"] = kw["notes"]
    tdir = _meta(tmp, name=name, **meta)
    for fspec in meta["files"]:
        write_file(os.path.join(tdir, "files", fspec["src"]),
                   kw.get("content", "hello {{ who }}\n"))
    return tdir


def _dest(tmp) -> str:
    """apply_template requires an existing target dir."""
    d = os.path.join(tmp, "dest")
    os.makedirs(d, exist_ok=True)
    return d


def _read(dest, rel) -> str:
    with open(os.path.join(dest, rel), encoding="utf-8") as f:
        return f.read()


# ── core apply semantics ────────────────────────────────────────────────────

def test_apply_writes_files_and_reports():
    with _workshop() as tmp:
        _simple_meta(tmp)
        out = tt.apply_template("test-tpl", target_dir=_dest(tmp), variables={"who": "gate"})
        assert "hello.txt" in out and "Error" not in out
        assert _read(_dest(tmp), "hello.txt") == "hello gate\n"


def test_missing_target_dir_errors():
    out = tt.apply_template("readme", target_dir="Z:/definitely/not/here", variables={})
    assert out.startswith("Error")


def test_skip_if_exists_default():
    with _workshop() as tmp:
        _simple_meta(tmp)
        dest = _dest(tmp)
        tt.apply_template("test-tpl", target_dir=dest, variables={"who": "a"})
        write_file(os.path.join(dest, "hello.txt"), "CUSTOM\n")
        out = tt.apply_template("test-tpl", target_dir=dest, variables={"who": "b"})
        assert "skipped" in out
        assert _read(dest, "hello.txt") == "CUSTOM\n"  # untouched


def test_overwrite_flag_replaces():
    with _workshop() as tmp:
        _simple_meta(tmp)
        dest = _dest(tmp)
        tt.apply_template("test-tpl", target_dir=dest, variables={"who": "a"})
        out = tt.apply_template("test-tpl", target_dir=dest, overwrite=True,
                                variables={"who": "b"})
        assert "overwritten" in out
        assert _read(dest, "hello.txt") == "hello b\n"


def test_if_exists_error_blocks():
    with _workshop() as tmp:
        _simple_meta(tmp, files=[{"src": "hello.txt.j2", "dest": "hello.txt",
                                  "if_exists": "error"}])
        dest = _dest(tmp)
        tt.apply_template("test-tpl", target_dir=dest, variables={"who": "a"})
        out = tt.apply_template("test-tpl", target_dir=dest, variables={"who": "b"})
        # commit-phase problems are reported in the "! errors" summary section
        assert "already exists (if_exists=error)" in out and "! errors (1)" in out
        assert _read(dest, "hello.txt") == "hello a\n"  # original untouched


def test_dry_run_writes_nothing():
    with _workshop() as tmp:
        _simple_meta(tmp)
        dest = os.path.join(tmp, "dest")
        os.makedirs(dest)
        out = tt.apply_template("test-tpl", target_dir=dest, dry_run=True,
                                variables={"who": "x"})
        assert "DRY RUN" in out
        assert not os.path.exists(os.path.join(dest, "hello.txt"))


def test_byte_identical_reapply_is_unchanged():
    with _workshop() as tmp:
        _simple_meta(tmp)
        dest = _dest(tmp)
        tt.apply_template("test-tpl", target_dir=dest, variables={"who": "same"})
        out = tt.apply_template("test-tpl", target_dir=dest, variables={"who": "same"})
        # the unchanged line itself reads "byte-identical, skipped" — assert the
        # unchanged bucket was used, not the exists-skip bucket
        assert "unchanged (1)" in out and "already exists" not in out


def test_two_phase_atomicity_nothing_written_on_failure():
    """A composition whose CHILD fails must leave zero files on disk."""
    with _workshop() as tmp:
        _simple_meta(tmp, name="child-tpl",
                     files=[{"src": "c.txt.j2", "dest": "c.txt"}],
                     content="{{ needs_undeclared }}",
                     sample_vars={})
        _simple_meta(tmp, name="parent-tpl",
                     files=[{"src": "p.txt.j2", "dest": "p.txt"}],
                     content="parent {{ who }}\n", uses=["child-tpl"],
                     sample_vars={"who": "w"})
        dest = os.path.join(tmp, "dest")
        out = tt.apply_template("parent-tpl", target_dir=dest, variables={"who": "w"})
        assert out.startswith("Error")
        assert not os.path.exists(dest) or not os.listdir(dest)


def test_same_dest_conflict_second_skipped_with_warning():
    with _workshop() as tmp:
        _simple_meta(tmp, name="tpl-a", files=[{"src": "h.txt.j2", "dest": "shared.txt"}])
        _simple_meta(tmp, name="tpl-b", files=[{"src": "h.txt.j2", "dest": "shared.txt"}],
                     content="B {{ who }}\n")
        _simple_meta(tmp, name="tpl-root",
                     files=[{"src": "r.txt.j2", "dest": "r.txt"}],
                     content="root\n",
                     uses=["tpl-a", "tpl-b"])
        dest = _dest(tmp)
        out = tt.apply_template("tpl-root", target_dir=dest, variables={"who": "x"})
        assert "conflict" in out
        assert _read(dest, "shared.txt") == "hello x\n"  # first composition wins


def test_dest_escape_rejected():
    with _workshop() as tmp:
        _simple_meta(tmp, files=[{"src": "hello.txt.j2", "dest": "../escaped.txt"}])
        out = tt.apply_template("test-tpl", target_dir=_dest(tmp), variables={"who": "x"})
        assert "Error" in out or "unsafe" in out
        assert not os.path.exists(os.path.join(tmp, "escaped.txt"))


def test_safe_dest_path_traversal():
    assert tt._safe_dest_path("D:/tmp/root", "../evil") is None
    assert tt._safe_dest_path("D:/tmp/root", "/abs") is None
    assert tt._safe_dest_path("D:/tmp/root", "sub/ok.txt") is not None
    assert tt._safe_dest_path("D:/tmp/root", ".") is None


# ── variable handling ───────────────────────────────────────────────────────

def test_coerce_int_bool_list():
    errs = []
    assert tt._coerce_variable("n", "int", "42", errs) == 42
    assert tt._coerce_variable("b", "bool", "yes", errs) is True
    assert tt._coerce_variable("b", "bool", 0, errs) is False
    assert tt._coerce_variable("l", "list", "a, b,c", errs) == ["a", "b", "c"]
    assert tt._coerce_variable("n", "int", "abc", errs) is None and len(errs) == 1
    assert tt._coerce_variable("b", "bool", "maybe", errs) is None and len(errs) == 2
    assert tt._coerce_variable("l", "list", 5, errs) is None and len(errs) == 3


def test_missing_required_variable_fails_clean():
    with _workshop() as tmp:
        _simple_meta(tmp, variables=[{"name": "who", "type": "string",
                                      "required": True, "description": "who"}],
                     sample_vars={"who": "x"})
        out = tt.apply_template("test-tpl", target_dir=_dest(tmp), variables={})
        assert "missing required variables" in out


def test_unused_variable_warning():
    with _workshop() as tmp:
        _simple_meta(tmp)
        out = tt.apply_template("test-tpl", target_dir=_dest(tmp),
                                variables={"who": "x", "bogus_extra": "1"})
        assert "unused variable 'bogus_extra'" in out


def test_type_coercion_in_apply_int():
    with _workshop() as tmp:
        _simple_meta(tmp, variables=[{"name": "port", "type": "int",
                                      "description": "port"}],
                     sample_vars={"port": 8080},
                     content="port={{ port }}\n")
        dest = _dest(tmp)
        out = tt.apply_template("test-tpl", target_dir=dest, variables={"port": "9090"})
        assert "Error" not in out
        assert _read(dest, "hello.txt") == "port=9090\n"


# ── composition ─────────────────────────────────────────────────────────────

def test_uses_composition_order_and_dedupe():
    with _workshop() as tmp:
        _simple_meta(tmp, name="dep-one", files=[{"src": "a.txt.j2", "dest": "a.txt"}])
        _simple_meta(tmp, name="dep-two", files=[{"src": "b.txt.j2", "dest": "b.txt"}])
        _simple_meta(tmp, name="root-tpl", files=[{"src": "r.txt.j2", "dest": "r.txt"}],
                     content="root {{ who }}\n", uses=["dep-one", "dep-two", "dep-one"])
        dest = _dest(tmp)
        out = tt.apply_template("root-tpl", target_dir=dest, variables={"who": "x"})
        assert "Composed 3 templates" in out  # root + 2 unique deps (dedupe)
        out_v = tt.apply_template("root-tpl", target_dir=dest, variables={"who": "x"},
                                  quiet=False)
        assert "already applied (dedupe)" in out_v  # verbose body shows the cycle break
        for fn in ("a.txt", "b.txt", "r.txt"):
            assert os.path.exists(os.path.join(dest, fn))


def test_use_override_flows_through_parent_var():
    with _workshop() as tmp:
        _simple_meta(tmp, name="dep-var",
                     variables=[{"name": "label", "type": "string", "description": "l"},
                                {"name": "who", "type": "string", "description": "w"}],
                     sample_vars={"label": "L", "who": "w"},
                     content="label={{ label }}\n")
        _simple_meta(tmp, name="root-var",
                     variables=[{"name": "who", "type": "string", "description": "who"}],
                     uses=[{"template": "dep-var", "variables": {"label": "{{ who }}-suffix"}}])
        dest = _dest(tmp)
        out = tt.apply_template("root-var", target_dir=dest, variables={"who": "PR"})
        assert "Error" not in out
        assert _read(dest, "hello.txt") == "label=PR-suffix\n"


def test_composition_cycle_is_safe():
    with _workshop() as tmp:
        _simple_meta(tmp, name="cyc-a", files=[{"src": "a.txt.j2", "dest": "a.txt"}],
                     uses=["cyc-b"])
        _simple_meta(tmp, name="cyc-b", files=[{"src": "b.txt.j2", "dest": "b.txt"}],
                     uses=["cyc-a"])
        out = tt.apply_template("cyc-a", target_dir=_dest(tmp), variables={"who": "x"})
        assert "Error" not in out  # dedupe breaks the cycle


def test_unknown_template_fuzzy_hint():
    # real catalog on purpose: the hint must match real template names
    out = tt.apply_template("github-fast-check", target_dir=tempfile.gettempdir(),
                            variables={})
    assert out.startswith("Error") and "did you mean" in out


# ── TODO extraction + notes ─────────────────────────────────────────────────

def test_todo_markers_extracted():
    with _workshop() as tmp:
        _simple_meta(tmp, content="# TODO wire the webhook\nvalue={{ who }}\n")
        out = tt.apply_template("test-tpl", target_dir=_dest(tmp), variables={"who": "x"})
        assert "TODO (1)" in out and "wire the webhook" in out


def test_notes_surfaced_in_quiet_mode():
    with _workshop() as tmp:
        _simple_meta(tmp, notes="Finish the wiring before shipping.")
        out = tt.apply_template("test-tpl", target_dir=_dest(tmp), variables={"who": "x"})
        assert "NOTES: Finish the wiring" in out


# ── identity inference (O2) — via patched _template_meta ────────────────────

def _with_declared(declared):
    orig = tt._template_meta
    tt._template_meta = lambda t: {"variables": declared}
    return orig


def test_infer_project_variables_kebab_and_ident():
    with _workshop() as tmp:
        ws = os.path.join(tmp, "My Cool App")
        os.makedirs(ws)
        declared = [{"name": "project_name", "type": "string", "description": "p"},
                    {"name": "package_dir", "type": "string", "description": "d"}]
        orig = _with_declared(declared)
        try:
            inferred, warns = tt._infer_project_variables("fake", ws, {})
        finally:
            tt._template_meta = orig
        assert inferred["project_name"] == "my-cool-app"
        assert inferred["package_dir"] == "my_cool_app"
        assert any("inferred project_name" in w for w in warns)


def test_inference_skips_undeclared_and_explicit_wins():
    with _workshop() as tmp:
        ws = os.path.join(tmp, "myapp")
        os.makedirs(ws)
        # nothing declared → nothing inferred
        orig = _with_declared([])
        try:
            inferred, _ = tt._infer_project_variables("fake", ws, {})
            assert inferred == {}
        finally:
            tt._template_meta = orig
        # explicit value wins → no inference, no warning
        orig = _with_declared([{"name": "project_name", "type": "string",
                                "description": "p"}])
        try:
            inferred, warns = tt._infer_project_variables("fake", ws,
                                                          {"project_name": "explicit"})
            assert inferred == {}
            assert not any("project_name" in w for w in warns)
        finally:
            tt._template_meta = orig


# ── template_new authoring (against a TEMP templates root) ──────────────────

def test_template_new_validation_and_happy_path():
    with _workshop() as tmp:
        orig_dir = tt._TEMPLATES_DIR
        tt._TEMPLATES_DIR = tmp
        try:
            r = tt.template_new("Bad Name", "routine", "d", {"a": "1"})
            assert r.startswith("Error") and "kebab" in r
            r = tt.template_new("tpl-x", "not-a-level", "d", {"a": "1"})
            assert r.startswith("Error") and "level must be one of" in r
            r = tt.template_new("tpl-x", "routine", "d", {})
            assert r.startswith("Error") and "sample_vars is required" in r
            r = tt.template_new("tpl-x", "routine", "d", {"a": "1"})
            assert r.startswith("Error")  # no files yet
            tdir = os.path.join(tmp, "routine", "tpl-x", "files")
            write_file(os.path.join(tdir, "conf.txt.j2"), "v={{ a }}\n")
            r = tt.template_new("tpl-x", "routine", "test template", {"a": "1"},
                                variables=["a"])
            assert r.startswith("[OK]") and "renders cleanly" in r, r
            meta = json.load(open(os.path.join(tmp, "routine", "tpl-x", "template.json"),
                                  encoding="utf-8"))
            assert meta["files"] == [{"src": "conf.txt.j2", "dest": "conf.txt"}]
            r = tt.template_new("tpl-x", "routine", "d", {"a": "1"})
            assert "already exists" in r
        finally:
            tt._TEMPLATES_DIR = orig_dir


def test_template_new_sample_key_undeclared_reported():
    with _workshop() as tmp:
        orig_dir = tt._TEMPLATES_DIR
        tt._TEMPLATES_DIR = tmp
        try:
            tdir = os.path.join(tmp, "routine", "tpl-y", "files")
            write_file(os.path.join(tdir, "f.txt.j2"), "static\n")
            r = tt.template_new("tpl-y", "routine", "d", {"undeclared": "1"})
            assert "problem(s)" in r and "not declared" in r
        finally:
            tt._TEMPLATES_DIR = orig_dir
