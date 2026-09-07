"""branch_levels.py — the shared 3-level resolver, exhaustively."""
import os

from _util import clean_ci_env  # must precede branch_levels: puts repo root on sys.path
import branch_levels as bl


def test_load_repo_config_full():
    import json, tempfile
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, ".harness-ci.json")
        with open(p, "w", encoding="utf-8") as f:
            json.dump({"branches": {"dev": "develop", "staging": "qa",
                                    "prod": ["trunk", "release"]}}, f)
        cfg = bl.load_repo_config(d)
        assert cfg["dev"] == "develop"
        assert cfg["staging"] == "qa"
        assert cfg["prod"] == ["trunk", "release"]


def test_load_repo_config_prod_scalar_becomes_list():
    import json, tempfile
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, ".harness-ci.json")
        with open(p, "w", encoding="utf-8") as f:
            json.dump({"branches": {"prod": "trunk"}}, f)
        assert bl.load_repo_config(d)["prod"] == ["trunk"]


def test_load_repo_config_absent_or_malformed_empty():
    assert bl.load_repo_config("Z:/no/such/dir") == {}
    import json, tempfile
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, ".harness-ci.json")
        with open(p, "w", encoding="utf-8") as f:
            f.write("{not json")
        assert bl.load_repo_config(d) == {}


def test_env_overrides_parsing():
    env = {"CI_DEV_BRANCH": " dev ", "CI_STAGING_BRANCH": "",
           "CI_PROD_BRANCHES": "trunk, release ,"}
    o = bl.env_overrides(env)
    assert o["dev"] == "dev"
    assert o["staging"] == ""
    assert o["prod"] == ["trunk", "release"]


def test_discover_candidates():
    assert bl.discover(["main", "develop", "qa"], "main") == ("develop", "qa")
    assert bl.discover(["main", "test"], "main") == ("", "test")
    assert bl.discover(["main"], "main") == ("", "")
    assert bl.discover([], "main") == ("", "")


def test_normalize_defaults():
    with clean_ci_env():
        lv = bl.normalize({}, {}, heads=["main"], default="main")
    assert lv["dev"] == "development"
    assert lv["staging"] == ""
    assert lv["prod"] == ["main", "master"]
    assert lv["source"] == "default"
    assert lv["collision"] is False


def test_normalize_prod_union_always_includes_hard_and_default():
    with clean_ci_env():
        lv = bl.normalize({"prod": ["trunk"]}, {}, heads=[], default="deploy")
    assert set(lv["prod"]) == {"trunk", "main", "master", "deploy"}


def test_normalize_precedence_declared_beats_discovery_beats_default():
    with clean_ci_env():
        lv = bl.normalize({"dev": "devx", "staging": "stx"}, {},
                          heads=["development", "qa"], default="main")
        assert (lv["dev"], lv["staging"]) == ("devx", "stx")
        assert lv["source"] == "declared"
        lv = bl.normalize({}, {}, heads=["development", "qa"], default="main")
        assert (lv["dev"], lv["staging"]) == ("development", "qa")
        assert lv["source"] == "discovered"


def test_normalize_env_fills_gaps():
    with clean_ci_env(CI_DEV_BRANCH="envd", CI_STAGING_BRANCH="envs"):
        lv = bl.normalize({}, bl.env_overrides(), heads=[], default="main")
    assert (lv["dev"], lv["staging"]) == ("envd", "envs")


def test_normalize_declared_beats_env():
    with clean_ci_env(CI_DEV_BRANCH="envd"):
        lv = bl.normalize({"dev": "decl"}, {}, heads=[], default="main")
    assert lv["dev"] == "decl"


def test_normalize_always_returns_levels():
    with clean_ci_env():
        lv = bl.normalize({}, {}, heads=[], default="main")
    assert lv is not None and lv["dev"] == "development"


def test_collision_flag():
    with clean_ci_env():
        lv = bl.normalize({"dev": "main"}, {}, heads=[], default="main")
    assert lv["collision"] is True


def test_default_equal_to_dev_not_counted_prod():
    """Fresh gated repos switch their default branch to the dev level — that
    default must NOT be unioned into prod."""
    with clean_ci_env():
        lv = bl.normalize({}, {}, heads=["development", "main"], default="development")
    assert lv["dev"] == "development"
    assert set(lv["prod"]) == {"main", "master"}
    assert lv["collision"] is False


def test_declared_dev_default_no_collision():
    with clean_ci_env():
        lv = bl.normalize({"dev": "develop", "staging": "qa",
                           "prod": ["main", "master"]},
                          {}, heads=["develop", "main"], default="develop")
    assert set(lv["prod"]) == {"main", "master"}
    assert lv["collision"] is False


def test_render_staging_name_fallback():
    assert bl.render_staging_name({"staging": "qa"}) == "qa"
    assert bl.render_staging_name({"staging": ""}) == "staging"
