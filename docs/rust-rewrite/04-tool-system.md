# 04 — Tool System

The tool layer is the largest compatibility surface in Stark: **42 definitions
in `TOOL_DEFINITIONS`**, each with a name, description, JSON Schema, scope,
caps, and a text result contract. The model sees these strings; changing them
changes behavior.

`propose_plan` exists in `tools/MainTool/proposePlan.py` and is allowed by the
plan-mode guard, but it is **not registered** in `TOOL_DEFINITIONS` or the
runtime `_SCOPE_MAP`; the native schema never exposes it. Phase 0 freezes the 42
registered tools and records this gap rather than changing Python behavior.

This document defines the Rust implementation and the exact port order.

---

## 1. Sources of truth

| Artifact | Source |
|---|---|
| Tool definitions | `tools/_schemas.py` (`TOOL_DEFINITIONS`) |
| Per-tool docs | `docs/tools/*.md` |
| Scope ACL | `docs/tools/_SCOPE_MAP.md` + `tools/_schemas.py` |
| Runtime metadata | `tools/_registry.py` (`ToolExecutionResult`) |
| Policy | `tools/MainTool/_cmdpolicy.py`, `tools/_runtime.py` |
| Approvals | `tools/_runtime.py`, `llm/native_tools.py` |
| Snapshots | `tools/checkpoints.py` |
| MCP | `tools/mcp/mcp_bridge.py` |

**Rule:** descriptions and parameter schemas are copied **verbatim**. Do not
"improve" wording while porting. If a description must change, it is a product
change with a before/after diff and a prompt regression run.

---

## 2. Spec export (do this first)

Add `scripts/export_tool_specs.py` in the Python tree:

- Imports `tools._schemas`.
- Writes `backend-rs/specs/tool-specs.json`:

```json
{
  "version": 1,
  "tools": [
    {
      "name": "read_file",
      "description": "Read one file, ...",
      "parameters": { "type": "object", "properties": {}, "required": [] },
      "scope": ["developer", "general", ...],
      "parallel_safe": true,
      "max_output": 12000,
      "approval": "none"
    }
  ]
}
```

- The exporter never imports agent loops or starts the server.
- CI runs it with `--check` and fails if the Rust-embedded copy differs.

Rust embeds the JSON:

```rust
pub const TOOL_SPECS_JSON: &str = include_str!("../../../specs/tool-specs.json");
```

This guarantees the prompt contract survives the rewrite even if Rust structs
are named differently.

---

## 3. Core types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub scope: Vec<Scope>,
    pub parallel_safe: bool,
    pub max_output: usize,
    pub approval: ApprovalHint,
}

pub struct ToolCtx {
    pub run: RunId,
    pub chat: ChatId,
    pub workspace: PathBuf,
    pub role: Role,
    pub envelope: Option<Envelope>,
    pub allowed_tools: Option<HashSet<String>>,
    pub caps: HashMap<String, usize>,
    pub cancel: CancellationToken,
    pub approvals: Arc<ApprovalBroker>,
    pub events: broadcast::Sender<ServerEvent>,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn spec(&self) -> &'static ToolSpec;
    async fn call(&self, ctx: &ToolCtx, args: serde_json::Value) -> ToolResult;
}

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub offloaded: bool,
}
```

`ToolResult::from_content` mirrors `tools/_registry.py`:
- `[OK] Exit N` / `[ERR] Exit N`
- `[TIMEOUT]` / "timed out"
- "offloaded" / `/scratch/`
- explicit error prefixes.

---

## 4. Registry and scope resolution

```rust
pub struct ToolRegistry {
    by_name: HashMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn for_scope(&self, scope: Scope, extra: Option<&HashSet<String>>) -> Vec<Arc<dyn Tool>> {
        // 1. scope ACL from specs
        // 2. intersect with per-run allowlist (never widen)
        // 3. hide plan-mode-vetoed tools
    }
}
```

Rules:
- A tool is defined **once**; scope assignment is data.
- A project can only **narrow** the base scope, never widen it.
- `never` means the tool is absent from the toolset (no attempt, no event).
- `blocked` means the tool is present but policy refuses the action (visible).
- Unknown tool names are rejected; no dynamic dispatch by string from the model
  without a registry hit.

---

## 5. Dispatch pipeline

Every tool call goes through one function:

```
validate args against schema
  → classify(action, envelope, grants)
      ├─ Never     → reject "tool not available"
      ├─ Blocked   → [BLOCKED] reason
      ├─ NeedsApproval → ApprovalBroker.request(...) → await
      └─ Autonomous
  → execute with timeout
  → catch panic → [ERR] tool panicked
  → redact secrets
  → cap output (per-tool + per-agent) → offload to /scratch if large
  → build ToolResult
  → emit tool_finished delta
