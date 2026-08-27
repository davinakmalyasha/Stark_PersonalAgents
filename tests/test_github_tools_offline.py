"""githubTools gate installer — fully offline via a scripted fake _gh_api.

The fake mimics `gh api` string responses (githubTools treats everything as
text) and STORES contents-PUT payloads so later GETs serve them back — that
makes the seed→fetch→parse→protect chain testable end-to-end with the REAL
renderer and REAL templates.
"""
import base64
import json
import os
import re

from _util import clean_ci_env, github_tools, temp_dir, write_file

gt = github_tools()
from _util import tools_stub
tools_stub().apply_template = __import__("_util").template_tools().apply_template

REPO = "owner/testrepo"


class FakeGH:
    """Scripted gh api. Routes are (pattern_in_joined_args, responder(args, joined)).
    Contents PUTs are always captured; contents GETs serve captured content."""
    def __init__(self, default_branch="main", heads=("main",), tree=()):
        self.routes = []
        self.puts = {}          # path -> bytes
        self.put_calls = []     # (path, bytes)
        self.put_shas = {}      # path -> sha supplied in the PUT body (overwrite)
        self.other_calls = []   # joined args
        self.protection = {}    # branch -> dict (from --input file)
        self.default_branch = default_branch
        self.heads = list(heads)
        self.tree = list(tree)
        self.refs = {}          # branch -> sha (40 chars) for git/ref GETs

    # -- helpers ------------------------------------------------------------
    def add(self, pattern, responder):
        self.routes.append((pattern, responder))

    def b64(self, text):
        return base64.b64encode(text.encode("utf-8")).decode("ascii")

    def seed_file(self, path, text, sha="deadbeefcafe"):
        """Pretend this file already exists remotely."""
        self.puts[path] = text.encode("utf-8")
        return sha

    # -- the fake _gh_api ---------------------------------------------------
    def __call__(self, args, timeout=30):
        joined = " ".join(str(a) for a in args)
        # contents PUT → capture and succeed (body via --input JSON file,
        # mirroring the Windows-safe production path)
        m = re.search(r"-X PUT repos/[^ ]+/contents/(.+?)(?: |$)", joined)
        if m and "--input" in args:
            path = m.group(1)
            with open(args[args.index("--input") + 1], encoding="utf-8") as f:
                body = json.load(f)
            self.puts[path] = base64.b64decode(body["content"])
            self.put_shas[path] = body.get("sha")
            self.put_calls.append((path, self.puts[path]))
            return '{"content": {"sha": "newsha"}}'
        # branch protection PUT → capture the --input JSON
        m = re.search(r"-X PUT repos/[^ ]+/branches/([^ /]+)/protection", joined)
        if m and "--input" in args:
            with open(args[args.index("--input") + 1], encoding="utf-8") as f:
                self.protection[m.group(1)] = json.load(f)
            return json.dumps({"required_status_checks": {"contexts": ["x"]}})
        # contents GET
        m = re.search(r"repos/[^ ]+/contents/(.+?)(?:\?ref=\S+)?(?: --jq|$)", joined)
        if m and "-X" not in args:
            path = m.group(1)
            if path in self.puts:
                if "--jq" in args and ".sha" in args[args.index("--jq") + 1]:
                    return "deadbeefsha"
                if "--jq" in args and ".content" in args[args.index("--jq") + 1]:
                    return self.b64(self.puts[path].decode("utf-8"))
        for pattern, responder in self.routes:
            if pattern in joined:
                return responder(args, joined)
        # built-in defaults for common endpoints
        if re.search(r"repos/[^ ]+ --jq \.default_branch", joined):
            return self.default_branch
        if "branches?per_page=100" in joined:
            return ",".join(self.heads)
        if re.search(r"git/ref/heads/(\S+) --jq \.ref", joined):
            b = re.search(r"git/ref/heads/(\S+) --jq", joined).group(1)
            return f"refs/heads/{b}" if b in self.refs else "Error: Not Found (404)"
        if re.search(r"git/ref/heads/(\S+) --jq \.object\.sha", joined):
            b = re.search(r"git/ref/heads/(\S+) --jq", joined).group(1)
            return self.refs.get(b, "Error: Not Found (404)")
        if "git/refs" in joined and "-X POST" in joined:
            ref = [a for a in args if str(a).startswith("ref=")][0]
            sha = [a for a in args if str(a).startswith("sha=")][0][4:]
            b = ref.split("heads/")[1]
            self.refs[b] = sha
            return '{"ref": "%s"}' % ref
        if "-X PATCH" in joined and "default_branch=" in joined:
            self.default_branch = [a for a in args
                                   if str(a).startswith("default_branch=")][0].split("=", 1)[1]
            return '{"default_branch": "%s"}' % self.default_branch
        if re.search(r"\buser --jq \.login\b", joined):
            return "owner"
        if "actions/runs?per_page=1" in joined:
            return "5"
        if "git/trees" in joined:
            return "\n".join(self.tree)
        self.other_calls.append(joined)
        return "Error: Not Found (404)"


