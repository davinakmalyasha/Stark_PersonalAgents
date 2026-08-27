"""Renderer tests — tools/AddOnTool/templateTools.py _render_env/_render_str."""
from _util import template_tools, Skip

try:
    import jinja2  # noqa: F401
except ImportError:
    raise Skip("jinja2 not installed")


def test_basic_substitution():
    tt = template_tools()
    assert tt._render_str("Hello {{ name }}!", {"name": "World"}) == "Hello World!"


def test_strict_undefined_rejects_missing():
    tt = template_tools()
    try:
        tt._render_str("{{ missing_var }}", {})
        assert False, "StrictUndefined should have raised"
    except Exception:
        pass


def test_keep_trailing_newline():
    tt = template_tools()
    assert tt._render_str("line\n", {}) == "line\n"


def test_trim_blocks_must_stay_off():
    """The regression class: block tags at end-of-line must NOT eat the next
    newline, or rendered YAML glues together. Assert env flags directly."""
    tt = template_tools()
    env = tt._render_env()
    assert env.trim_blocks is False
    assert env.lstrip_blocks is False
    assert env.keep_trailing_newline is True


def test_block_tag_newline_preserved_in_render():
    tt = template_tools()
    text = "a: 1\n{% if true %}\nb: 2\n{% endif %}\nc: 3\n"
    out = tt._render_str(text, {})
    # with trim_blocks=False the newlines after %} survive → YAML stays valid
    import yaml
    data = yaml.safe_load(out)
    assert data == {"a": 1, "b": 2, "c": 3}


def test_custom_delimiters_var_only():
    tt = template_tools()
    delims = ["[[", "]]", "{@", "@}"]
    out = tt._render_str("x: [[ v ]]", {"v": "42"}, delims)
    assert out == "x: 42"
    # default tokens must stay untouched under custom delimiters
    out2 = tt._render_str("keep {{ raw }} and [[ v ]]", {"v": "1"}, delims)
    assert out2 == "keep {{ raw }} and 1"


def test_custom_delimiters_full_set():
    tt = template_tools()
    delims = ["[[", "]]", "{@", "@}"]
    out = tt._render_str("{@ if on @}Y{@ endif @}", {"on": True}, delims)
    assert out == "Y"


def test_env_cache_reuses_per_delims():
    tt = template_tools()
    e1 = tt._render_env()
    e2 = tt._render_env()
    e3 = tt._render_env(["[[", "]]"])
    assert e1 is e2
    assert e3 is not e1


def test_has_template_tokens():
    tt = tt = template_tools()
    assert tt._has_template_tokens("{{ x }}", None) is True
    assert tt._has_template_tokens("{% if %}", None) is True
    assert tt._has_template_tokens("plain", None) is False
    assert tt._has_template_tokens("[[ x ]]", ["[[", "]]"]) is True
    assert tt._has_template_tokens("{{ x }}", ["[[", "]]"]) is False


def test_sandboxed_environment_blocks_dunder():
    tt = template_tools()
    try:
        tt._render_str("{{ ''.__class__ }}", {})
        # SandboxedEnvironment allows attribute access but blocks dangerous ones;
        # rendering __class__ itself may succeed — accessing __subclasses__ must not.
        tt._render_str("{{ ''.__class__.__subclasses__() }}", {})
        assert False, "sandbox should have blocked __subclasses__"
    except Exception:
        pass


def test_production_tier_templates_render_under_default_delims():
    """All three tier templates must render with their sample_vars without
    StrictUndefined errors (renderer/catalog contract). ${{ }} GitHub
    expressions legitimately survive rendering."""
    import re
    tt = template_tools()
    for name in ("github-fast-checks", "github-staging", "github-release-heavy"):
        meta = tt._template_meta(name)
        variables = {}
        for v in meta.get("variables", []):
            if not v.get("required") and "default" in v:
                variables[v["name"]] = v["default"]
        variables.update(meta.get("sample_vars") or {})
        for fspec in meta.get("files", []):
            with open(tt._template_dir(name) + "/files/" + fspec["src"], encoding="utf-8") as f:
                out = tt._render_str(f.read(), dict(variables), meta.get("delimiters"))
            stripped = re.sub(r"\$\{\{[^}]*\}\}", "", out)
            assert "{{" not in stripped and "{%" not in stripped, \
                f"{name}: unrendered tokens remain"
