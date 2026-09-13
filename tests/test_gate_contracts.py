"""Tier content contracts — deep assertions on REALLY rendered T1/T2/T3 YAML.

Absorbs scratch/check_triggers.py and extends it: trigger shapes, job-graph
wiring, DB-drill safety, kill switches, concurrency, and level-name injection
(default AND custom ladder names) for all three tiers.
"""
import yaml

from _util import (render_template, jobs_of, triggers_of, parse_workflow_yaml,
                   concurrency_of)

T1 = "github-fast-checks"
T2 = "github-staging"
T3 = "github-release-heavy"


def _render(name, extra=None):
    _, files = render_template(name, extra)
    assert len(files) == 1, f"{name}: expected exactly one workflow file"
    return next(iter(files.values()))


# ══ TIER 1 — fast-checks ════════════════════════════════════════════════════

def test_t1_default_trigger_shape():
    on = triggers_of(_render(T1))
    ig = on.get("push", {}).get("branches-ignore") or []
    assert {"main", "master", "staging"}.issubset(set(ig)), ig
    assert "branches" not in on.get("push", {})  # push stays unfiltered
    assert "pull_request" in on and "workflow_dispatch" in on and "merge_group" in on
    assert "schedule" not in on


def test_t1_custom_ladder_names_injected():
    on = triggers_of(_render(T1, {"staging_branch": "qa", "prod_branches": "trunk,deploy"}))
    assert set(on["push"]["branches-ignore"]) == {"trunk", "deploy", "qa"}


def test_t1_pull_request_unfiltered():
    on = triggers_of(_render(T1))
    pr = on.get("pull_request")
    assert pr is not None
    assert "branches" not in pr and "branches-ignore" not in pr  # every PR runs T1
    assert "paths-ignore" in pr  # docs-only skips, but the event still fires


def test_t1_concurrency_group_present():
    cc = concurrency_of(_render(T1))
    assert "group" in cc, "T1 concurrency missing group"


def test_t1_stack_filters_byte_identical_across_jobs():
    text = _render(T1)
    jobs = jobs_of(text)
    filters = []
    for job in jobs.values():
        for step in job.get("steps", []):
            step_with = step.get("with") or {}
            if "filters" in step_with:
                filters.append(step_with["filters"])
    assert len(filters) >= 3, f"expected >=3 filter steps, got {len(filters)}"
    assert all(f == filters[0] for f in filters), "STACK_FILTERS diverged between jobs"
    # the shared block actually covers every stack
    for marker in ("py:", "node:", "go:", "php:", "deno:"):
        assert marker in filters[0], f"STACK_FILTERS missing {marker}"


def test_t1_kill_switches_and_timeouts():
    jobs = jobs_of(_render(T1))
    assert any("FASTCHECK_NO_TESTS" in str(j.get("if", "")) for j in jobs.values())
    assert any("FASTCHECK_NO_AUDIT" in str(j.get("if", "")) for j in jobs.values())
    for jid, job in jobs.items():
        assert "timeout-minutes" in job, f"T1 job {jid} missing timeout"


def test_t1_every_job_runs_on_pr_and_dispatch():
    """No T1 job may exclude pull_request events (the merge-blocking tier must
    always be able to report on PRs). Kill-switch ifs are fine."""
    jobs = jobs_of(_render(T1))
    for jid, job in jobs.items():
        cond = str(job.get("if", ""))
        if "event_name" in cond:
            assert "pull_request" in cond, \
                f"T1 job {jid} if-condition filters out PRs: {cond[:120]}"


# ══ TIER 2 — staging ════════════════════════════════════════════════════════

T2_WORKERS = ["backup-drill", "n-minus-one", "rotation-check", "multi-worker",
              "chaos-drill", "dns-preflight", "tls-check", "external-smoke",
              "dast-baseline", "k6-load", "staging-smoke"]


