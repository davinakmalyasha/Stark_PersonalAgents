"""ci_gate.push_if_changed targeting matrix -- real git, local bare remotes.

The workspace's origin URL looks like https://github.com/testowner/testrepo.git
but rewrites to a local bare path via insteadOf, so the REAL
remote_owner_name parsing + push logic run fully offline.
"""
import json
import os

from _util import (ci_gate, clean_ci_env, commit_file, git_ok, initial_commit_and_push,
                   make_repo_pair, remote_heads, remote_sha, require_git,
                   temp_dir, write_file, Skip)

cg = ci_gate()


def _setup(base, name, seed_branch="main"):
    require_git()
    remote, ws = make_repo_pair(base, name)
    initial_commit_and_push(ws, seed_branch)
    return remote, ws


# -- S1 bootstrap: prod-checked workspace -> dev branch created via refspec --

def test_s1_bootstrap_creates_dev_branch_only():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s1")
        main_sha = remote_sha(remote, "main")
        write_file(os.path.join(ws, "a.txt"), "a\n")  # uncommitted change
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s1 change")
        assert r["pushed"] is True, r["note"]
        assert r["branch"] == "development"
        assert r["repo"] == "testowner/testrepo"
        assert "development" in remote_heads(remote)
        assert remote_sha(remote, "main") == main_sha  # prod untouched


# -- S2 adopt-existing dev branch -------------------------------------------

def test_s2_adopts_existing_dev_branch():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s2")
        git_ok(ws, "push", "origin", "HEAD:refs/heads/development")
        git_ok(ws, "checkout", "-B", "development")
        git_ok(ws, "pull", "origin", "development")
        write_file(os.path.join(ws, "a.txt"), "a\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s2 change")
        assert r["pushed"] is True and r["branch"] == "development", r["note"]
        assert remote_sha(remote, "development") == r["sha"]


# -- S3 config-obey: custom names from .harness-ci.json ----------------------

def test_s3_config_names_obeyed():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s3")
        write_file(os.path.join(ws, ".harness-ci.json"), json.dumps(
            {"branches": {"dev": "develop", "staging": "qa", "prod": ["trunk"]}}))
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s3 change")
        assert r["pushed"] is True and r["branch"] == "develop", r["note"]
        heads = remote_heads(remote)
        assert "develop" in heads and "trunk" not in heads and "qa" not in heads


def test_s3b_staging_level_never_autopushed():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s3b")
        write_file(os.path.join(ws, ".harness-ci.json"), json.dumps(
            {"branches": {"dev": "develop", "staging": "qa"}}))
        git_ok(ws, "add", "-A")
        git_ok(ws, "commit", "-m", "config")
        git_ok(ws, "push", "origin", "HEAD:refs/heads/qa")
        git_ok(ws, "checkout", "-B", "qa")
        git_ok(ws, "pull", "origin", "qa")
        qa_sha = remote_sha(remote, "qa")
        write_file(os.path.join(ws, "b.txt"), "b\n")  # uncommitted
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s3b change")
        assert r["pushed"] is True and r["branch"] == "develop", r["note"]
        assert remote_sha(remote, "qa") == qa_sha  # staging untouched


# -- S4 detached HEAD redirects to the dev level ------------------------------

def test_s4_detached_head_redirects_to_dev():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s4")
        git_ok(ws, "checkout", "--detach", "HEAD")
        write_file(os.path.join(ws, "d.txt"), "d\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s4 change")
        assert r["pushed"] is True and r["branch"] == "development", r["note"]


# -- S5/S6 switches -----------------------------------------------------------

def test_s5_auto_push_false_config():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s5")
        write_file(os.path.join(ws, ".harness-ci.json"),
                   json.dumps({"harness": {"auto_push": False}}))
        write_file(os.path.join(ws, "x.txt"), "x\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s5")
        assert r["pushed"] is False
        assert "auto-push disabled" in r["note"], r["note"]


def test_s6_env_kill_switch():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s6")
        write_file(os.path.join(ws, "x.txt"), "x\n")
        with clean_ci_env(CI_GATE_BRANCH="off"):
            r = cg.push_if_changed(ws, "s6")
        assert r["pushed"] is False and "auto-push disabled" in r["note"], r["note"]


# -- S7 feature branch respected as-is ----------------------------------------

def test_s7_feature_branch_respected():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s7")
        git_ok(ws, "checkout", "-B", "feature/oauth")
        write_file(os.path.join(ws, "f.txt"), "f\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s7 change")
        assert r["pushed"] is True and r["branch"] == "feature/oauth", r["note"]
        assert "feature/oauth" in remote_heads(remote)


