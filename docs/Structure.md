you type "fix this bug"
   ↓ Z3 desk saves it, opens the TV stream
   ↓ Z1a cop: "no hint, no project name → default" → send to BOSS
   ↓ Z1b boss graph wakes: analyzes, calls AI once → AI decides "delegate to Dev"
   ↓ Z1c dispatch creates a child chat for Dev, starts it in a thread
   ↓ Z1d Dev loops: read_file → edit_file → run tests … until the code works
   ↓ Z1e every edit was snapshotted & guarded; Dev reports back
   ↑ Z1b boss wakes from park, writes a short summary → stream closes
   ↓ Z3/Z4 your UI shows: live trace + report + git diff


     Z4 ─ FRONT DOORS (remote controls — what you touch)
  ┌─────────────────┐  ┌───────────────┐  ┌──────────────┐      Z4 ─ reply channels
  │ Web/Electron UI │  │ Telegram bot  │  │ Voice (mic   │      ┌─────────────┐ ┌─────────────┐
  │  React, :9000   │  │ (phone chat)  │  │  → text STT) │      │ Phone mirror│ │ TTS speech  │
  └────────┬────────┘  └───────┬───────┘  └──────┬───────┘      └─────────────┘ └─────────────┘
           └──────────────┐    │    ┌───────────┘
                          ▼    ▼    ▼
  Z3 ─ RECEPTION DESK: FastAPI server, port 8123 (server/)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ POST /api/run (message in) → saves it → starts a worker thread → returns      │
  │ an "SSE live TV" stream. Also: /api/steer, /api/voice/*, /api/house/*,        │
  │ auth/pairing middleware (local free, phone needs token)                       │
  └──────────────────────────────────┬────────────────────────────────────────────┘
                                     ▼  prompt + which folder (workspace) + which chat
  Z1a ─ ROUTER  decide_route()  — traffic cop, pure rules, no AI          ⬅ (Part 1)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ frontend hint "house/personal/dfd/design" → that direct agent (skip boss)     │
  │ "create a project…" wording  → ORCHESTRATOR                                    │
  │ mentions an existing project → straight to Dev worker (saves boss tokens)      │
  │ anything else                → ORCHESTRATOR (default)                          │
  └──────────────────────────────────┬────────────────────────────────────────────┘
                                     ▼ default path
  Z1b ─ ORCHESTRATOR "the BOSS" — pydantic-graph state machine (agents/)    ⬅ (Part 1)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ nodes: analyze → call_llm → decide → park → verify → end                      │
  │ · boss NEVER codes — only delegates & asks you questions                      │
  │ · saves its typed state to disk after every step (crash = resume)             │
  │ · "park" = sleeps on an inbox queue while workers run                         │
  └──────────────────────────────────┬────────────────────────────────────────────┘
                                     │ calls the `delegate` tool
                                     ▼
  Z1c ─ DELEGATION FAN-OUT — parallel dispatch (delegation.py)              ⬅ (Part 1)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ each job = its own child chat + own thread, run in parallel, results          │
  │ reduced into one [DELEGATION RESULT] block sent back to the boss              │
  └──────────────────────────────────┬────────────────────────────────────────────┘
                                     ▼
  Z1d ─ WORKER AGENTS "the EMPLOYEES" — each a full AI loop               ⬅ (Part 1)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ Dev (coding) · General (research) · House (smart home)                        │
  │ · the framework (pydantic-ai) runs their loop internally: model asks → tool   │
  │   runs → result returns → model asks again … until done                       │
  │ · worker node = one uniform callable (scheduler/Telegram/CLI all reuse it)    │
  │ · durable child sessions (cont-) + specialist JSON profiles (UI/DevOps)      │
  └──────────────────────────────────┬────────────────────────────────────────────┘
                                     │ "I need to edit a file / run a command"
                                     ▼
  Z1e ─ TOOL WAREHOUSE + GUARDS (tools/)                                   ⬅ (Part 1)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ registry (every tool declared once) + role-scope ACL (least privilege)        │
  │ guard pipeline: plan-mode lock, secret redaction, untrusted-marking,          │
  │ oversized-output offload · snapshot before each edit (rewind) · MCP tools     │
  └───────┬───────────────────────────────────────┬───────────────────────────────┘
          ▼                                       ▼
   real effects (files, commands,            "ask_user" → question modal
   git, house devices)                       → your answer goes back to Z1b
  ═════════════════════════════════════════════════════════════════════════════════
  Z2 ─ THE TAP & MEMORY — invisible layers that feed EVERY agent          (Part 2)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ llm/: which AI provider + model each role drinks from (4 modes, free pool    │
  │   with rate limits + fallback, cancel tokens)                                 │
  │ memory/: chat summaries, context compaction (token diet), repo map           │
  │   (navigation), project identity card, vector store (auxiliary)               │
  └───────────────────────────────────────────────────────────────────────────────┘
  Z3 ─ LIVE PIPE — makes it visible & interruptible                       (Part 3)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ sync/ shared state bus (what's happening right now) · realtime SSE bus ·      │
  │ event bus (phone mirroring) · inbox queues (steer/abort) · session logs       │
  │ (branching JSONL history) + chat stores (SQLite)                              │
  └───────────────────────────────────────────────────────────────────────────────┘
  Z4 ─ FRONTEND: hooks parse the TV feed into the trace UI (Part 4) / Telegram+Voice
  ═════════════════════════════════════════════════════════════════════════════════
  Z5 ─ GROUP PROJECTS — durable multi-agent projects      (Part 5 · DESIGNED, not built)
  ┌───────────────────────────────────────────────────────────────────────────────┐
  │ PLANNED. No code yet - see docs/group-projects/00-vision.md before            │
  │ assuming anything here exists.                                                │
  │                                                                               │
  │ a project folder is SELF-CONTAINED - it carries its own capability set:       │
  │                                                                               │
  │   PROJECT.md (intent)  AGENTS.md  deps/          .gp/ state.db + lock         │
  │   agents/ (chosen)      skills/     tools/       mcp/ (creds by NAME)         │
  │   context/  inbox.md                                                          │
  │                                                                               │
  │   you speak -> orchestrator: plan the DAG + pick the environment              │
  │                              |    and MATERIALISE it into the folder          │
  │                              v                                                │
  │                  supervisor.tick()   (no LLM, 11 phases)                      │
  │                              |                                                │
  │            milestones -> readiness -> lease -> dispatch                       │
  │            -> verify against acceptance criteria                              │
  │            -> invalidate (re-plan) -> budget/deadline/stall                   │
  │                              |                                                │
  │               events = the ONLY truth; the board is a projection              │
  │                              |                                                │
  │   provider: command . deepseek-v4.1-flash + mimo-v2.6-flash, both high        │
  │   design, phases, cloud concept, open Qs: docs/group-projects/                │
  └───────────────────────────────────────────────────────────────────────────────┘