def test_t2_default_trigger_shape():
    on = triggers_of(_render(T2))
    assert on.get("push", {}).get("branches") == ["staging"]
    assert "workflow_dispatch" in on
    assert "schedule" not in on and "pull_request" not in on


def test_t2_custom_staging_name_injected():
    on = triggers_of(_render(T2, {"staging_branch": "qa"}))
    assert on["push"]["branches"] == ["qa"]


def test_t2_changes_job_feeds_all_workers():
    jobs = jobs_of(_render(T2))
    assert "changes" in jobs
    for w in T2_WORKERS:
        assert w in jobs, f"T2 missing worker {w}"
    # service-container workers (postgres boots) MUST wait for the diff gate
    for w in ("backup-drill", "n-minus-one", "multi-worker", "k6-load", "staging-smoke"):
        needs = jobs[w].get("needs") or []
        needs = [needs] if isinstance(needs, str) else needs
        assert "changes" in needs, f"T2 worker {w} does not depend on changes"


def test_t2_summary_needs_complete_graph():
    jobs = jobs_of(_render(T2))
    summary_name = next((k for k in ("staging-summary", "summary", "gate-summary")
                         if k in jobs), None)
    assert summary_name, "T2 summary job missing"
    needs = jobs[summary_name].get("needs") or []
    needs = [needs] if isinstance(needs, str) else needs
    assert set(needs) == set(jobs) - {summary_name}, \
        f"summary needs {sorted(needs)} != all other jobs {sorted(set(jobs) - {summary_name})}"


def test_t2_db_drills_safety():
    text = _render(T2)
    assert "PGPASSWORD" in text
    assert "ON_ERROR_STOP" in text
    assert "stage_restore" in text  # restore lands in a FRESH database


def test_t2_no_live_overrides():
    text = _render(T2)
    assert "STAGING_NO_LIVE" in text
    assert "ignoring live STAGING_URL" in text or "forcing ephemeral" in text


def test_t2_live_jobs_gated_on_staging_url():
    jobs = jobs_of(_render(T2))
    live_jobs = [jid for jid, j in jobs.items()
                 if "inputs.staging_url" in str(j.get("if", ""))
                 or "vars.STAGING_URL" in str(j.get("if", ""))]
    assert len(live_jobs) >= 3, f"live jobs not URL-gated: {live_jobs}"


def test_t2_kill_switches_at_job_level():
    jobs = jobs_of(_render(T2))
    switches = ["STAGING_NO_DB_DRILLS", "STAGING_NO_K6", "STAGING_NO_LIVE",
                "STAGING_NO_ROTATION"]
    joined_ifs = " ".join(str(j.get("if", "")) for j in jobs.values())
    for s in switches:
        assert s in joined_ifs, f"T2 kill switch {s} missing"
    # kill switches live on JOB ifs (not buried in steps)
    for jid, j in jobs.items():
        for step in j.get("steps", []):
            assert "STAGING_NO_" not in str(step.get("if", "")), \
                f"{jid}: kill switch belongs at job level"


def test_t2_timeouts_and_concurrency():
    assert "group" in concurrency_of(_render(T2)), "T2 concurrency missing group"
    jobs = jobs_of(_render(T2))
    for jid, job in jobs.items():
        assert "timeout-minutes" in job, f"T2 job {jid} missing timeout"


# ══ TIER 3 — release-heavy ══════════════════════════════════════════════════

def test_t3_trigger_shape_default():
    on = triggers_of(_render(T3))
    push = on.get("push", {})
    assert push.get("tags") == ["v*"]
    assert {"main", "master"}.issubset(set(push.get("branches") or []))
    assert "workflow_dispatch" in on
    assert "schedule" not in on and "pull_request" not in on


def test_t3_custom_prod_branches_injected():
    on = triggers_of(_render(T3, {"prod_branches": "trunk,deploy"}))
    assert set(on["push"]["branches"]) == {"trunk", "deploy"}


def test_t3_per_ref_concurrency():
    group = concurrency_of(_render(T3)).get("group", "")
    assert "github.ref" in group, f"T3 concurrency not per-ref: {group}"


