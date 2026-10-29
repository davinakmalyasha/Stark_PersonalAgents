# 09 — Persistence

The Rust backend owns these files. Old chats, group projects, and sessions open without migration.

---

## 1. File layout

```
<harness>/.deep_agents/
├── chats.db                 # chats + messages + FTS5
├── sessions/
│   └── {kind}/{workspace_id}/{chat_id}.jsonl
├── snapshots/
│   └── {chat_id}/{run_id}/{path}.snap
├── mcp_servers.json
├── workspaces.json
├── house_state.json
├── jobs.db
├── agent_notes.db
├── agent_wakes.db
└── schedules.db

<workspace>/.deep_agents/
├── snapshots/...            # chat rewind snapshots
└── runs/...                 # run artifacts (if any)

<group project>/.gp/
├── state.db                 # events + projections
├── plan.md
├── lock.json
└── runs/<chat_id>.jsonl
```

The harness root is `HARNESS_ROOT` or the repo root.

---

## 2. Chat DB (`chats.db`)

### 2.1 Schema

```sql
CREATE TABLE chats (
  id TEXT PRIMARY KEY,
  workspace_id TEXT,
  workspace_path TEXT,
  title TEXT,
  model TEXT,
  created_at TEXT,
  updated_at TEXT,
  message_count INTEGER DEFAULT 0,
  mode TEXT DEFAULT 'build',
  pinned INTEGER DEFAULT 0,
  tasks_json TEXT,
  parent_chat_id TEXT,
  parent_workspace_id TEXT,
  token_usage_json TEXT
);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,
  chat_id TEXT,
  role TEXT,
  content TEXT,
  timestamp TEXT,
  is_trace INTEGER DEFAULT 0,
  metadata_json TEXT,
  FOREIGN KEY(chat_id) REFERENCES chats(id) ON DELETE CASCADE
);
CREATE INDEX idx_messages_chat_ts ON messages(chat_id, timestamp);
CREATE INDEX idx_chats_ws_updated ON chats(workspace_id, updated_at);
```

FTS5:
```sql
CREATE VIRTUAL TABLE messages_fts USING fts5(
  content, title, content='messages', content_rowid='rowid', tokenize='porter'
);
```
Keep the insert/delete/update FTS triggers exactly as defined here.

### 2.2 Rules
- `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;`
- `SqlitePool` with max connections > 1; writes serialized by SQLite.
- Per-chat lock in memory for read-modify-write sequences.
- `message_count` maintained on insert/delete.
- `mode` is `build` or `plan`; preserve.
- `pinned` integer (0/1).
- Schemas are created with `CREATE TABLE IF NOT EXISTS`; upgrades only ever add columns.

### 2.3 Repository API

```rust
#[async_trait]
pub trait ChatRepository {
    async fn list_chats(&self, ws: &str) -> Result<Vec<Chat>>;
    async fn create_chat(&self, req: NewChat) -> Result<Chat>;
    async fn get_chat(&self, id: &str) -> Result<Option<Chat>>;
    async fn delete_chat(&self, id: &str) -> Result<()>;
    async fn list_messages(&self, chat: &str) -> Result<Vec<MessageRow>>;
    async fn append_message(&self, m: NewMessage) -> Result<MessageRow>;
    async fn search(&self, query: &str) -> Result<Vec<SearchHit>>;
    async fn set_mode(&self, chat: &str, mode: ChatMode) -> Result<()>;
    async fn set_title(&self, chat: &str, title: &str) -> Result<()>;
    async fn add_usage(&self, chat: &str, usage: &TokenUsage) -> Result<()>;
}
```

---

## 3. Session JSONL

Canonical: `.deep_agents/sessions/{kind}/{ws}/{chat}.jsonl`.
One JSON object per line. Header first.

### 3.1 Entries

