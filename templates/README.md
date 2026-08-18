# Agent Template Library

Reusable boilerplate for autonomous development. Every template is a folder with:

```
templates/<name>/
├── template.json   ← metadata: description, category, variables, files, notes, uses
└── files/          ← the Jinja2 boilerplate files (written by apply_template)
```

The engine never loads templates into context automatically — agents discover them
via `template_list()` / `template_list("<name>")` and instantiate with `apply_template()`.
Boilerplate generation moves from LLM output tokens to deterministic tool execution.

## The 5 levels

```
STACK      complete recipe for a greenfield project — composes existing templates,
           pre-connected (auth + crud + ci + deploy in ONE call). No new code.
FEATURE    a full vertical slice — multi-file, wired together (model+schema+router+
           service+tests). May reference components.
COMPONENT  one cohesive module — single file or a small page (exporter, scheduler,
           admin CRUD page). Imported and wired once.
PROJECT    minimal runnable skeleton — the foundation everything mounts onto.
```

Small snippets are NOT templated — the agent writes them inline (a 30-token helper
costs more to discover than to generate).

## The template rule (what stays agent-written)

**Templates are for NEW file creation ONLY.** `apply_template` is used when creating
files that do not exist AND the template map has a match. Never use a template to
modify an existing file — `edit_file` owns that. If no template matches, the agent
writes the file with `edit_file`, size irrelevant.

Always agent-written (never templated):
- Wiring/glue into existing code (router includes, mounting components into the app tree)
- Code that imports the project's own modules/services/models
- Bug fixes, refactors, one-off endpoints
- Small helpers and inline utilities
- Tests (the CI gate owns the deterministic suite; the agent writes tests per task)
- Resolving a template's TODO markers against the real project — that is the agent's
  adaptation work, not the template's

## Category map

| category  | level    | templates |
|-----------|----------|-----------|
| stack     | L5       | stack-webapp, stack-webapp-full, stack-admin, stack-chat, stack-landing |
| feature   | L3       | feature-crud, feature-auth, feature-rbac, feature-upload, feature-email, feature-errors, feature-config, feature-export, feature-import, feature-audit-log, feature-notifications, feature-sse, feature-rate-limit, feature-password-reset, feature-webhook |
| component | L4       | comp-csv-exporter, comp-queue-worker, comp-search-filter, comp-paginated-table, comp-charts, comp-form, comp-modal, comp-toast, comp-layout, comp-login, comp-http-client, comp-dropzone, comp-error-boundary, comp-crud-page, comp-dashboard, comp-settings, comp-chat, comp-page, comp-notification-bell, comp-markdown-viewer, comp-code-block, comp-landing, comp-signup |
| project   | L1       | project-fastapi, project-frontend |
| config    | —        | github-ci-python, github-ci-security-python, github-ci-security-generic, dockerfile-python, docker-compose, nginx |
| routine   | —        | gitignore-python, gitignore-node, requirements, pyproject, readme |
| spec      | —        | spec-feature |

## How agents should choose

1. Greenfield project → a `stack-*` template (or `project-*` + features)
2. Adding one feature → `feature-*`
3. Adding a subsystem/page → `comp-*`
4. Small helper → write it inline with `edit_file` — never template it
5. Config/deploy → `config`/`routine`
6. Before implementation → `spec-*` (standardized input, fewer retries)

## Conventions (curation rules)

- `description` must be ONE line — it is what `template_list` shows
- Every template NEEDS `sample_vars` — CI FAILS a template without it (no silent skips)
- Variable specs need `name`, `type` (`string|int|bool|list`) and `description`; required vars MUST appear in `sample_vars`
- Frontend templates use `[[ ]]` delimiters (`"delimiters": ["[[", "]]", "<%", "%>"]`) so
  they never collide with JS/HTML template syntax
- Stacks declare required vars once; nested templates inherit them, and use-level
  overrides may reference parent vars: `"variables": {"package_dir": "{{ project_name }}"}`
  (parent vars are DEFAULTED BEFORE uses compose — overrides can rely on defaults)
- `apply_template` is cycle-safe, dedupes repeated composition, renders EVERYTHING
  before writing anything (no partial applies), and skips existing files by default
  (`overwrite=true` to replace; per-file `"if_exists": "skip"|"overwrite"|"error"` in metadata)
- Rendering is sandboxed (SandboxedEnvironment) — templates from the hub are untrusted input
- New templates: write `.j2` files into `templates/<level>/<name>/files/`, then call
  `template_new(name, level, description, sample_vars, ...)` — it writes template.json
  and validates the render immediately

## Tooling

| tool            | purpose |
|-----------------|---------|
| `apply_template`| render a template (or whole stack) into the project (sandboxed, two-phase, skip-by-default, dry_run/overwrite flags) |
| `template_list` | no arg = catalog · `query=` ranked search (regex / words / code symbols / semantic) · `template="<name>"` = variables + files + example + notes for one template |
| `template_new`  | author a new template (metadata + immediate render validation) |

## The Gate (security + performance + bug detection)

Installed by `github_apply_gate` / `github_init_project` (with user confirmation),
non-destructively: existing project files are never overwritten.

**Profiles** (auto-detected): `package.json`→node · `composer.json`→php ·
`go.mod`→go · pyproject/requirements→python · none→generic (a repo is never ungated).

| Job | python | node (incl. Next) | go | php | generic |
|---|---|---|---|---|---|
| secrets (gitleaks) | ✅ | ✅ | ✅ | ✅ | ✅ |
| sca | pip-audit | npm audit | govulncheck | composer audit | osv-scanner |
| sast | Bandit | — | staticcheck | semgrep+CodeQL | — |
| quality (lint+format+type) | Ruff S+B+format+mypy | ESLint+prettier+tsc | gofmt+go vet | php -l+PHPStan | — |
| coverage threshold | ✅ | c8 | go cover | PHPUnit clover | — |
| semgrep (p/security-audit) | ✅ | ✅ | ✅ | ✅ | ✅ |
| container (trivy, if Dockerfile) | ✅ | ✅ | — | ✅ | — |
| codeql | py | js/ts | go | php | multi-lang |
| zizmor (workflow self-audit) | ✅ | ✅ | ✅ | ✅ | ✅ |
| perf-smoke | ✅ | ✅ main-import | — | — | — |
| bundle budget | — | ✅ | — | — | — |
| perf-load (k6, dispatch-only) | ✅ | ✅ | — | — | — |

Every job is SHA-pinned and graceful-skipping: a fresh repo stays green, a real
project gets real enforcement. Coverage applies only when a real package exists.
Perf budgets are universal defaults — tune after first green.

CI (`scripts/validate_templates.py`) re-renders every template with its sample vars on
every push — a broken template fails the build in seconds.