def test_t3_key_jobs_exist_with_conditions():
    jobs = jobs_of(_render(T3))
    for jid in ("sbom-cyclonedx", "env-audit"):
        assert jid in jobs, f"T3 missing {jid}"
        assert str(jobs[jid].get("if", "")), f"T3 {jid} must be conditionally gated"
    # tests-detected style perf/contract jobs exist
    assert any("tests/test_perf_smoke" in str(j.get("if", "")) for j in jobs.values())


def test_t3_continue_on_error_policy():
    """dist download after a green build-full must fail loudly; the ONLY
    sanctioned soft-fail sites are rollback-readiness's optional artifact fetch
    and release-summary's optional test-report fetch (coverage/junit — the
    summary degrades gracefully without them, unlike dist artifacts)."""
    OPTIONAL_FETCH = "Retrieve test reports for summary"
    jobs = jobs_of(_render(T3))
    for jid, job in jobs.items():
        needs = job.get("needs") or []
        needs = [needs] if isinstance(needs, str) else needs
        if "build-full" in needs:
            assert job.get("continue-on-error") is not True, \
                f"T3 {jid}: dist path must fail loudly"
            for step in job.get("steps", []):
                if str(step.get("name", "")).startswith(OPTIONAL_FETCH):
                    continue
                assert step.get("continue-on-error") is not True, \
                    f"T3 {jid}: step-level continue-on-error on dist path"
    soft = [jid for jid, job in jobs.items()
            if job.get("continue-on-error")
            or any(s.get("continue-on-error") for s in job.get("steps", []))]
    assert set(soft) <= {"rollback-readiness", "release-summary"}, \
        f"unexpected continue-on-error: {soft}"


def test_t3_scoped_restore_keys():
    """Every cache's restore-keys must be a version-scoped PREFIX of its key
    (e.g. key `trivy-0.73.0-linux_x64` -> restore `trivy-0.73.0-`), never a
    cross-version or cross-tool wildcard."""
    jobs = jobs_of(_render(T3))
    for jid, job in jobs.items():
        for step in job.get("steps", []):
            if step.get("uses", "").startswith("actions/cache"):
                step_with = step.get("with") or {}
                key = str(step_with.get("key", ""))
                rks = str(step_with.get("restore-keys", "")).strip()
                if not rks:
                    continue
                first = rks.splitlines()[0].strip()
                assert key.startswith(first.rstrip("-").rstrip("_")), \
                    f"{jid}: restore-keys '{first}' not a version-scoped prefix of key '{key}'"


def test_t3_summary_and_rollback_graph():
    jobs = jobs_of(_render(T3))
    names = set(jobs)
    for jid, job in jobs.items():
        needs = job.get("needs") or []
        needs = [needs] if isinstance(needs, str) else needs
        for n in needs:
            assert n in names, f"T3 {jid} needs unknown job '{n}'"
    summary_jobs = [jid for jid in names if "summary" in jid]
    assert summary_jobs, "T3 summary job missing"


def test_t3_timeouts_everywhere():
    jobs = jobs_of(_render(T3))
    for jid, job in jobs.items():
        assert "timeout-minutes" in job, f"T3 job {jid} missing timeout"


# ══ cross-tier invariants ═══════════════════════════════════════════════════

def test_all_tiers_yaml_valid_with_custom_ladder():
    """The exact names a company might declare must render valid YAML everywhere."""
    import re
    extra = {"staging_branch": "qa", "prod_branches": "trunk,release"}
    for name in (T1, T2, T3):
        text = _render(name, extra)
        data = parse_workflow_yaml(text)
        assert "jobs" in data
        # ${{ }} GitHub expressions legitimately survive rendering
        stripped = re.sub(r"\$\{\{[^}]*\}\}", "", text)
        assert "{{" not in stripped and "{%" not in stripped, f"{name}: unrendered tokens"


