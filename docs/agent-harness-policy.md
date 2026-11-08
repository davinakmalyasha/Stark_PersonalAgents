# Agent harness policy

This is the runtime contract for the Orchestrator, workspace agents, tools, and
sandbox. It matches the Codex-style split: reads may cross workspace boundaries,
writes stay in a workspace, and every advertised tool is gated at dispatch.

## Orchestrator: read-only manager

The Orchestrator decides whose job a request is. It answers questions itself
with read-only inspection, and delegates implementation.

Allowed without a write:

- `bash` only for commands the Codex-style read-only classifier accepts
  (`ls`, `cat`, `rg`, `git status`, `git diff`, ...). Background jobs, process
  actions, redirection, and mutating commands are blocked. This is the
  Orchestrator's only file inspection tool.
- `memory` (read actions only) and `ask_user_question`.
- `workspace` `list` / `info`; `create`, `clone`, and `remove` are blocked in
  read-only mode.
- `delegate`, `delegation_status`, `job_message`, `job_cancel`, groups,
  schedules, notes, and canvas for coordination.

There is no `read`, `glob`, `grep`, `browser_act`, `web_fetch`, `web_search`,
`skill`, `tool_search`, `update_plan`, or `files_changed` in this scope.

Delegation stays in one family: `delegate` + `delegation_status` +
`job_message` / `job_cancel`. A delegated workspace agent runs in the target
workspace and is not limited by the Orchestrator's read-only sandbox. The
Orchestrator prompt says this explicitly so it does not give up when a path is
outside its own workspace.

## General: personal operator

General handles everyday life: research, writing, admin, personal records,
signed-in browser tasks, and desktop actions. It does not build software
(Developer) and does not control devices (House).

Routing:

- Public web → `web_search`, `web_fetch`, `browser_act` (isolated browser, no
  logins).
- Signed-in web (Gmail, certificates, portals) → the `browse` MCP server,
  exposed to `general` only. It drives the user's real browser and tabs. A
  named session plus `allowedOrigins` limits the sites it can touch. Logins,
  2FA, captchas, and payments are handed back to the user through browse's
  `handoff()` and resume only after the user clicks Continue.
- Desktop apps → the `agent-desktop` MCP server (Vercel Labs, Apache-2.0,
  native Rust core over Windows UI Automation), exposed to `general` only.
  `desktop_snapshot` first; act on refs (`@snapshot:ref`), then snapshot again
  to verify. Refs are snapshot-scoped: `STALE_REF` or `AMBIGUOUS_TARGET` means
  take a new snapshot. Only ref-based UI tools are enabled; clipboard, traces,
  notifications, pixel mouse, and key/mouse down/up stay off. Files and
  processes stay with `bash`.
- Personal records → `personal_os` (see [personal-os.md](./personal-os.md)).
  General gets the full surface; House gets read-only household context and
  never money.
- Confirmation: `ask_user_question` before sending email/messages, submitting
  forms, applications, certificate exams, payments, purchases, deletions, or
  typing into a desktop app. Show the exact recipient/subject/body or amount
  and payee first.
- Money is bookkeeping only. No transfers, no card or bank credentials, and no
  claim that a payment happened.
- Secrets: never written to memory, files, plans, notes, or tool arguments.
  2FA and logins stay with the user.

MCP scope is enforced by `stark-tools::mcp` (`scopes` on each server), not just
by the prompt. A server marked `["general"]` is invisible to every other
scope, and read-only agents cannot call MCP tools at all.

The versioned example config is
`backend-rs/specs/mcp-general.example.json`; the live copy is
`.deep_agents/mcp_servers.json`. Browse is installed with
`npx @opencode/browse install` (extension + `browse mcp`). Desktop control is
`npm install -g agent-desktop` followed by `agent-desktop mcp`; the package
ships its native Rust binary, and no Python or .NET runtime is involved.

## Tool surface

Legacy names (`read_file`, `edit_file`, `search_code`, `run_command`,
`call_subagent`, `ask_user`, `todo_write`) and the removed `write` tool are
absent from the contract and every scope. A dispatch denylist remains for old
transcripts. The v2 surface is:

- files: `read`, `edit`, `apply_patch`, `glob`, `grep`
- shell: `bash`, `job`
- plan: `update_plan` (one plan store, full replacement or `ops`)
- ask: `ask_user_question`
- agents: `spawn_agent`, `message_agent`, `wait_agent`, `interrupt_agent`,
  `close_agent`
