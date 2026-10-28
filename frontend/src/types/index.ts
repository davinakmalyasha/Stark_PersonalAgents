// ── Stark IDE Type Definitions ──

export interface TokenCall {
  agent: string;
  model: string;
  input: number;
  output: number;
  cache_hits?: number;
  cache_misses?: number;
  cost: number;
  timestamp: string;
  where?: string;
}

export interface TokenUsage {
  total_input_tokens: number;
  total_output_tokens: number;
  total_cache_hit_tokens?: number;
  total_cache_miss_tokens?: number;
  total_cost: number;
  calls: TokenCall[];
}

export interface RunArtifacts {
  files_created?: string[];
  files_modified?: string[];
  files_deleted?: string[];
  files?: ArtifactFile[];
  total_additions?: number;
  total_deletions?: number;
  run_id?: string;
  tests_passing?: boolean;
}

export interface ArtifactFile {
  path: string;
  status: 'added' | 'modified' | 'deleted';
  additions: number;
  deletions: number;
  diff?: string;
  after_hash?: string | null;
}

export interface AgentOutputs {
  requirements: string;
  tech_spec: string;
  code: string;
  agent_report?: string;
  test_report: string;
  devops_config: string;
  analytics_report: string;
  gherkin: string;
  mermaid: string;
}

export interface ProjectRef {
  name: string;
  workspace_id: string;
  chat_id: string;
  path: string;
  full_reply: string;
}

/** P10: live token delta frame from the backend SSE stream. */
export interface StreamDelta {
  kind: 'text' | 'reasoning' | 'usage' | 'end' | 'tool_started' | 'tool_finished';
  text: string;
  agent?: string;
  model?: string;
  ts?: number;
  /** tool_started/tool_finished only: tool name + JSON-encoded args. */
  tool?: string;
  args?: string;
  /** tool_finished only. */
  ok?: boolean;
  ms?: number;
  /** Structured child session reference for live delegation cards. */
  children?: ChildReference[];
}

export interface ChildReference {
    chatId: string;
    jobId?: string;
    agentName?: string;
    label?: string;
    workspaceId?: string;
}

export interface PlanItem {
  step: string;
  status: 'pending' | 'in_progress' | 'completed';
}

export interface McpServerStatus {
  name: string;
  status: string;
  transport: 'stdio' | 'http';
  error: string;
  tool_count: number;
  scopes: string[];
}

export interface McpServerConfig {
  name?: string;
  command?: string;
  args?: string[];
  url?: string;
  env?: Record<string, string>;
  env_vars?: string[];
  cwd?: string;
  scopes?: string[];
  enabled_tools?: string[];
  disabled_tools?: string[];
  approval?: 'auto' | 'prompt';
  instructions?: string;
  startup_timeout_sec?: number;
  tool_timeout_sec?: number;
  status?: string;
  error?: string;
  tool_count?: number;
  transport?: 'stdio' | 'http';
}

export interface AppState {
  active_node: string;
  next_agent: string;
  completed_nodes: string[];
  thoughts: Record<string, string>;
  client_request: string;
  outputs: AgentOutputs;
  project_path: string;
  agents_plan: string;
  /** Codex-style plans per scope: {"developer": [{step, status}, ...]}. */
  plan?: Record<string, PlanItem[]>;
  /** Live MCP server status snapshots from the backend bridge. */
  mcp_servers?: McpServerStatus[];
  antigravity_log: { sender: string; prompt: string; response: string; timestamp: string }[];
  live_terminal_log: string;
  selected_model: string;
  selected_temp: number;
  token_usage: TokenUsage;
  artifacts?: RunArtifacts;
  /** P9: set when this run routed straight to a project's Dev agent. */
  project_ref?: ProjectRef | null;
  /** P9: set when this run created a new project workspace. */
  workspace_created?: boolean;
  /** Present when the backend completed with a provider or pipeline error. */
  error?: string;
  traceback?: string;
}

