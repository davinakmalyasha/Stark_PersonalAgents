# 11 — Migration Plan

Phased, contract-first, side-by-side. Each phase has a demoable acceptance
criterion and a rollback point.

---

## 1. Strategy

- **Port the contract, not the code.**
- Rust runs on `:8124` while Python stays on `:8123`.
- The Vite dev proxy selects the backend with `STARK_PORT`.
- Every phase adds tests; no phase removes the Python fallback.
- Cutover is a config flip after parity, not a big-bang rewrite.

**Honest effort estimate** (one developer, first Rust project):
- Usable coding-agent slice: **3–5 weeks**
- Full feature parity: **6–9 months**
- Voice/Telegram as Rust: additional **2–4 months** (not required for cutover)

---

## 2. Phases

### Phase 0 — Setup & snapshot (1–2 days)

Tasks:
- Install Rust/MSVC; verify `rustc`, `cargo`.
- Clone Codex read-only to `D:\ref\codex`.
- Run Python tests; commit current work; push; tag `pre-rust-rewrite`.
- Create `rust-rewrite` branch.
- Create `backend-rs/` workspace with `stark-contract` only.
- Add `scripts/export_tool_specs.py` + `specs/tool-specs.json`.
- Add `rust-toolchain.toml`, `.gitignore` entries, CI skeleton.

Acceptance:
- `cargo test` passes (empty contract tests).
- `tool-specs.json` matches Python.
- Branch pushed; Python app still runs.

Rollback: delete branch; Python untouched.

---

### Phase 1 — Vertical slice (1–2 weeks)

Tasks:
- `stark-contract`: `AppState`, `Delta`, `ServerEvent`, `ToolSpec`.
- `stark-providers`: OpenAI-compatible `reqwest` + `eventsource-stream`.
- `stark-core`: loop, cancellation, deltas.
- `stark-tools`: `read_file`, `edit_file` A/B, `search_code`,
  `run_command` (foreground), `ask_user`.
- `stark-store`: chat/messages + JSONL + active run.
- `stark-server`: `/api/run`, `/api/stream/{chat}`, `/api/abort`,
  `/api/health`, `/api/state`, `/api/history`.
- Point the UI at `:8124`.

Acceptance:
- A real coding task completes from the existing UI.
- SSE `delta`/`state`/`done` match Python shapes.
- Abort works; history survives restart.
- 5 tool fixtures pass.

Rollback: point UI back to `:8123`.

---

### Phase 2 — Tools & policy (3–4 weeks)

Tasks:
- Full registry from `tool-specs.json`; scope resolution.
- `edit_file` Mode C + verify + snapshots.
- `run_command` background/process manager/readiness/cleanup.
- `search_code` annotation/function snippets/offload.
- Policy: fs permission, command policy, envelope, approvals.
- Output caps, redaction, `/scratch` offload.
- MCP client (`rmcp`) + per-tool approval.
- Approval broker + `ask_user` round-trip.

Acceptance:
- All Phase-2 tools pass parity fixtures.
- Dangerous commands blocked; approval flow works.
- `run_command` background lifecycle matches.
- Snapshots restore exact bytes.

Rollback: disable new tools via scope; Phase 1 still works.

---

### Phase 3 — Agents & sessions (3–4 weeks)

Tasks:
- `AgentRegistry` (workspace agents + specialist JSON).
- Developer/General/House/DFD direct runs.
- `RunCtx` + `WorkspaceDeps` equivalent.
- Session JSONL read/write/derive; child sessions.
- Compaction (summarizer, retained tail, token budget).
- Model registry, provider modes, fallback pool.
- Token/cost accounting.

Acceptance:
- Developer and General complete real tasks.
- Python and Rust session files interoperate.
- Compaction keeps tool pairs intact.
- Provider fallback works on mock 429/500.

Rollback: route those agents back to Python.

---

### Phase 4 — Orchestration (3–4 weeks)

Tasks:
- Router parity.
- Durable graph with checkpoints/park/resume.
- Delegation jobs (fresh/resume), parallel fan-out, reduction.
- Status/steer/cancel.
- Specialists + `call_subagent`.
- Plan mode + plan dock.
- Schedules/wakes.

Acceptance:
- Boss delegates two jobs in parallel and reports one result.
- Crash at each step resumes.
- Steering/cancel work.
- Plan mode hides write tools.

Rollback: orchestrator stays Python; direct agents can be Rust.

---

### Phase 5 — Group projects (3–4 weeks)

Tasks:
- Event store/reducer/rebuild.
- Supervisor tick phases.
- Materialization + ops.
- Leases, budgets, deadlines, stalls.
- Criteria (`check`, `manual`, `llm`, `budget`).
- Approvals + idempotency.
- Group tools + board/events endpoints.

Acceptance:
- One real project completes on a real model.
- Existing Python project opens and replays.
- Kill/restart at each phase resumes.
- External write dedupe works.

Rollback: group projects stay Python; Rust handles non-group work.

---

### Phase 6 — Memory & remaining tools (3–4 weeks)

