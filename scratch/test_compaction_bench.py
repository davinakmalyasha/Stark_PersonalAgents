"""Deterministic tests for the compaction bench harness + instrumentation.

Run:  venv312/Scripts/python.exe -m pytest scratch/test_compaction_bench.py -q
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from scratch.compaction_bench import (  # noqa: E402
    ambiguity_report,
    build_haystack,
    coverage,
    improvement_hints,
    mock_run,
    objective_coverage,
)


def test_haystack_contains_all_needles():
    history, needles, facts = build_haystack(3800)
    text = "\n".join(
        str(getattr(p, "content", ""))
        for m in history for p in getattr(m, "parts", []) or []
    )
    assert len(needles) == 6
    for needle in needles:
        assert needle["token"] in text
    assert "src/compaction_probe_a1b2c3.py" in facts["paths"][0]


def test_coverage_and_objective_math():
    needles = [
        {"id": "a", "token": "TOKEN_A"},
        {"id": "b", "token": "TOKEN_B"},
    ]
    cov = coverage(needles, "has TOKEN_A only")
    assert cov == {"total": 2, "found": 1, "rate": 0.5, "missing": ["b"]}
    obj = objective_coverage({"paths": ["x.py", "y.py"]}, "x.py exists")
    assert obj["paths"]["found"] == 1 and obj["paths"]["missing"] == ["y.py"]


def test_ambiguity_report_flags_dangling_and_vague():
    amb = ambiguity_report(
        "we changed the file and it was fixed; see missing_thing.py:42",
        known_paths=["src/known.py"],
    )
    assert amb["vague_references"] >= 1
    assert amb["dangling_refs"] == ["missing_thing.py:42"]
    assert amb["uncertain_markers"] == 0


def test_improvement_hints_map_missing_region_to_knob():
    hints = improvement_hints(
        {"trimmed": 2, "ledgers": 1, "ledger_tokens": 100},
        {"missing": ["seam_error"]},
        {"seam_error": "seam"},
    )
    joined = " ".join(hints)
    assert "COMPACTION_SEAM_OVERLAP_TOKENS" in joined
    assert "Trim events" in joined
    assert "under-driving detail" in joined


def test_mock_run_transports_every_needle_and_writes_metrics():
    report = mock_run(3800)
    assert report["summary_recall"]["rate"] == 1.0
    assert report["final_transport"]["rate"] == 1.0
    assert report["result"]["chunks"] >= 2
    assert report["result"]["overlap_tokens"] > 0
    assert report["result"]["duration_ms"] >= 0
    assert report["result"]["summary_tokens"] > 0
    # instrumentation: metrics jsonl was emitted for the sandbox project.
    metrics = os.path.join(report["project_path"], ".agent-memory", "compaction_metrics.jsonl")
    assert os.path.isfile(metrics)
    record = json.loads(open(metrics, encoding="utf-8").read().strip().splitlines()[-1])
    assert record["chunks"] == report["result"]["chunks"]


def test_metrics_jsonl_written_for_project():
    import tempfile

    from memory.compaction_engine import _emit_metrics

    d = tempfile.mkdtemp()
    _emit_metrics(d, {"chunks": 2, "ledgers": 1, "summary": "x" * 100, "full_map": "should-not-be-stored"})
    path = os.path.join(d, ".agent-memory", "compaction_metrics.jsonl")
    assert os.path.isfile(path)
    record = json.loads(open(path, encoding="utf-8").read().strip())
    assert record["chunks"] == 2
    assert "full_map" not in record  # large field excluded


if __name__ == "__main__":
    for fn in list(globals()):
        if fn.startswith("test_"):
            globals()[fn]()
            print(f"PASS {fn}")
