// ── Antigravity IDE Type Definitions ──

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
  kind: 'text' | 'reasoning' | 'usage' | 'end';
  text: string;
  agent?: string;
  model?: string;
  ts?: number;
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
}

export interface ChatData {
  id: string;
  title: string;
  model: string;
  createdAt: string;
  updatedAt: string;
  messages: ChatMessage[];
  token_usage?: TokenUsage;
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

export type TaskMode = 'immediate' | 'saved' | 'scheduled';
export type TaskStatus = 'ready' | 'scheduled' | 'running' | 'completed' | 'failed';
export type TaskRecurrence = 'none' | 'daily' | 'weekly' | 'monthly' | `interval-${number}`;

export interface TaskSchedule {
  nextRunAt?: string;
  recurrence: TaskRecurrence;
}

export interface AgentTask {
  id: string;
  workspaceId: string;
  chatId: string;
  prompt: string;
  attachments: ChatAttachment[];
  mode: TaskMode;
  status: TaskStatus;
  schedule?: TaskSchedule;
  createdAt: string;
  updatedAt: string;
  lastRunAt?: string;
  completedAt?: string;
  error?: string;
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