```

This replaces pydantic-ai's `FilteredToolset` + hooks. The order matters:
policy before execution, redaction before the model sees anything.

---

## 6. Policy

### 6.1 Filesystem permission

Port `tools/_runtime.py:enforce_fs_permission`:

- Resolve the path; reject symlink escapes.
- Workspace root is the boundary; project envelope can narrow it.
- Reads outside the workspace may be allowed by scope; writes outside are
  `never`/approval depending on role.
- `real_path` must start with the resolved workspace root (case-insensitive on
  Windows).

### 6.2 Command policy

Port `tools/MainTool/_cmdpolicy.py` semantics:

- Split on unquoted `;`, `&&`, `||`, `|`, `&`, newline.
- Tokenize with shell-like quoting; tolerate parse failure by falling back to
  conservative substring denial (never allow on parse failure).
- Handle wrappers (`env`, `nohup`, `time`, `command`, `exec`, `sudo`, `nice`)
  and subshells (`bash`, `sh`, `powershell`, `cmd`).
- Global deny: fork bomb, `dd if=`, `mkfs`.
- Subshell deny: destructive patterns (`rm -rf /`, `format c:`, registry
  deletes, shutdown, diskpart, …).
- Credential-bearing programs and secret paths.
- Plan mode: allow only proven read-only programs; git read subcommands only.
- Negative exit codes for `grep`/`find`/`diff` are "no match/differences", not
  errors.

Use `regex` + a tokenizer. Do **not** reimplement with naive substring checks;
the Python version exists because naive checks produced both false positives
and bypasses.

### 6.3 Envelope

Group-project tiers (`agents/group/policy.py`):

```rust
pub enum Decision { Autonomous, NeedsApproval, Blocked, Never }

pub struct Action {
    pub kind: ActionKind, // Fs, Command, Mcp
    pub target: String,
    pub write: bool,
    pub outside: bool,
    pub dangerous: bool,
}

pub fn classify(action: &Action, envelope: Option<&Envelope>, grants: &Grants) -> Decision;
```

Order:
1. Inside the project folder → free zone (except dangerous).
2. Dangerous outside → `Blocked` (credentials, money, destructive).
3. `never` paths/tools → not in toolset (handled earlier).
4. Envelope tier.
5. Default → `NeedsApproval`.

Keep `classify` pure (no I/O, no clock). Test it exhaustively.

### 6.4 Idempotency

External writes are deduped by `"{item_id}:{shape}:{payload_sha}"`.
States: `fresh → reserved → done | skipped`; a reserved key with no done is
`unknown` and asks a human. The ledger is derived from events
(`write.reserved/done/skipped`), not a table.

---

## 7. Approvals and `ask_user`

```rust
pub struct ApprovalBroker {
    pending: DashMap<ApprovalId, PendingApproval>,
    waiters: DashMap<ApprovalId, oneshot::Sender<Decision>>,
}