def _install(fake):
    gt._gh_api = fake
    return fake


# ── _job_ids_from_yaml ──────────────────────────────────────────────────────

def test_job_ids_clean():
    yml = "name: x\non: push\njobs:\n  lint:\n    runs-on: x\n  tests:\n    runs-on: x\n  audit:\n    runs-on: x\n"
    assert gt._job_ids_from_yaml(yml) == ["lint", "tests", "audit"]


def test_job_ids_with_comments_and_blank_lines():
    yml = ("jobs:\n"
           "  alpha:  # top job\n"
           "    steps: []\n"
           "\n"
           "  beta:\n"
           "    steps: []\n")
    assert gt._job_ids_from_yaml(yml) == ["alpha", "beta"]


def test_job_ids_stops_at_next_top_level_key():
    yml = "jobs:\n  one:\n    steps: []\npermissions: read\n  ghost:\n"
    assert gt._job_ids_from_yaml(yml) == ["one"]


def test_job_ids_no_jobs_key():
    assert gt._job_ids_from_yaml("name: x\non: push\n") == []


def test_job_ids_ignores_nested_two_space_deeper_keys():
    yml = "jobs:\n  outer:\n    inner_deep: 1\n"
    assert gt._job_ids_from_yaml(yml) == ["outer"]


def test_workflow_job_ids_via_fake():
    fake = _install(FakeGH())
    fake.seed_file(".github/workflows/fast-checks.yml",
                   "jobs:\n  a:\n  b:\n  c:\n  d:\n")
    ids = gt._workflow_job_ids(REPO, ".github/workflows/fast-checks.yml", ref="main")
    assert ids == ["a", "b", "c", "d"]


def test_workflow_job_ids_missing_file():
    _install(FakeGH())
    assert gt._workflow_job_ids(REPO, ".github/workflows/nope.yml") == []


# ── _resolve_branch_levels ──────────────────────────────────────────────────

def test_resolve_declared_config_wins():
    fake = _install(FakeGH())
    cfg = {"branches": {"dev": "develop", "staging": "qa", "prod": ["trunk"]}}
    fake.seed_file(".harness-ci.json", json.dumps(cfg))
    with clean_ci_env():
        lv = gt._resolve_branch_levels(REPO)
    assert lv["dev"] == "develop"
    assert lv["staging"] == "qa"
    assert set(lv["prod"]) == {"trunk", "main", "master"}
    assert lv["declared"] is True


def test_resolve_discovery_from_heads():
    _install(FakeGH(heads=("main", "develop", "qa")))
    with clean_ci_env():
        lv = gt._resolve_branch_levels(REPO)
    assert lv["dev"] == "develop" and lv["staging"] == "qa"
    assert lv["declared"] is False


def test_resolve_env_overrides():
    _install(FakeGH(heads=("main",)))
    with clean_ci_env(CI_DEV_BRANCH="envd", CI_STAGING_BRANCH="envs"):
        lv = gt._resolve_branch_levels(REPO)
    assert lv["dev"] == "envd" and lv["staging"] == "envs"


def test_resolve_fallback_defaults():
    _install(FakeGH(heads=("main",)))
    with clean_ci_env():
        lv = gt._resolve_branch_levels(REPO)
    assert lv["dev"] == "development"
    assert lv["staging"] == "staging"  # render-name fallback for seeding
    assert set(lv["prod"]) == {"main", "master"}


def test_resolve_ignores_kill_switch():
    _install(FakeGH(heads=("main",)))
    with clean_ci_env(CI_GATE_BRANCH="off"):
        lv = gt._resolve_branch_levels(REPO)
    assert lv is not None and lv["dev"] == "development"


# ── ladder + default branch ─────────────────────────────────────────────────

def test_ensure_branch_exists_vs_create():
    fake = _install(FakeGH())
    fake.refs["development"] = "1" * 40
    out = gt._ensure_branch(REPO, "development", "2" * 40)
    assert "exists" in out
    out = gt._ensure_branch(REPO, "qa", "2" * 40)
    assert "created" in out and fake.refs.get("qa") == "2" * 40


def test_ensure_branch_ladder_skips_prod_and_empty():
    fake = _install(FakeGH())
    fake.refs["main"] = "3" * 40
    out = gt._ensure_branch_ladder(REPO, {"dev": "development", "staging": "",
                                          "prod": ["main", "master"]})
    assert "development: created" in out
    assert "qa" not in out and fake.refs.get("qa") is None


def test_set_default_branch():
    _install(FakeGH())
    out = gt._set_default_branch(REPO, "development")
    assert "switched to development" in out