```json
{"type":"session","version":1,"id":"...","timestamp":123,"cwd":"...","parent_session":null,"delegation_depth":0,"name":null,"chat_id":"..."}
{"type":"message","id":"...","parent_id":"...","timestamp":123,"role":"user","content":"hi"}
{"type":"message","id":"...","parent_id":"...","timestamp":124,"role":"assistant","content":[{"type":"text","text":"..."},{"type":"toolCall","id":"call_1","name":"read_file","arguments":{"file_path":"..."}}],"provider":"...","model":"...","usage":{...},"stop_reason":"toolUse"}
{"type":"message","id":"...","parent_id":"...","timestamp":125,"role":"toolResult","content":"...","tool_call_id":"call_1","tool_name":"read_file","is_error":false}
{"type":"compaction","id":"...","parent_id":"...","timestamp":126,"summary":"...","first_kept_entry_id":"...","retained_tail":[...],"tokens_before":12345}
{"type":"model_change","id":"...","parent_id":"...","timestamp":127,"provider":"...","model":"..."}
{"type":"thinking_level_change","id":"...","parent_id":"...","timestamp":128,"level":"high"}
{"type":"branch_summary","id":"...","parent_id":"...","timestamp":129,"summary":"..."}
{"type":"custom","id":"...","parent_id":"...","timestamp":130,"custom_type":"...","data":{}}
{"type":"label","id":"...","parent_id":"...","timestamp":131,"label":"..."}
{"type":"session_info","id":"...","parent_id":"...","timestamp":132,"name":"..."}
```

Content blocks:
- `text`
- `thinking`
- `toolCall` (`id`, `name`, `arguments`)
- `image` (`data`, `mime_type`)

### 3.2 Rules
- LLM history is **derived** from entries; never stored twice.
- Appends are serialized per chat; write full lines and flush.
- `compaction.retained_tail` is a self-contained checkpoint; rebuilding starts
  there.
- A child session has `parent_session` and `delegation_depth`.
- Keep field names/casing exactly as the JSONL schema above.

Rust:
```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEntry {
    Session(SessionHeader),
    Message(MessageEntry),
    Compaction(CompactionEntry),
    BranchSummary(BranchSummaryEntry),
    ModelChange(ModelChangeEntry),
    ThinkingLevelChange(ThinkingLevelChangeEntry),
    Custom(CustomEntry),
    Label(LabelEntry),
    SessionInfo(SessionInfoEntry),
}
```

Field names/casing on disk are frozen (e.g. `parentId` in JSONL headers). Match the on-disk output exactly.

---

## 4. Group project DB (`state.db`)

Exact schema in [07-group-projects.md](./07-group-projects.md) §3.1.

Additional rules:
- `events` is append-only; never update/delete.
- `seq` is `AUTOINCREMENT`.
- One transaction per event + projection + checkpoint.
- `projection_checkpoint` is per project.
- `rebuild()` replays from `seq=0`.
- `PRAGMA foreign_keys` is not required.
- `ops.state`: `pending|applied|rejected`; one event per applied op.

---

## 5. Jobs DB (`jobs.db`)

```sql
CREATE TABLE jobs (
  id TEXT PRIMARY KEY,
  parent_chat_id TEXT,
  project_id TEXT,
  item_id TEXT,
  agent TEXT NOT NULL DEFAULT '',
  chat_id TEXT,
  title TEXT,
  status TEXT NOT NULL,
  attempts INTEGER DEFAULT 0,
  lease_owner TEXT,
  lease_expires REAL,
  workdir TEXT,
  usage_json TEXT,
  result TEXT,
  error TEXT,
  artifact TEXT,
  created_at REAL NOT NULL,
  updated_at REAL NOT NULL,
  finished_at REAL,
  entry_json TEXT
);
CREATE INDEX ix_jobs_parent ON jobs(parent_chat_id, status);
CREATE INDEX ix_jobs_project ON jobs(project_id, status);
```

`entry_json` carries the full delegation entry; columns are the queryable
projection. Keep both in sync (columns are the queryable projection of the JSON).

---

## 6. Notes DB (`agent_notes.db`)

```sql
CREATE TABLE agent_notes (
  id TEXT PRIMARY KEY,
  scope_type TEXT NOT NULL,
  scope_id TEXT NOT NULL,
  workspace_path TEXT DEFAULT '',
  title TEXT DEFAULT '',
  body TEXT DEFAULT '',
  todos_json TEXT DEFAULT '[]',
  created_by TEXT DEFAULT 'orchestrator',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_agent_notes_scope ON agent_notes(scope_type, scope_id);
```