# -- S8 divergence -> one rebase retry ----------------------------------------

def test_s8_divergence_rebase_retry():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s8")
        git_ok(ws, "push", "origin", "HEAD:refs/heads/development")
        git_ok(ws, "checkout", "-B", "development")
        git_ok(ws, "pull", "origin", "development")
        # another machine pushes to development
        ws2 = os.path.join(tmp, "s8-other")
        git_ok(ws, "fetch", "origin")
        git_ok(tmp, "clone", "--quiet", remote, ws2)
        git_ok(ws2, "config", "user.name", "Other")
        git_ok(ws2, "config", "user.email", "other@test.local")
        git_ok(ws2, "checkout", "-B", "development", "origin/development")
        write_file(os.path.join(ws2, "other.txt"), "other\n")
        git_ok(ws2, "add", "-A")
        git_ok(ws2, "commit", "-m", "remote-side commit")
        git_ok(ws2, "push", "origin", "development")
        # local workspace diverges: one local commit + one uncommitted change
        local_sha = commit_file(ws, "mine.txt", "mine\n")
        write_file(os.path.join(ws, "mine2.txt"), "mine2\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s8 local change")
        assert r["pushed"] is True, r["note"]
        assert remote_sha(remote, "development") == r["sha"]
        assert r["sha"] != local_sha  # rebased onto the remote-side commit
        log = git_ok(ws, "log", "--oneline", "-4")
        assert "s8 local change" in log and "remote-side commit" in log


# -- S9 collision: dev level inside prod set -> skip ---------------------------

def test_s9_prod_collision_skips():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s9")
        write_file(os.path.join(ws, ".harness-ci.json"), json.dumps(
            {"branches": {"dev": "trunk", "prod": ["trunk"]}}))
        write_file(os.path.join(ws, "x.txt"), "x\n")  # uncommitted trigger change
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s9")
        assert r["pushed"] is False and "collides" in r["note"], r["note"]


# -- S10 excludes + preview ----------------------------------------------------

def test_s10_excludes_and_preview():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s10")
        write_file(os.path.join(ws, "node_modules", "pkg", "index.js"), "junk\n")
        write_file(os.path.join(ws, ".env"), "SECRET=1\n")
        write_file(os.path.join(ws, "app.py"), "print('hi')\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s10 change")
        assert r["pushed"] is True, r["note"]
        assert "node_modules" in r["preview"] and "Skipped" in r["preview"]
        names = git_ok(ws, "show", "--name-only", "--format=", "HEAD")
        assert "app.py" in names
        assert "node_modules" not in names and ".env" not in names


# -- S11 no remote -> local commit only ----------------------------------------

def test_s11_no_remote_local_commit_only():
    with temp_dir() as tmp:
        require_git()
        ws = os.path.join(tmp, "s11-ws")
        os.makedirs(ws)
        git_ok(ws, "-c", "init.defaultBranch=main", "init")
        git_ok(ws, "config", "user.name", "T")
        git_ok(ws, "config", "user.email", "t@t")
        write_file(os.path.join(ws, "solo.txt"), "solo\n")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s11 change")
        assert r["pushed"] is False
        assert r["repo"] is None and r["sha"], r
        assert "no git remote" in r["note"]


# -- S12 no changes -> clean no-op ----------------------------------------------

def test_s12_no_changes_noop():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s12")
        with clean_ci_env():
            r = cg.push_if_changed(ws, "s12")
        assert r["pushed"] is False and r["note"] == "no changes" and r["sha"] == ""


# -- resolver integration through the real git IO -------------------------------

def test_resolve_levels_discovers_from_remote_heads():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s13")
        git_ok(ws, "push", "origin", "HEAD:refs/heads/qa")
        git_ok(ws, "push", "origin", "HEAD:refs/heads/development")
        with clean_ci_env():
            lv = cg.resolve_levels(ws)
        assert lv["dev"] == "development"
        assert lv["staging"] == "qa"
        assert lv["default_branch"] == "main"
        assert "main" in lv["prod"] and "master" in lv["prod"]


def test_push_preview_reports_state_without_touching():
    with temp_dir() as tmp:
        remote, ws = _setup(tmp, "s14")
        write_file(os.path.join(ws, "p.txt"), "p\n")
        preview = cg._push_preview(ws)
        assert "Will commit (1)" in preview and "p.txt" in preview
        clean = cg._push_preview(remote)  # not a worktree -> no changes path
        assert "no changes" in clean