def test_no_shellcheck_directive_lookalike_comments():
    """A comment line like `#   shellcheck findings ...` parses as a shellcheck
    DIRECTIVE (`# shellcheck key=value`) and fails the secrets job with
    SC1073/SC1072 (live-run finding). No tier comment may look like one."""
    import re
    for name in (T1, T2, T3):
        text = _render(name)
        for i, line in enumerate(text.splitlines(), 1):
            stripped = line.strip()
            if stripped.startswith("#") and re.match(r"#\s*shellcheck\s+\w", stripped):
                assert False, f"{name}:{i}: comment parses as a shellcheck directive: {line[:80]}"


def test_tier_headers_present():
    for name, header in ((T1, "TIER 1"), (T2, "TIER 2"), (T3, "TIER 3")):
        assert header in _render(name), f"{name}: tier header lost"


def test_shims_compose_tiers_and_ship_no_workflows():
    import json
    import os
    from _util import template_tools
    tt = template_tools()
    for prof in ("python", "node", "generic"):
        name = f"github-ci-security-{prof}"
        tdir = tt._template_dir(name)
        meta = json.load(open(os.path.join(tdir, "template.json"), encoding="utf-8"))
        wf = [f.get("dest") for f in meta.get("files", [])
              if str(f.get("dest", "")).startswith(".github/")]
        assert wf == [], f"shim {prof} ships workflow files: {wf}"
        uses = [u["template"] if isinstance(u, dict) else u for u in meta.get("uses", [])]
        assert T1 in uses and T3 in uses, f"shim {prof} must compose T1+T3: {uses}"


def test_tiers_exactly_one_workflow_file_each():
    import os
    from _util import template_tools
    tt = template_tools()
    for name in (T1, T2, T3):
        tdir = tt._template_dir(name)
        files = os.listdir(os.path.join(tdir, "files"))
        assert files == [f for f in files if f.endswith(".yml.j2")] and len(files) == 1, \
            f"{name}: must ship exactly one workflow file, found {files}"


# ══ AGENT-NATIVE OUTPUTS — gate-summary.json / coverage+junit / generic fallbacks ══

def test_gate_summary_emitted_and_uploaded_all_tiers():
    for tier, art in ((T1, 'gate-summary-T1'), (T2, 'gate-summary-T2'), (T3, 'gate-summary-T3')):
        text = _render(tier)
        assert '"schema": "gate-summary/v1"' in text, f'{tier}: schema marker missing'
        assert f'name: {art}' in text, f'{tier}: summary artifact upload missing'
        assert '_gate/gate-summary.json' in text, f'{tier}: emitter target path missing'
        assert 'retention-days: 30' in text, f'{tier}: summary retention missing'


def test_t1_coverage_and_junit_reports():
    text = _render(T1)
    for frag in ('--cov-report=json:_gate/coverage.json', '--junitxml=_gate/junit.xml',
                 'pytest-cov', 'COVERAGE_MIN'):
        assert frag in text, f'T1 missing: {frag}'
    jobs = jobs_of(_render(T1))
    ups = [s for s in jobs['tests']['steps'] if str(s.get('name', '')).startswith('Upload test reports')]
    assert ups and ups[0].get('if') == 'always() && !cancelled()', 'report upload must run even on failure'
    assert (ups[0].get('with') or {}).get('name') == 'gate-reports-T1'


def test_t3_tests_detected_coverage_and_junit():
    text = _render(T3)
    for frag in ('--cov-report=json:_gate/coverage.json', '--junitxml=_gate/junit.xml'):
        assert frag in text, f'T3 missing: {frag}'
    jobs = jobs_of(_render(T3))
    ups = [s for s in jobs['tests-detected']['steps']
           if (s.get('with') or {}).get('name') == 'gate-reports-release']
    assert ups, 'T3 tests-detected report upload missing'


