# 10 — Parity Testing

The rewrite is done when the Rust backend behaves like the Python backend, not
when it compiles. This document defines how that is proven.

---

## 1. Principles

1. **Black-box first.** Compare HTTP/SSE outputs and on-disk state, not
   internal function names.
2. **Deterministic fixtures.** Use a mock provider for tests; real models are
   for manual acceptance only.
3. **Normalize environment noise.** Paths, timestamps, durations, temp dirs,
   and model token counts are replaced with stable markers.
4. **Every ported tool has at least one fixture.** No exceptions.
5. **Every ported route has a contract test.** Method + path + status + shape.
6. **Both directions.** Rust reads Python artifacts; Python reads Rust
   artifacts (until cutover).

---

## 2. Test layers

| Layer | Command | Scope |
|---|---|---|
| Unit | `cargo test -p stark-<crate>` | Pure logic: policy, reducer, router, patch parser |
| Integration | `cargo test --workspace` | Store, server, agent loop with mock provider |
| Parity | `cargo test -p stark-parity` | Python vs Rust fixtures |
| Contract | `cargo test -p stark-server --test contract` | Routes/SSE shapes |
| Manual | `cargo run -p stark-server` + UI | Real model / voice / Telegram |
| Python gate | `python -m pytest tests/ -q` | Existing suite stays green |

---

## 3. Fixtures layout

```
backend-rs/tests/parity/
├── tools/
│   └── <tool>/
│       ├── case-01.args.json
│       ├── case-01.expected.json
│       └── case-02...
├── routing/
│   └── cases.json
├── specs/
│   └── tool-specs.expected.json
├── runs/
│   └── <name>/
│       ├── input.json
│       ├── provider.jsonl        # mocked SSE chunks
│       └── expected-events.jsonl
├── db/
│   ├── chats.fixture.db
│   └── group.fixture.db
└── sessions/
    └── *.jsonl
```

Keep fixtures small. Do not commit secrets, real user chats, or model keys.

---

## 4. Tool spec parity

`scripts/export_tool_specs.py` writes the canonical JSON.

Test:
1. Export from Python.
2. Compare with `backend-rs/specs/tool-specs.json` (embedded).
3. Fail on byte difference.

Also compare:
- tool names (set equality)
- descriptions (byte equality)
- parameter JSON (structural equality)
- scope sets
- caps/parallel-safe/approval hints

This single test protects the most fragile contract.

---

## 5. Tool behavior parity

Runner:

```text
for each case:
  python: tools._registry.execute_tool_detailed(name, args, deps)
  rust:   ToolDispatcher::call(name, args, ctx)
  normalize both
  assert is_error, first line, body, offloaded, exit_code
```

Normalization:
- workspace path → `<WS>`
- temp dir → `<TMP>`
- timestamps/durations → `<TS>`/`<MS>`
- line/column paths → relative
- JSON key order → sorted
- CRLF → LF for comparison only
- truncation markers → stable

Tool-specific cases:
| Tool | Must test |
|---|---|
| `read_file` | batch, paging, cache stub, binary/image, missing |
| `edit_file` | A/B/C, create, delete, move, CRLF, drift, verify fail |
| `run_command` | foreground, timeout, background start, logs, wait, ready, send, kill, cleanup |
| `search_code` | content/files/count, globs, ignore_case, fixed, context, annotate, overflow |
| `ast_search` | each rule, path escape rejection |
| `ask_user` | options, multiSelect, plan-review intent, timeout |
| `browser_*` | mocked `agent-browser` output |
| `read_pdf`/docx/xlsx | file exists, bytes, page ranges |
| `delegate`/`job_*` | fresh/resume, parallel, steer, cancel |
| `group_*` | board/detail/events/update op |
| `house_*` | get/set/scene/history |
| `dfd_bridge` | non-money ops only |
| `github_apply_gate` | dry-run/validation (mock `gh`) |

---

## 6. Routing parity

Fixture corpus of `(prompt, workspace_path, chat_id, workspace_kind)` →
`RouteDecision`.

Cases:
- house/personal/dfd/design hints
- creation phrasing variants
- existing project by path
- existing project by name mention
- coding workspace no hit
- empty workspace
- router error fallback