Tasks:
- Repo map (tree-sitter), summaries, notes/meta.
- `read_pdf`, `create_docx`, `create_xlsx`.
- `browser_*` (subprocess), `github_apply_gate`.
- House, DFD, screen/vision, `ast_search`.
- Optional embeddings/vector store.

Acceptance:
- Remaining tool fixtures pass.
- Repo map quality is comparable.
- House/DFD flows work.

---

### Phase 7 — Full server parity (3–4 weeks)

Tasks:
- All routes (workspaces, chats, snapshots, notes, canvas, admin).
- SSE reattach + run tracker.
- Voice proxy (HTTP + WS).
- Telegram sidecar compatibility.
- Pairing/auth, CORS, static serving.
- Single-instance + graceful shutdown.

Acceptance:
- Existing UI, Telegram, and voice work unchanged.
- Route inventory diff is empty.
- Reattach/abort/steer under load.

---

### Phase 8 — Cutover (1–2 weeks)

Tasks:
- Shadow-run both backends on the same fixtures.
- Flip the Vite proxy/Electron startup to Rust.
- Keep Python voice/Telegram sidecars.
- Monitor errors/latency.
- Remove Python backend startup; archive the tree.

Acceptance:
- One week with no P0/P1 regressions.
- Rollback tested (flip port back).
- Docs updated.

---

## 3. Work breakdown by subsystem

| Subsystem | Phase | Key risk |
|---|---|---|
| Tool specs/descriptions | 0 | Drift; mitigated by export/diff |
| Agent loop | 1 | Tool-call streaming edge cases |
| Patch/edit semantics | 2 | CRLF, drift, multi-hunk |
| Process manager | 2 | Windows backgrounding/cleanup |
| Policy | 2 | False positives/bypasses |
| Sessions/compaction | 3 | Tool-pair splitting |
| Provider fallback | 3 | Rate limits/retry storms |
| Graph resume | 4 | Idempotent step replay |
| Delegation | 4 | Parallel child lifecycle |
| Group reducer | 5 | One wrong rule corrupts state |
| Supervisor | 5 | Single-writer/crash recovery |
| Voice proxy | 7 | Binary frames/close codes |
| Cutover | 8 | Hidden UI assumptions |

---

## 4. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Tool contract drift | Export specs from Python; CI byte diff |
| Silent behavior change | Parity fixtures per tool/route; no "cleanup" while porting |
| Windows process semantics | Port `_procman` with tests; use process groups; kill-tree |
| Patch engine bugs | Port test corpus; transactional apply; snapshot fallback |
| Group event corruption | One reducer; rebuild tests; never edit events |
| Compaction context loss | Never split tool pairs; retained tail; regression prompts |
| Provider quirks | Wiremock cases for reasoning/tool deltas/usage |
| Rust learning curve | Start with 8 crates; one vertical slice; no macros early |
| Scope creep | Phase gates; no UI redesign; no new features |
| Two runtimes | Feature flag + port switch; shared DBs; contract tests |
| Codex copy trap | Reference only; no fork; Apache-2.0 attribution if copying |

---

## 5. Accepted deviations

Record every intentional difference here. Initial list:

| Deviation | Reason | Approval |
|---|---|---|
| No Bazel | Cargo is sufficient at this scale | Required |
| No TUI at first | React/Electron is the UI | Required |
| Python voice/Telegram sidecars | Model parity cost; API is stable | Required |
| `tower-http` for CORS/static | Codex uses raw axum/tiny_http; this is standard | Required |
| `pdf-extract`/`docx-rs`/`rust_xlsxwriter` | Domain tools not in Codex | Required |
| `xcap`/Windows capture | Screen tools not in Codex | Required |
| Rust reads/writes existing DBs | Avoids migration; forces parity | Locked |
| No `rig`/`swiftide`/`async-openai` | Hand-rolled, Codex-style | Locked |

---

## 6. Definition of done

- [ ] 42 tool specs byte-identical; all fixtures pass.
- [ ] ~90 routes implemented; contract tests pass.
- [ ] Existing chats/sessions/group projects open and mutate.
- [ ] Frontend works with no UI changes.
- [ ] Telegram and voice work through the sidecars.
- [ ] Abort/steer/reattach/crash-resume verified.
- [ ] Group project completes end-to-end.
- [ ] `cargo fmt`, `clippy -D warnings`, `cargo test` green.
- [ ] Python suite still green (until removal).
- [ ] Rollback tested.
- [ ] Docs updated; deviations recorded.

---

## 7. First 10 days

| Day | Work |
|---|---|
| 1 | Install Rust/MSVC; verify; clone Codex; run Python tests |
| 2 | Commit/push/tag/branch; create `backend-rs` workspace |
| 3 | `stark-contract` types; export `tool-specs.json`; CI skeleton |
| 4 | `stark-providers` SSE client + mock server |
| 5 | `stark-core` loop + deltas + cancellation |
| 6 | `stark-store` chats/messages/JSONL |
| 7 | 5 tools + dispatcher skeleton |
| 8 | `/api/run` + SSE + abort |
| 9 | Point UI at `:8124`; end-to-end smoke |
| 10 | Fixtures for 5 tools; write Phase-1 report |