export interface AgentNode {
  id: string;
  kind: 'orchestrator' | 'workspace' | 'subagent';
  path: string;
  parentId: string | null;
  chatId: string;
  workspaceId: string;
  scope: string;
  role: string;
  nickname: string;
  status: 'idle' | 'queued' | 'running' | 'interrupted' | 'completed' | 'failed' | 'cancelled';
  model: string;
  depth: number;
  result?: string;
}

export interface ModelOption {
  id: string;
  name: string;
  provider: string;
}

// ── Chat / Workspace types ──

export type WorkspaceKind = 'general' | 'house' | 'personal' | 'project';
export interface WorkspaceInfo {
  id: string;
  name: string;
  path: string;
  exists: boolean;
  chatCount: number;
  addedAt: string;
  updatedAt?: string;
  /** @deprecated use workspaceKind */
  isGeneral?: boolean;
  /** @deprecated use workspaceKind */
  isHouse?: boolean;
  /** @deprecated use workspaceKind */
  isPersonal?: boolean;
  workspaceKind?: WorkspaceKind;
}

export function getWorkspaceKind(ws: WorkspaceInfo): WorkspaceKind {
  if ((ws as any).workspaceKind) return (ws as any).workspaceKind;
  if (ws.isGeneral) return 'general';
  if ((ws as any).isHouse) return 'house';
  if ((ws as any).isPersonal) return 'personal';
  return 'project';
}

export interface ChatSummary {
  id: string;
  title: string;
  messageCount: number;
  model: string;
  createdAt: string;
  updatedAt: string;
  parentChatId?: string;
  parentWorkspaceId?: string;
  /** Short preview of the newest message (server-side, display only). */
  lastMessage?: string;
  lastRole?: string;
}

export interface ChatData {
  id: string;
  title: string;
  model: string;
  createdAt: string;
  updatedAt: string;
  messages: ChatMessage[];
  token_usage?: TokenUsage;
  parentChatId?: string;
  parentWorkspaceId?: string;
}

export interface ChatMessage {
  id: string;
  role: 'user' | 'agent' | 'error' | 'system';
  content: string;
  timestamp: string;
  agentName?: string;
  duration?: string;
  isCollapsible?: boolean;
  metadata?: Record<string, unknown>;
}

export interface ChatAttachment {
  id: string;
  name: string;
  type: string;
  size: number;
  url?: string;
  isImage: boolean;
}

/** A child session spawned by an agent (drill-in subagent view). */
export interface SubagentInfo {
  child_id: string;
  name: string;
  label: string;
  status: 'idle' | 'running' | string;
  created: number;
  parent_session?: string | null;
}

export interface SubagentSessionEntry {
  kind: 'user' | 'assistant' | 'tool';
  content: string;
  toolName?: string;
  isError?: boolean;
  ts: number;
}

export interface SubagentSession {
  info: SubagentInfo & { parent_chat_id?: string; session_file?: string };
  entries: SubagentSessionEntry[];
}

export interface UserProfile {
  user_info: {
    name: string;
  };
  global_rules: string[];
}

export interface WorkspaceRules {
  stack: Record<string, string[]>;
  workspace_rules: string[];
}

// ── Session tabs (main workspace top bar) ───────────────────────────────────

export type SessionTabType = 'chat' | 'coding';

/** One top-bar tab = one real backend chat session. */
export interface SessionTab {
  id: string; // `${workspaceId}:${chatId}`
  type: SessionTabType;
  title: string;
  workspaceId: string;
  workspaceName: string;
  workspacePath: string;
  chatId: string;
  updatedAt: string;
  /** Spawned by a higher agent — used to nest/group instead of flat siblings. */
  parentChatId?: string;
  parentWorkspaceId?: string;
  isSpawned?: boolean;
}

/** Live pipeline status from GET /api/runs/active. */
export interface ActiveRun {
  running: boolean;
  workspace_path: string;
  chat_id: string;
  started_at: string;
}

/** Chat source filter for the sidebar list (Codex-style dropdown). */
export type ChatsFilter = 'general' | 'coding' | 'house' | 'personal';