def test_coverage_floor_is_report_only_by_default():
    # rendered default is the literal '0' — no repo fails on coverage out of the box
    for tier in (T1, T3):
        text = _render(tier)
        assert "COVERAGE_MIN: '0'" in text, f'{tier}: floor must bake to 0 by default'
        assert "coverage_min | default(0)" not in text  # jinja must not leak into output
    assert 'below minimum' in _render(T1), 'enforcement snippet present but gated behind min > 0'


def test_t3_endpoint_smoke_docker_fallback():
    text = _render(T3)
    jobs = jobs_of(_render(T3))
    ep = jobs['endpoint-smoke']
    assert "hashFiles('Dockerfile')" in ep['if'], 'must boot for Dockerfile-only repos'
    body = '\n'.join(str(s.get('run', '')) for s in ep['steps'])
    assert 'docker build -t gate-endpoint-smoke' in body
    assert 'docker logs gate-es' in body, 'failure path must dump container logs'
    py_i = body.find('Python path')
    dk_i = body.find('Docker fallback')
    sk_i = body.find('no runnable app detected')
    assert -1 < py_i < dk_i < sk_i, 'ordering: python < docker-fallback < final-skip'


def test_t3_openapi_lint_contract():
    import re
    text = _render(T3)
    jobs = jobs_of(_render(T3))
    job = jobs.get('openapi-lint')
    assert job, 'openapi-lint job missing'
    cond = job['if'] or ''
    assert 'RELEASE_NO_OPENAPI' in cond and "hashFiles('openapi.yaml')" in cond
    run_body = '\n'.join((s.get('run') or '') for s in job['steps'])
    m = re.search(r'@redocly/cli@(\d+\.\d+\.\d+)\b', run_body)
    assert m, 'redocly CLI must be pinned to an exact version in rendered output'
    rel = jobs['release-summary']
    needs = rel['needs']
    needs = needs if isinstance(needs, list) else [needs]
    assert 'openapi-lint' in needs, 'release-summary must aggregate openapi-lint'
    assert 'R_OPENAPI' in text


def test_t2_t3_push_triggers_have_no_native_paths_filters():
    # native paths+branches combos on push are a phantom-run hazard (zero-job
    # failure records observed on non-matching branches) — diff-gating belongs
    # in the dorny changes job (T2) / hashFiles guards (T3) instead.
    for tier in (T2, T3):
        on = triggers_of(_render(tier))
        push = on.get('push', {})
        assert 'paths-ignore' not in push and 'paths' not in push, \
            f'{tier}: native paths filter on push trigger'


def test_perf_stub_templates_skip_gracefully_when_no_app():
    py_files = render_template('github-ci-security-python')[1]
    py_body = '\n'.join(str(v) for v in py_files.values())
    assert 'importorskip' in py_body, 'python perf stub must skip without app package'
    assert "import app.main']" not in py_body
    node_files = render_template('github-ci-security-node')[1]
    node_body = '\n'.join(str(v) for v in node_files.values())
    assert 'PERF-SKIP' in node_body, 'node perf stub must skip without app entry'
    assert '"../src/main.ts"' not in node_body


# ══ REFINEMENT ROUND 2 — R1 repowise selection / R2 keploy replay / R3 strix / R4 junit distill ══

import re


def test_t1_test_selection_lane_is_opt_in_and_pinned():
    text = _render(T1)
    jobs = jobs_of(text)
    lane = [s for s in jobs['tests']['steps'] if s.get('id') == 'testsel']
    assert len(lane) == 1, 'exactly one selection lane expected'
    cond = lane[0]['if'] or ''
    assert "vars.FASTCHECK_TESTSEL == 'true'" in cond, 'selection must be opt-in'
    run_body = lane[0].get('run') or ''
    rv = (lane[0].get('env') or {}).get('REPOWISE_VERSION', '')
    assert re.fullmatch(r'\d+\.\d+\.\d+', rv), f'repowise must be pinned to exact version, got {rv!r}'
    # safety contracts: empty/unknown -> full suite; never a hard skip
    for marker in ('selection not beneficial — running full suite',
                   'no merge-base — running full suite',
                   'repowise index failed — running full suite'):
        assert marker in run_body, f'missing fallback: {marker}'
    # pytest consumes the lane output
    pytest_step = [s for s in jobs['tests']['steps'] if str(s.get('name', '')).startswith('Python — pytest')]
    assert pytest_step and '$TARGETS' in (pytest_step[0].get('run') or '')