# ── _protect_branch payload ─────────────────────────────────────────────────

def test_protect_branch_payload_shape():
    _install(FakeGH())
    checks = ["lint", "tests", "quick-audit", "secrets-workflow", "fast-summary"]
    out = gt._protect_branch(REPO, "main", checks)
    assert out.startswith("Protected main")
    body = gt._gh_api.protection["main"]
    assert body["required_status_checks"]["strict"] is True
    assert body["required_status_checks"]["contexts"] == checks
    assert body["enforce_admins"] is True
    assert body["required_pull_request_reviews"] is None
    assert body["restrictions"] is None


def test_protect_branch_empty_branch_noop():
    _install(FakeGH())
    assert gt._protect_branch(REPO, "", ["x"]) == ""


# ── _detect_stack ───────────────────────────────────────────────────────────

def test_detect_stack_matrix():
    cases = [
        (["package.json", "src/index.ts"], "node"),
        (["composer.json"], "php"),
        (["go.mod", "main.go"], "go"),
        (["pyproject.toml", "app.py"], "python"),
        (["requirements.txt"], "python"),
        (["setup.py"], "python"),
        (["README.md"], "generic"),
        ([], "python"),  # empty repo → python (graceful-skip profile)
    ]
    for tree, want in cases:
        _install(FakeGH(tree=tree))
        assert gt._detect_stack(REPO) == want, f"{tree} → {want}"


# ── _seed_gate ──────────────────────────────────────────────────────────────

def test_seed_gate_fresh_repo_seeds_everything():
    fake = _install(FakeGH(heads=("main",)))
    fake.refs["main"] = "4" * 40
    with clean_ci_env():
        msg = gt._seed_gate(REPO, profile="python", ref="main")
    assert msg.startswith("Seeded") and "profile: python" in msg, msg
    paths = {p for p, _ in fake.put_calls}
    for must in (".github/workflows/fast-checks.yml", ".github/workflows/staging.yml",
                 ".github/workflows/release-heavy.yml", ".harness-ci.json"):
        assert must in paths, f"missing {must}"
    # persisted branch map carries the resolved names
    cfg = json.loads(fake.puts[".harness-ci.json"].decode("utf-8"))
    assert cfg["branches"]["dev"] == "development"
    assert cfg["branches"]["staging"] == "staging"
    assert "main" in cfg["branches"]["prod"] and "master" in cfg["branches"]["prod"]
    assert cfg["harness"]["auto_push"] is True


def test_seed_gate_existing_files_kept_by_default():
    fake = _install(FakeGH(heads=("main",)))
    fake.refs["main"] = "4" * 40
    fake.seed_file("README.md", "CUSTOM README")
    fake.seed_file("requirements.txt", "flask==1.0\n")
    fake.seed_file(".github/workflows/fast-checks.yml", "jobs:\n  old:\n")
    with clean_ci_env():
        msg = gt._seed_gate(REPO, profile="python", ref="main")
    assert "kept" in msg
    paths = [p for p, _ in fake.put_calls]
    assert "README.md" not in paths
    assert "requirements.txt" not in paths
    assert ".github/workflows/fast-checks.yml" not in paths  # level-owned too, w/o force
    assert fake.puts["README.md"].decode() == "CUSTOM README"


def test_seed_gate_force_levels_overwrites_only_level_owned():
    fake = _install(FakeGH(heads=("main",)))
    fake.refs["main"] = "4" * 40
    for p in ("README.md", "requirements.txt", ".gitignore",
              ".github/workflows/fast-checks.yml", ".github/workflows/staging.yml",
              ".github/workflows/release-heavy.yml", ".harness-ci.json"):
        fake.seed_file(p, "OLD CONTENT")
    with clean_ci_env():
        msg = gt._seed_gate(REPO, profile="python", ref="main", force_levels=True)
    put = [p for p, _ in fake.put_calls]
    assert ".github/workflows/fast-checks.yml" in put
    assert ".github/workflows/staging.yml" in put
    assert ".github/workflows/release-heavy.yml" in put
    assert ".harness-ci.json" in put
    assert "README.md" not in put and "requirements.txt" not in put
    # overwrites MUST carry the existing file's sha (Contents API requirement)
    for p in (".github/workflows/fast-checks.yml", ".harness-ci.json"):
        assert fake.put_shas[p] == "deadbeefsha", f"{p}: overwrite without sha"
    # overwritten with REAL rendered content, not the OLD placeholder
    assert fake.puts[".harness-ci.json"].decode().startswith("{")
    assert "OLD CONTENT" not in fake.puts[".github/workflows/fast-checks.yml"].decode()
    assert "jobs:" in fake.puts[".github/workflows/fast-checks.yml"].decode()


