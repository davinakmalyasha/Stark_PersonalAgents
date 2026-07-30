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

export interface AppState {
  active_node: string;
  next_agent: string;
  completed_nodes: string[];
  thoughts: Record<string, string>;
  client_request: string;
  outputs: AgentOutputs;
  project_path: string;
  agents_plan: string;
  antigravity_log: { sender: string; prompt: string; response: string; timestamp: string }[];
  live_terminal_log: string;
  selected_model: string;
  selected_temp: number;
  token_usage: TokenUsage;
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

export interface WorkspaceInfo {
  id: string;
  name: string;
  path: string;
  exists: boolean;
  chatCount: number;
  addedAt: string;
  isGeneral?: boolean;
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

export type TaskMode = 'immediate' | 'saved' | 'scheduled';
export type TaskStatus = 'ready' | 'scheduled' | 'running' | 'completed' | 'failed';
export type TaskRecurrence = 'none' | 'daily' | 'weekly' | 'monthly';

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
