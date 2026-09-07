# Agent Template Library — CI/Deploy Gate Only

Scope: reusable Jinja boilerplate is limited to the CI/deploy gate + project
scaffolds. Everything else (features, stacks, components, UI) is written by
the agent by hand with `edit_file` — there is no `template` tool exposed to
agents anymore. The only engine consumer is `github_apply_gate` / `github_init_project`.

## What lives here

```
templates/
├── config/     github-ci-* tiered workflows + docker/nginx
├── routine/    gitignore, requirements, pyproject, readme
└── project/    runnable skeletons (project-fastapi, project-frontend)
```

| category | templates |
|----------|-----------|
| config   | github-ci-security-python, github-ci-security-node, github-ci-security-generic, github-fast-checks, github-staging, github-release-heavy, dockerfile-python, docker-compose, nginx |
| routine  | gitignore-python, gitignore-node, requirements, pyproject, readme |
| project  | project-fastapi, project-frontend |

`template_new`/`template_pull`/`template_push` (catalog authoring + hub sync)
are removed from the agent tool surface; `apply_template` remains importable
internally for the gate render path only.

## Conventions (curation rules)

- `description` must be ONE line.
- Every template NEEDS `sample_vars` — CI FAILS a template without it.
- `apply_template` renders EVERYTHING before writing anything (no partial
  applies), skips existing files by default (`overwrite=true` to replace).
- Rendering is sandboxed (SandboxedEnvironment) — templates are untrusted input.

## The Gate (security + performance + bug detection)

Installed by `github_apply_gate` / `github_init_project` (with user confirmation),
non-destructively: existing project files are never overwritten.

### Branch-level ladder (3 tiers ↔ 3 levels)

The gate maps each tier to a branch LEVEL. Names are adaptive per company —
resolved from `.harness-ci.json` in the repo > remote branch scan > defaults —
and persisted back as `.harness-ci.json` so the agent harness auto-push obeys
the same map:

| Level | Default names | Tier | Fires on |
|---|---|---|---|
| dev | `development` | T1 fast-checks (~18s) | dev-level + feature-branch pushes, PRs |
| staging | `staging` | T2 staging (live proofs) | staging-level push (+ dispatch) |
| prod | `main`, `master` | T3 release-heavy | prod-level push + tags `v*` (+ dispatch) |

`github_apply_gate` idempotently creates the dev/staging branches, seeds the
workflows with the resolved names (`staging_branch` / `prod_branches` template
variables), writes `.harness-ci.json`, and protects the prod branch with the
Tier 1 check contexts. The harness auto-push (`ci_gate.py`) lands on the
dev-level branch only — production and staging are never auto-pushed.

**Profiles** (auto-detected): `package.json`→node · `composer.json`→php ·
`go.mod`→go · pyproject/requirements→python · none→generic (a repo is never ungated).

**What each tier actually runs** (all SHA-pinned and graceful-skipping — a fresh
repo stays green, a real project gets real enforcement):

- **T1 fast-checks (~18s, dev/feature pushes + PRs)**: gitleaks secrets ·
  actionlint+shellcheck · lint (ruff / eslint+tsc / gofmt+vet / php -l / deno
  lint) · tests (pytest / vitest / go test / phpunit / deno test) · quick audit
  (pip-audit / npm audit / govulncheck / composer / deno) — high-severity only.
  Diff-gated per stack; dispatch runs full.
- **T2 staging (staging-level push)**: backup drill with MD5 data round-trip ·
  alembic N-1 forward-upgrade head assert · rotation age (vault or heuristic) ·
  multi-worker burst · DNS/TLS preflight · external smoke · k6 load · live smoke
  with golden /ready semantics (200 pass · 404 warn · 503 fail). Live jobs skip
  at job level when unwired; `STAGING_NO_LIVE=true` forces ephemeral targets.
- **T3 release-heavy (prod-level push + tags `v*`)**: trivy image scan + size
  budget · CycloneDX SBOM (py/node/go/php/deno) + license denylist · env-audit ·
  zizmor workflow self-audit · schema-drift + migration round-trip · config
  validation · endpoint smoke · graceful shutdown · oasdiff contract diff ·
  perf benchmark · full build + static-asset integrity · tests-detected ·
  rollback readiness.

Perf budgets are universal defaults — tune after first green.

## Tooling

| tool            | purpose |
|-----------------|---------|
| `github_apply_gate`      | CI/deploy gate: seed tiered workflows (T1 fast-checks / T2 staging / T3 release-heavy), branch ladder, prod branch protection. Agent-visible (Deploy scope). |
| `github_apply_observability` | RUNTIME scaffold (health/ready + JSON logging + DSN-dormant Sentry). Warehouse-only — NOT in any agent scope; a future devops subagent calls it. Run before go-live so T2/T3 live-smoke has a real `/ready`. |

Runtime vs CI separation: `github_apply_gate` installs *checks that run on GitHub*;
`github_apply_observability` installs *files that run inside the app*. Use both
for a production-ready repo; each is non-destructive (skip-if-exists).

CI (`scripts/validate_templates.py`) re-renders every template with its sample vars on
every push AND YAML-parses every rendered workflow — a broken template fails the build in seconds.