Compare `kind` and `reason` category (reason strings may be normalized).

---

## 7. Agent registry parity

Use `tests/fixtures/agent_registry_golden.json`.

Compare:
- agent keys/aliases/display
- tool scopes
- MCP scopes
- model pins per mode
- thinking/compaction/caps
- delegatable/history namespace

A new specialist JSON must appear without code changes.

---

## 8. SSE / run parity

Use a mock provider that emits a fixed sequence:

```jsonl
{"type":"text_delta","text":"Hel"}
{"type":"text_delta","text":"lo"}
{"type":"tool_call","id":"c1","name":"read_file","args":{"file_path":"a.txt"}}
{"type":"tool_result","id":"c1","content":"..."}
{"type":"text_delta","text":"Done"}
{"type":"done","usage":{...}}
```

Record both backends' SSE and compare:
- event names/order
- delta kinds/text
- tool_started/tool_finished presence (not exact ms)
- final `AppState` fields (normalize generated ids/paths)
- `done.status`

Also test:
- reattach after disconnect
- abort mid-stream
- steering between turns
- error propagation (`error` + `traceback` presence)

---

## 9. Database compatibility

| Test | Steps |
|---|---|
| Read Python chat DB | Copy fixture; Rust lists chats/messages/search; compare |
| Write then Python | Rust appends chat/message; Python reads; compare |
| Read Python group DB | Replay events; compare board JSON |
| Write then Python | Rust appends events/ops; Python rebuilds; compare |
| Session JSONL | Rust loads Python file, appends, Python loads |
| Snapshots | Restore and SHA-256 |
| Notes/wakes/schedules | CRUD round-trip |
| House JSON | Both legacy shapes |

Never test against a live user DB; copy to a temp dir.

---

## 10. Server contract

- **Route inventory:** parse Python decorators and Rust router; compare
  method + path. Allow an explicit ignore list for static fallback.
- **Status codes:** 400/404/409/422 basics.
- **Request/response schemas:** use `schemars` to emit JSON Schema; compare
  against `frontend/src/types/index.ts`/recorded responses.
- **CORS:** allowed origins match.
- **Auth/pairing:** local vs remote, rotate.
- **Static:** `/` serves `index.html`; `/api/unknown` is JSON 404.

---

## 11. Provider mock

`wiremock` (Codex uses it) or a local Axum test server:

- `/chat/completions` returns SSE chunks.
- Simulate:
  - text only
  - reasoning + text
  - tool call split across chunks
  - tool call with invalid JSON (retry)
  - 429 with retry-after
  - 500 then success (fallback)
  - auth failure
  - stream cut mid-tool-call
- Assert retries/fallbacks and final error mapping.

---

## 12. Compaction / token tests

- History over window triggers one compaction.
- Tool call/result pairs are never split.
- System prompt is reinjected.
- Retained tail rebuilds the same context as the in-memory path.
- Token estimate is conservative (never below provider usage).
- Cost accounting matches usage fields.

---

## 13. CI gates

```yaml
jobs:
  rust:
    steps:
      - cargo fmt --all -- --check
      - cargo clippy --workspace --all-targets -- -D warnings
      - cargo test --workspace
      - python scripts/export_tool_specs.py --check
  python:
    steps:
      - pip install -r requirements-test.txt
      - python -m pytest tests/ -q
```

Both must pass before merging to `rust-rewrite`.

Optional:
- `cargo nextest run`
- `cargo audit`
- `cargo deny check`
- `insta` review (CI rejects unaccepted snapshots)

---

## 14. Performance smoke (not a gate)

The point of Rust is not microbenchmarks; still, record:
- cold start to `GET /api/health` (target < 500 ms)
- idle RSS with no model loaded
- SSE first-token overhead vs Python
- `search_code` on the repo (target: no slower)
- SQLite chat list/search latency

Do not optimize before parity.

---

## 15. Definition of parity

- All tool specs byte-identical.
- All 42 tools have fixtures and pass.
- All ~90 routes exist with matching shapes.
- A recorded run replays with the same event sequence.
- Existing chats/projects open and mutate correctly.
- Frontend, Telegram, and voice work with no UI changes.
- Python test suite still passes against the Python backend (until removed).