# ── github_apply_gate full offline orchestration ────────────────────────────

def _fake_run_gh_create():
    """Fake `gh repo create <name> --private` → success (owner resolved via _gh_api)."""
    gt._run_gh = lambda args, timeout=30: (
        "Created repo owner/<name>" if args and args[0] == "repo" else "")


def test_apply_gate_end_to_end_offline():
    fake = _install(FakeGH(heads=("main",)))
    fake.refs["main"] = "5" * 40
    _fake_run_gh_create()
    with clean_ci_env():
        out = gt.github_apply_gate("newrepo", stack="python", gate="yes",
                                   create=True, private=True)
    # create path resolved the owner
    assert out.startswith("Created owner/newrepo."), out[:200]
    # ladder created, default switched, run waited, protection applied
    assert "Seeded" in out
    assert "Branch ladder" in out and "development: created" in out
    assert "Default branch switched to development" in out
    assert "First gate run" in out
    assert "Protected main" in out
    # protection contexts == REAL parsed job ids of the seeded T1 workflow
    t1_text = fake.puts[".github/workflows/fast-checks.yml"].decode("utf-8")
    ids = gt._job_ids_from_yaml(t1_text)
    assert len(ids) >= 3, f"real T1 should expose >=3 job ids, got {ids}"
    assert fake.protection["main"]["required_status_checks"]["contexts"] == ids
    # seeded tier ymls are valid YAML with the right trigger shapes
    import yaml
    for p in (".github/workflows/fast-checks.yml", ".github/workflows/staging.yml",
              ".github/workflows/release-heavy.yml"):
        data = yaml.safe_load(fake.puts[p].decode("utf-8"))
        assert isinstance(data, dict) and "jobs" in data, p


def test_apply_gate_skips_protection_when_contexts_unparseable():
    fake = _install(FakeGH(heads=("main",)))
    fake.refs["main"] = "5" * 40
    # pre-existing broken T1 → skip-if-exists keeps it → <3 ids → warning path
    fake.seed_file(".github/workflows/fast-checks.yml", "jobs:\n  only-one:\n")
    with clean_ci_env():
        out = gt.github_apply_gate(REPO, stack="python", gate="yes")
    assert "WARNING" in out and "protection SKIPPED" in out
    assert "main" not in fake.protection


def test_apply_gate_input_validation():
    _install(FakeGH())
    out = gt.github_apply_gate("owner/name", create=True, gate="yes")
    assert out.startswith("Error")  # plain name required with create=True
    out = gt.github_apply_gate("plainname", gate="yes")
    assert out.startswith("Error") and "owner/name" in out
    out = gt.github_apply_gate("owner/x", stack="cobol", gate="yes")
    assert out.startswith("Error") and "unknown stack" in out


def test_apply_gate_rerun_protects_prod_not_dev():
    """After the default-branch switch, a re-run must still protect the PROD
    level (main), never the dev level — even though the runtime default is
    now the dev branch."""
    fake = _install(FakeGH(default_branch="development",
                           heads=("main", "development", "staging")))
    fake.refs["main"] = "6" * 40
    fake.refs["development"] = "7" * 40
    fake.seed_file(".harness-ci.json", json.dumps(
        {"branches": {"dev": "development", "staging": "staging",
                      "prod": ["main", "master"]},
         "harness": {"auto_push": True}}))
    fake.seed_file(".github/workflows/fast-checks.yml",
                   "jobs:\n  a:\n  b:\n  c:\n")
    fake.seed_file(".github/workflows/staging.yml", "jobs:\n  s1:\n")
    fake.seed_file(".github/workflows/release-heavy.yml", "jobs:\n  r1:\n")
    with clean_ci_env():
        out = gt.github_apply_gate(REPO, stack="python", gate="yes")
    assert "Protected main" in out, out[:400]
    assert "Protected development" not in out
    assert "main" in fake.protection and "development" not in fake.protection


def test_apply_gate_declined_leaves_repo_alone():
    fake = _install(FakeGH(heads=("main",)))
    with clean_ci_env():
        out = gt.github_apply_gate(REPO, gate="no")
    assert "without the gate" in out
    assert fake.put_calls == []  # nothing written


def test_gate_checks_profiles_complete():
    from _util import template_tools
    tt = template_tools()
    for prof, checks in gt._GATE_CHECKS.items():
        assert checks, prof
        assert len(checks) == len(set(checks)), f"{prof}: duplicate contexts"
    assert set(gt._GATE_PROFILE_TEMPLATES) == set(gt._GATE_CHECKS)
    for prof, tpl in gt._GATE_PROFILE_TEMPLATES.items():
        assert tt._template_dir(tpl["security"]) is not None, f"{prof}: security shim missing"
        assert tt._template_dir(tpl["staging"]) is not None, f"{prof}: staging template missing"