def test_t1_summary_distills_junit_failures():
    text = _render(T1)
    assert '"test_failures"' in text and '_gate-in/junit.xml' in text
    assert 'failures_artifact' in text


def test_t3_summary_distills_junit_failures():
    text = _render(T3)
    assert '"test_failures"' in text and '_gate-in/junit.xml' in text
    assert 'gate-reports-release' in text


def test_t2_keploy_replay_contract():
    jobs = jobs_of(_render(T2))
    job = jobs.get('keploy-replay')
    assert job, 'keploy-replay job missing'
    cond = job['if'] or ''
    assert "vars.STAGING_NO_KEPLOY != 'true'" in cond, 'kill switch missing'
    assert "hashFiles('keploy/**')" in cond, 'must boot only with recorded suites'
    body = '\n'.join((s.get('run') or '') for s in job['steps'])
    m = re.search(r'keploy/releases/download/\$\{KEPLOY_VERSION\}/keploy_linux_amd64\.tar\.gz', body)
    assert m, 'keploy binary must come from a pinned release asset'
    rel = jobs['staging-summary']
    needs = rel['needs']
    needs = needs if isinstance(needs, list) else [needs]
    assert 'keploy-replay' in needs, 'staging-summary must aggregate keploy-replay'
    assert 'R_KEPLOY' in _render(T2)


def test_t3_strix_pentest_contract():
    text = _render(T3)
    jobs = jobs_of(text)
    job = jobs.get('strix-pentest')
    assert job, 'strix-pentest job missing'
    cond = job['if'] or ''
    assert "github.event_name == 'workflow_dispatch'" in cond, 'dispatch-only — never on push/schedule'
    assert "vars.STRIX_ENABLED == 'true'" in cond and 'LLM_API_KEY' in cond, 'double opt-in required'
    on = triggers_of(text)
    assert 'schedule' not in on, 'LLM spend must never be automatic via cron'
    body = '\n'.join((s.get('run') or '') for s in job['steps'])
    sv = next(((s.get('env') or {}).get('STRIX_VERSION', '') for s in job['steps']
               if (s.get('env') or {}).get('STRIX_VERSION')), '')
    assert re.fullmatch(r'\d+\.\d+\.\d+', sv), f'strix-agent must be pinned to exact version, got {sv!r}'
    ups = [s for s in job['steps'] if (s.get('with') or {}).get('name') == 'strix-findings']
    assert ups and ups[0].get('if') == 'always() && !cancelled()'
    rel = jobs['release-summary']
    needs = rel['needs']
    needs = needs if isinstance(needs, list) else [needs]
    assert 'strix-pentest' in needs and 'R_STRIX' in text


import re  # noqa: E402  (used by selection-lane assertions above)


# ══ REFINEMENT ROUND 3 — P1 release-watch · P2 k6 scenarios · P3 chaos · P4 DAST · P5 launch checklist ══


def test_t2_k6_scenario_pack_contract():
    text = _render(T2)
    assert "K6_SCENARIO" in text, "scenario variable missing"
    for scenario in ("smoke", "load", "soak", "spike", "breakpoint"):
        assert scenario in text, f"k6 scenario '{scenario}' missing"
    assert "--summary-export=_gate/k6-summary.json" in text, "k6 summary export missing"
    assert "k6-capacity/v1" in text, "machine-readable capacity doc missing"
    assert "K6_TARGET_RPS" in text, "capacity target variable missing"
    jobs = jobs_of(text)
    arts = [str((s.get('with') or {}).get('name')) for s in jobs['k6-load']['steps']]
    assert 'k6-capacity-T2' in arts, "capacity artifact upload missing from k6-load"