/** Persisted UI layout (open tabs, active tab, chat management), stored via /api/ui/state. */
export interface UiState {
  open_tabs?: string[];
  active_tab?: string | null;
  pinned_chats?: string[];
  chats_filter?: ChatsFilter;
}

// ── Group projects (durable multi-agent projects) ───────────────────────────
//
// Mirrors `server/routers/groups.py`. The server sends the projection TABLES,
// not `GroupToolkit.board()`'s formatted string — a UI that had to parse prose
// back into state would make the rendering the source of truth.

/** One row in the board's project list. */
export interface GroupProjectRow {
  id: string;
  path: string;
  title: string;
  status: string;
  spent_usd?: number | null;
  /** The workspace it was created from; null means global. */
  origin_workspace_id?: string | null;
  /** True when the project opted into a shell. A member with a shell can reach
   *  anywhere, so the rest of the envelope is advisory — worth showing. */
  shell_enabled?: boolean;
  milestones_done: number;
  milestones_total: number;
  items_done: number;
  items_total: number;
  awaiting_approval: number;
  goal?: string;
  active_item?: { id: string; title: string; status: string } | null;
  active_member?: string | null;
  participants?: string[];
  last_message?: GroupMessagePreview | null;
  last_activity_ts?: number;
}

/** Small, list-safe chat preview. */
export interface GroupMessagePreview {
  seq: number;
  ts: number;
  from: string;
  to: string;
  body: string;
  kind: string;
  item_id?: string | null;
}

/** One persisted agent/supervisor message in a group project. */
export interface GroupMessage extends GroupMessagePreview {
  reply_to?: number | null;
  deliver?: boolean;
}

export interface GroupMember {
  name: string;
  display: string;
  kind?: string;
  derived_from?: string;
  model?: string;
}

export interface GroupProjectList {
  projects: GroupProjectRow[];
  /** True when the board is filtered to one coding workspace. */
  scoped: boolean;
}

/** A scheduled prompt attached to a regular chat. */
export interface ScheduledTask {
  id: string;
  name: string;
  prompt: string;
  workspaceId: string;
  chatId: string;
  nextRunAt: string;
  recurrence: 'none' | 'daily' | 'weekly' | 'monthly' | string;
  status: string;
}

export interface GroupEvent {
  seq: number;
  ts: number;
  kind: string;
  actor: string;
  milestone_id?: string | null;
  item_id?: string | null;
  data?: Record<string, unknown>;
}

export interface GroupItem {
  id: string;
  project_id: string;
  milestone_id?: string | null;
  title: string;
  detail?: string;
  status: string;
  attempts?: number;
  max_attempts?: number;
  owner_member?: string | null;
  lease_owner?: string | null;
  cost_tokens?: number;
  cost_usd?: number;
  result?: string | null;
  error?: string | null;
  blocked_reason?: string | null;
  invalidated_reason?: string | null;
  acceptance?: unknown[];
  updated_at?: number;
}

export interface GroupMilestone {
  id: string;
  project_id: string;
  title: string;
  status: string;
  depends_on?: string[];
}

export interface GroupProject {
  status?: string;
  goal?: string;
  spent_usd?: number | null;
  budget_usd?: number | null;
  halt_reason?: string | null;
  deadline?: string | number | null;
  [key: string]: unknown;
}

export interface GroupProjectDetail {
  id: string;
  path: string;
  runtime_notice?: string;
  /** Members hold a shell, so the rest of the envelope is advisory. */
  shell_enabled?: boolean;
  project: GroupProject;
  members?: GroupMember[];
  messages?: GroupMessage[];
  milestones: GroupMilestone[];
  items: GroupItem[];
  events: GroupEvent[];
}

/** The only ops the board may perform — see `groups.py` `UI_OPS`. */
export type GroupOp = 'pause' | 'resume' | 'close';

export interface AgentNoteTodo {
  text: string;
  done: boolean;
}

export interface AgentNote {
  id: string;
  scopeType: 'chat' | 'group';
  scopeId: string;
  title: string;
  body: string;
  todos: AgentNoteTodo[];
  createdAt: string;
  updatedAt: string;
}