pub struct PendingApproval {
    pub id: ApprovalId,
    pub chat_id: ChatId,
    pub tool: String,
    pub args: serde_json::Value,
    pub reason: String,
    pub created_at: DateTime<Utc>,
}
```

- `NeedsApproval` creates a pending record, emits an SSE/state event, and
  awaits a `oneshot`.
- `/api/ask/pending` lists pending batches; `/api/ask/answer` resolves them.
- `ask_user` uses the same broker with a question batch payload.
- Timeout (default 600s) → reject with a clear message.
- Denial is a `[BLOCKED]` tool result, not a crash.

Plan-approval flow: `ask_user` questions may carry
`intent: { kind: "plan-review", approve: "<label>" }`. Preserve this shape.

---

## 8. Output handling

- Per-tool cap from the spec; per-agent override from `WorkspaceDeps.tool_caps`.
- If output exceeds the cap, write the full text to `/scratch/` and return a
  stub with the path (current behavior).
- Redact secrets (tokens, keys, `.env` values) before returning.
- Preserve the `[OK]/[ERR]` first line; truncation must not hide it.
- Tool metadata (`is_error`, `exit_code`, `timed_out`, `offloaded`) is derived
  at the boundary; loops must not infer from prose alone.

---

## 9. Snapshots and rewind

Port `tools/checkpoints.py`:

```
<workspace>/.deep_agents/snapshots/<chat_id>/<run_id>/<path>.snap
<workspace>/.deep_agents/snapshots/<chat_id>/_index.json
<workspace>/.deep_agents/snapshots/<chat_id>/runs/<run_id>.json
```

- Snapshot bytes before a write; gzip if > 16 KB.
- Best-effort: a failed snapshot must not block the write.
- LRU cap (default 100 runs per chat).
- `begin_run` allocates `run-<ts>-<hex>`; the run manifest lists files.
- Restore by run or by snapshot ID.

---

## 10. Per-tool port notes

The description/schema always comes from `tool-specs.json`. This table covers
implementation notes only.

### 10.1 Core

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `read_file` | `stark-tools::fs::read_file` | `MainTool/readFile.py` | Batch `file_paths`; paging; cached-stub for unchanged full re-reads; `[FILE]` sections; image/PDF handling where applicable |
| `edit_file` | `stark-tools::patch` | `MainTool/editFile.py` | Mode A exact replace; Mode B whole file; Mode C patch headers + `@@` hunks; transactional apply; `verify=py_compile|tsc|shell`; snapshot before write |
| `run_command` | `stark-tools::proc` | `MainTool/runCommand.py` | Git Bash; timeout/idle timeout; auto-background server-like commands; process actions list/logs/kill/wait/ready/send/kill_all; `expect` regex; owner cleanup |
| `search_code` | `stark-tools::search` | `MainTool/searchCode.py` | `ignore` + `globset` + `regex`; lines/files/count; symbol annotation via tree-sitter; multi-pattern ranking; function snippets; `/scratch` overflow |
| `ast_search` | `stark-tools::ast` | `MainTool/astSearch.py` | Python-only structural rules; use `tree-sitter-python` or `rustpython-parser`; keep exact result format; port last |

### 10.2 Documents

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `read_pdf` | `stark-tools::docs::pdf` | `DocTool/doc_tools.py` | Page ranges; cap pages (default 20); text only |
| `create_docx` | `stark-tools::docs::docx` | same | Title heading + paragraph per line |
| `create_xlsx` | `stark-tools::docs::xlsx` | same | Headers + rows; cell types |

### 10.3 Interaction

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `ask_user` | `stark-tools::ask` | `MainTool/planTools.py` | Question batch schema; 2–4 options; `multiSelect`; plan-review intent; timeout |
| `update_plan` | `stark-tools::plan` | `MainTool/planTools.py` | Full replacement; ≤1 `in_progress`; persist per scope |
| `propose_plan` | `stark-tools::plan` | `MainTool/proposePlan.py` | Plan-review gate; **not in `TOOL_DEFINITIONS` or `_SCOPE_MAP`** — unregistered in Python; not part of the 42-tool Phase 0 contract |
| `files_changed` | `stark-tools::watch` | `tools/watcher.py` | Recent external edits; advisory |

### 10.4 Browser / screen

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `browser_act` | `stark-tools::browser` | `AddOnTool/browserTools.py` | Shell out to `agent-browser` (Rust binary); pass args/timeouts; parse JSON/text |
| `browser_navigate` | same | same | Snapshot with `@ref` selectors |
| `browser_extract` | same | same | Text/HTML/attr/title/URL |
| `browser_screenshot` | same | same | Temp file path |
| `browser_close` | same | same | Free resources |
| `look_at_screen` | `stark-tools::screen` | `AddOnTool/screenTools.py` | Capture via `xcap`/Windows APIs; attach image |
| `set_screen_share` | same | same | on/off/continuous; update state |

### 10.5 Delegation

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `delegate` | `stark-agents::delegate` | `AddOnTool/delegationTools.py` | Fresh vs resume jobs; parallel child chats; result reduction |
| `delegation_status` | same | same | Live statuses |
| `job_message` | same | same | Steer a running job |
| `job_cancel` | same | same | Cooperative abort |
| `call_subagent` | `stark-agents::subagent` | `AddOnTool/subagentTools.py` | run/spawn/status; specialist profile lookup |

### 10.6 Projects / groups

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `create_coding_project` | `stark-tools::projects` | `AddOnTool/workspaceTools.py` | Folder + workspace + Dev agent |
| `create_group` | `stark-groups::tools` | `AddOnTool/groupTools.py` | Materialize folder + manifest |
| `group_board` | same | same | Read-only projection |
| `group_item_detail` | same | same | Read-only |
| `group_events` | same | same | Event log |
| `group_update` | same | same | One op per call |
| `schedule` | `stark-tools::schedule` | `AddOnTool/scheduleTools.py` | One-off/recurring wake |
| `give_note` | `stark-tools::notes` | `AddOnTool/noteTools.py` | Chat/project sticky note |
| `canvas_mode` | `stark-tools::canvas` | `AddOnTool/canvasTools.py` | Watch chats; wake orchestrator |

### 10.7 House

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `house_get_state` | `stark-tools::house` | `HouseTool/house_tools.py` | Rooms/devices/scenes/energy/security |
| `house_get_device` | same | same | State + capabilities |
| `house_set_device` | same | same | Type-specific state keys |
| `house_run_scene` | same | same | Apply scene actions |
| `house_get_history` | same | same | Hourly kWh series |

### 10.8 Business / GitHub / memory

| Tool | Rust module | Source | Notes |
|---|---|---|---|
| `dfd_bridge` | `stark-tools::dfd` | `AddOnTool/dfdBridge.py` | HTTP client; non-money ops only |
| `create_client` | same | `AddOnTool/workspaceTools.py` | Scaffold; requires `approved_by` |
| `github_apply_gate` | `stark-tools::github` | `AddOnTool/githubTools.py` | `gh` + `git` subprocess; branch ladder; first run |
| `update_feature` | `stark-memory::features` | `tools/…` | `features.md` |
| `update_project_meta` | `stark-memory::meta` | `tools/…` | Allowed keys only |

---

## 11. Port order

1. **Phase 1:** `read_file`, `edit_file` (A/B), `search_code`, `run_command`
   (foreground), `ask_user`.
2. **Phase 2:** `edit_file` C + verify, `run_command` background/process
   manager, `search_code` annotations, `update_plan`, `propose_plan`,
   `files_changed`, MCP wrapper.
3. **Phase 3:** `read_pdf`, `create_docx`, `create_xlsx`, `browser_*`,
   `github_apply_gate`.
4. **Phase 4:** `delegate`, `delegation_status`, `job_message`, `job_cancel`,
   `call_subagent`.
5. **Phase 5:** `create_group`, `group_*`, `schedule`, `give_note`,
   `canvas_mode`, `create_coding_project`.
6. **Phase 6:** `house_*`, `dfd_bridge`, `create_client`, `look_at_screen`,
   `set_screen_share`, `update_feature`, `update_project_meta`.
7. **Phase 7:** `ast_search` (last; Python-specific semantics).

---

## 12. Parity tests

For each tool, keep a fixture directory:

```
tests/parity/tools/<tool>/
  case-01.args.json
  case-01.expected.json
  case-02.args.json
  ...
```

Run the same case through Python (`execute_tool_detailed`) and Rust, normalize:

- workspace path → `<WS>`
- timestamps/durations → `<TS>`/`<MS>`
- absolute temp paths → `<TMP>`
- truncation hints → stable markers

Compare:
- exit status / `is_error`
- first line (`[OK]`/`[ERR]`)
- normalized body
- offload behavior

Add `insta` snapshots for the tool spec JSON and for representative outputs.

---

## 13. Acceptance for the tool layer

- `tool-specs.json` diff is empty in CI.
- Every scoped agent sees exactly the Python tool list (golden test from
  `tests/fixtures/agent_registry_golden.json`).
- All 42 descriptions byte-identical.
- Policy fixtures pass: allowed/blocked/approval decisions.
- `run_command` background lifecycle matches (start, logs, wait, send, kill,
  cleanup).
- `edit_file` patch cases match (add/update/delete/move, CRLF, drift).
- Approval flow round-trips through the existing UI endpoints.
- Snapshots restore exact bytes.