def test_t2_chaos_drill_contract():
    jobs = jobs_of(_render(T2))
    job = jobs.get('chaos-drill')
    assert job, 'chaos-drill job missing'
    cond = job['if'] or ''
    assert "vars.STAGING_NO_CHAOS != 'true'" in cond, 'kill switch missing'
    assert "needs.changes.outputs.backend" in cond, 'must obey the backend diff gate'
    body = '\n'.join((s.get('run') or '') for s in job['steps'])
    assert 'docker run -d --name chaos-db' in body, 'disposable DB must be docker-run'
    assert 'docker stop chaos-db' in body and 'docker start chaos-db' in body, 'chaos stop/start missing'
    assert 'DATABASE_URL' in body, 'drill DB wiring missing'
    assert 'never dialed the drill DB' in body, 'non-DB apps must skip, not fail'
    assert 'did not recover' in body, 'recovery assertion missing'
    assert 'services:' not in str(job), 'a services container cannot be stopped — use a disposable container'


def test_t2_dast_baseline_contract():
    jobs = jobs_of(_render(T2))
    job = jobs.get('dast-baseline')
    assert job, 'dast-baseline job missing'
    cond = job['if'] or ''
    assert "vars.STAGING_NO_DAST != 'true'" in cond, 'kill switch missing'
    assert "vars.STAGING_URL" in cond, 'must be live-URL gated'
    ups = [s for s in job['steps']
           if str(s.get('uses', '')).startswith('zaproxy/action-baseline@')]
    assert ups, 'ZAP baseline action missing'
    assert re.search(r'zaproxy/action-baseline@[0-9a-f]{40}\b', ups[0]['uses']), \
        'ZAP action must be SHA-pinned'
    with_ = ups[0].get('with') or {}
    assert with_.get('fail_action') is True, 'ZAP must fail the job on alerts'
    assert (with_.get('artifact_name') or '') == 'zap-baseline-T2'


def test_t2_summary_aggregates_new_lanes():
    jobs = jobs_of(_render(T2))
    needs = jobs['staging-summary']['needs']
    needs = [needs] if isinstance(needs, str) else needs
    assert 'chaos-drill' in needs and 'dast-baseline' in needs
    text = _render(T2)
    assert 'R_CHAOS' in text and 'R_DAST' in text


def test_t3_release_watch_contract():
    text = _render(T3)
    jobs = jobs_of(text)
    job = jobs.get('release-watch')
    assert job, 'release-watch job missing'
    cond = job['if'] or ''
    assert "vars.RELEASE_WATCH_ENABLED == 'true'" in cond, 'opt-in gate missing'
    assert "vars.DEPLOY_PROVIDER != ''" in cond, 'provider gate missing'
    assert 'needs.release-summary.result' in cond, 'watch must follow a green release-summary'
    assert 'secrets.' not in cond, 'secrets must never appear in if: conditions'
    needs = job.get('needs') or []
    needs = [needs] if isinstance(needs, str) else needs
    assert 'release-summary' in needs
    body = '\n'.join((s.get('run') or '') for s in job['steps'])
    assert 'vercel rollback' in body and 'wrangler rollback' in body, 'provider rollback commands missing'
    assert 'AUTO_ROLLBACK' in str(job), 'auto-rollback gate missing'
    ups = [s for s in job['steps'] if (s.get('with') or {}).get('name') == 'release-watch-report']
    assert ups and ups[0].get('if') == 'always() && !cancelled()'


def test_t3_launch_checklist_in_release_summary():
    text = _render(T3)
    assert 'Public-launch checklist' in text, 'launch checklist missing from release summary'
    assert '"launch_checklist_human"' in text, 'machine-readable checklist missing'
    for marker in ('privacy', 'status page', 'capacity', 'accessibility'):
        assert marker in text, f'launch checklist missing: {marker}'