---

## 7. Wakes / canvas DB (`agent_wakes.db`)

```sql
CREATE TABLE pending_wakes (
  id TEXT PRIMARY KEY,
  workspace_path TEXT DEFAULT '',
  chat_id TEXT NOT NULL,
  text TEXT NOT NULL,
  metadata_json TEXT DEFAULT '{}',
  mode TEXT DEFAULT 'orchestrator',
  status TEXT DEFAULT 'queued',
  attempts INTEGER DEFAULT 0,
  not_before TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_pending_wakes_due ON pending_wakes(status, not_before);

CREATE TABLE canvas_watches (
  id TEXT PRIMARY KEY,
  orchestrator_chat_id TEXT NOT NULL,
  workspace_path TEXT DEFAULT '',
  mode TEXT DEFAULT 'all',
  targets_json TEXT DEFAULT '[]',
  wake_on_json TEXT DEFAULT '["completed","failed"]',
  max_actions INTEGER DEFAULT 20,
  actions_used INTEGER DEFAULT 0,
  until TEXT,
  status TEXT DEFAULT 'active',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE canvas_events (
  id TEXT PRIMARY KEY,
  watch_id TEXT NOT NULL,
  source_chat_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  payload_json TEXT DEFAULT '{}',
  created_at TEXT NOT NULL
);
```

---

## 8. Schedules DB (`schedules.db`)

```sql
CREATE TABLE orchestrator_schedules (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  prompt TEXT NOT NULL,
  workspace_path TEXT DEFAULT '',
  chat_id TEXT NOT NULL,
  run_at TEXT NOT NULL,
  next_run_at TEXT NOT NULL,
  recurrence TEXT DEFAULT 'none',
  status TEXT DEFAULT 'scheduled',
  last_run_at TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX idx_orch_schedules_due ON orchestrator_schedules(status, next_run_at);
```

Recurrence parsing supports `none`, `daily-HH:MM`, and `every-Nm`.

---

## 9. JSON files

| File | Shape |
|---|---|
| `.deep_agents/house_state.json` | rooms/devices/scenes/energy/security (two legacy shapes supported) |
| `.deep_agents/mcp_servers.json` | server configs + scopes |
| `.deep_agents/workspaces.json` | workspace registry |
| `<workspace>/profile.json` | workspace profile |
| `<workspace>/rules.md` | workspace rules |

Preserve pretty-printing and key ordering where the UI or hand-editing depends
on it. Use atomic writes (temp + rename).

---

## 10. Snapshots

```
<workspace>/.deep_agents/snapshots/<chat_id>/
├── <run_id>/<relative_path>.snap      # bytes before write; gzip > 16 KB
├── _index.json                        # runs + files + mtimes
└── runs/<run_id>.json                 # manifest
```

- Snapshot before every file mutation; best-effort, never blocks the write.
- `snap_id` format is `run-<millis>-<hex8>` with a per-run manifest.
- Restore writes bytes back and updates the index.
- LRU cap 100 runs per chat (configurable).

---

## 11. Migrations

- Migrations are additive and idempotent (`ALTER TABLE ... ADD COLUMN`,
  `CREATE TABLE IF NOT EXISTS` via guarded init). Never drop/rename a column.

---

## 12. Concurrency and durability

- WAL everywhere; `busy_timeout=5000`.
- One writer for group `state.db` (supervisor).
- Chat writes serialized per chat; reads can be concurrent.
- Session JSONL: per-chat mutex; append with `OpenOptions::append(true)`.
- Fsync policy: `synchronous=NORMAL` for chat; `FULL` for group event commits
  if durability is preferred over speed (document the choice).
- Crash safety: event + projection + checkpoint in one transaction.
- Backup: copy DB + `-wal`/`-shm` while stopped, or use `VACUUM INTO`.

---

## 13. Compatibility tests

- Open a copy of a real `chats.db`; list/read/search; append; reopen and compare.
- Open a real `.gp/state.db`; replay and compare the board.
- Load a real session JSONL; rebuild context; append; reload and compare.
- Restore a snapshot and compare SHA-256.
- Jobs/notes/wakes/schedules round-trip.
- House JSON both legacy shapes.