- research: `web_fetch`, `web_search`, `browser_act`
- memory: `memory`
- personal: `personal_os` (money/todos/food/training)
- MCP discovery: `tool_search` (General only; Developer delegates MCP work)

## Sandbox and approvals

- **Reads** may cross the workspace boundary for verifiably read-only shell
  commands and for the read tools.
- **Writes** use `apply_patch`/`edit` and resolve inside the workspace; `../` and symlink escapes are
  rejected with a canonical-path check.
- **External writes** go through `permissions.json` `external_directory` and
  require approval unless `STARK_AUTO_APPROVE` is set.
- `STARK_APPROVAL=never` turns every `ask` into a block (no hidden prompt).
- Session approvals are exact command token sequences. A model-supplied
  `prefix_rule` cannot widen a grant.
- Read-only agents never write scratch files, never run background jobs, and
  never manage processes.
- Code mode blocks background shell, job control, and subagent tools by
  capability, not just by name.

On Windows the OS backend is still reported honestly as not enforced; the
dispatch policy and approval gates are the enforcement layer today.

## Status and polling

- Delegation results arrive as `[DELEGATION RESULT]` events.
- `delegation_status` is a one-per-turn snapshot; it is not a polling loop.
- Group progress is pushed as `[GROUP RESULT]`; `group_board` and `group_events`
  are also one snapshot per turn.
- A second snapshot in the same turn returns `[STATUS ALREADY CHECKED]` and
  tells the model to wait for the next event or answer with the last state.

## Read-only shell and parallelism

The shell classifier follows Codex's `is_known_safe_command` policy: a fixed
read-only allowlist, safe `find`/`rg`/`base64`/`sed`/`git` options, rejection of
unsafe git globals and path-qualified binaries, and redirection/substitution
rejection. Only commands that pass this classifier join a parallel batch;
mutating shell commands stay sequential.

## Browser budget

`browser_act` is a stateless CLI wrapper over Vercel Labs `agent-browser`
(Apache-2.0, native Rust + CDP). The CLI is installed globally with
`npm install -g agent-browser@latest`; its versioned skill (`agent-browser
skills get core`) always matches the installed binary, and Chrome is refreshed
with `agent-browser install`.

Actions: `navigate`, `click`, `fill`, `type`, `press`, `extract`,
`screenshot`, `close` use the CLI directly. `read`, `a11y`, `record`, and
`network` go through `agent-browser mcp call <tool> '<json>'`, which runs one
MCP tool without a persistent stdio session and returns JSON — a good fit for
one short process per action. Read-only agents get `read`, `a11y`, and
`network_action=requests|request`; `record`, `network` routes/HAR, and the
interaction actions stay write-gated.

Keep it at the stateless budget on small machines:

- 1 GB / 1 CPU: stateless `agent-browser`, one short-lived headless Chromium per
  action; prefer `web_fetch` for static pages and `extract` over screenshots.
- 2 GB: the same contract; optionally reuse one tab, but do not run a resident
  Playwright MCP server alongside the model.
- Cloud: same `browser_act` contract; run the browser in the cloud worker and
  keep the local machine at the stateless budget.

Screenshots from read-only agents go to
`<harness>/.deep_agents/scratch/`, never into a workspace. Navigations are
checked against the same public-URL guard as `web_fetch`.

## Error and resume contract

- Missing/invalid arguments produce a typed, actionable error.
- A timed-out foreground command stays resumable (`session #N`), with
  `logs` / `wait` / `kill` instructions.
- Repeated tool errors trigger a retry-guidance message before any hard stop.
- One failed delegation item does not cancel the rest of the fan-out.
- A crash-interrupted tool call is preserved with a synthetic retry result on
  replay; compaction keeps its retained tail.
- Assistant text is stored once per turn, never duplicated in live history.

## Trust checks

`cargo test --workspace` covers the invariants:

- every exposed tool has an inventory entry and dispatch arm;
- no scope advertises a legacy tool;
- read-only Orchestrator keeps inspection tools and loses write tools;
- read-only cross-workspace reads work and writable agents cannot use the
  selector;
- patch application is all-or-nothing and `Add File` fails on an existing file;
- approvals are exact-command and environment dumps are denied.
