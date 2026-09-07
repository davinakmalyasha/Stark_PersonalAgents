"""ci_gate is now a no-op stub — auto-push removed."""
from _util import ci_gate

cg = ci_gate()

def test_stub_push_always_noop():
    import os
    from _util import temp_dir, make_repo_pair, initial_commit_and_push, write_file
    with temp_dir() as tmp:
        remote, ws = make_repo_pair(tmp, "stub")
        initial_commit_and_push(ws, "main")
        write_file(os.path.join(ws, "x.txt"), "x\n")
        r = cg.push_if_changed(ws, "msg")
        assert r["pushed"] is False
        assert "auto-push removed" in r["note"]

def test_stub_after_run_noop():
    assert cg.after_run("/tmp", "hi", "cid") is None
