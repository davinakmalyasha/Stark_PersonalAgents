import type { AppState, ChatAttachment, StreamDelta, ActiveRun, UiState } from '../../types';
import { API_BASE, fetchWithRetry } from './base';

export interface AskOption { label: string; description?: string }
export interface AskQuestion {
  id: string;
  question: string;
  detail?: string;
  header?: string;
  multiSelect?: boolean;
  multi_select?: boolean;
  options?: (string | AskOption)[];
  intent?: { kind: string; approve?: string };
}
export interface AskBatch { batch_id: string; chat_id?: string; questions: AskQuestion[]; created: number }
export interface AskAnswer { id: string; selected: string[]; custom: string }

export async function fetchPendingQuestions(): Promise<AskBatch[]> {
  try {
    const res = await fetch(`${API_BASE}/ask/pending`);
    if (!res.ok) return [];
    const data = await res.json();
    return Array.isArray(data?.batches) ? data.batches : [];
  } catch {
    return [];
  }
}

export async function submitAskAnswers(batchId: string, answers: AskAnswer[]): Promise<boolean> {
  try {
    const res = await fetch(`${API_BASE}/ask/answer`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ batch_id: batchId, answers }),
    });
    return res.ok;
  } catch {
    return false;
  }
}

export async function fetchActiveRun(): Promise<ActiveRun> {  try {
    const res = await fetch(`${API_BASE}/runs/active`);
    if (!res.ok) return { running: false, workspace_path: '', chat_id: '', started_at: '' };
    return await res.json();
  } catch {
    return { running: false, workspace_path: '', chat_id: '', started_at: '' };
  }
}

export async function steerRun(chatId: string, message: string): Promise<boolean> {  try {
    const res = await fetch(`${API_BASE}/steer`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ chat_id: chatId, message }),
    });
    if (!res.ok) return false;
    const data = await res.json();
    return !!data?.ok;
  } catch {
    return false;
  }
}

export async function abortRun(chatId: string): Promise<boolean> {
  // Tell the backend to actually cancel the run (inbox abort + pydantic-ai
  // cancellation token). Fire-and-forget from Stop handlers — the fetch
  // abort alone only detaches this client while the run keeps burning tokens.
  if (!chatId) return false;
  try {
    const res = await fetch(`${API_BASE}/abort`, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ chat_id: chatId }),
    });
    return res.ok;
  } catch {
    return false;
  }
}

export async function fetchUiState(): Promise<UiState> {
  try {
    const res = await fetch(`${API_BASE}/ui/state`);
    if (!res.ok) return {};
    return await res.json();
  } catch {
    return {};
  }
}

export async function saveUiState(state: UiState): Promise<void> {
  try {
    await fetch(`${API_BASE}/ui/state`, {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(state),
    });
  } catch { /* non-fatal */ }
}

export async function fetchState(): Promise<AppState> {
  const res = await fetchWithRetry(`${API_BASE}/state`);
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  return res.json();
}

export async function fetchHistory(): Promise<string[]> {
  const res = await fetchWithRetry(`${API_BASE}/history`);
  if (!res.ok) return ['No recent prompts'];
  const data = await res.json();
  return data.prompts ?? ['No recent prompts'];
}

import type { ModelOption } from '../../types';
export type ProviderMode = 'free' | 'deepseek' | 'opencode' | 'command' | 'openrouter';
export interface ProviderInfo {
  provider: ProviderMode;
  provider_label?: string;
  modes?: ProviderMode[];
  matrix?: Record<string, { backend?: string; model?: string; iter_effort?: string; think_bias?: boolean }>;
  base_url: string;
  model_flash: string;
  model_pro: string;
  has_key: boolean;
  issues?: string[];
  config_file?: boolean;
  legacy_env?: string[];
  health?: Record<string, string>;
  providers?: Array<{
    id: string;
    label: string;
    base_url: string;
    key_env: string[];
    has_key: boolean;
    default_model: string;
    models: Array<{ key: string; id: string; label: string; reasoning: boolean }>;
  }>;
}
export async function fetchModels(): Promise<ModelOption[]> {
  const res = await fetchWithRetry(`${API_BASE}/models`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.models ?? [];
}

export async function fetchProvider(): Promise<ProviderInfo> {
  const res = await fetchWithRetry(`${API_BASE}/provider`);
  if (!res.ok) return { provider: 'deepseek', base_url: '', model_flash: '', model_pro: '', has_key: false };
  return res.json();
}

export async function setProvider(provider: string): Promise<{ ok: boolean; provider: string }> {
  const res = await fetch(`${API_BASE}/provider`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider }),
  });
  if (!res.ok) throw new Error('Failed to set provider');
  return res.json();
}

export interface AgentOverride {
  key?: string;
  provider?: string | null;
  model?: string | null;
  thinking?: string | null;
  tools?: string[] | null;
  skills?: string[] | null;
  mcp?: string[] | null;
  system_prompt?: string | null;
}

export interface EffectiveAgent {
  key: string;
  display: string;
  tier: string;
  provider: string;
  provider_label: string;
  base_url: string;
  model_key: string;
  model: string;
  thinking: string | null;
  reasoning?: boolean;
  tools: string[] | null;
  skills: string[] | null;
  mcp: string[] | null;
  system_prompt: string | null;
  key_env: string[];
  has_key: boolean;
}

export interface AgentConfigEntry {
  key: string;
  display: string;
  tier: string;
  configured: boolean;
  override: AgentOverride | null;
  effective: EffectiveAgent;
}

export interface AgentConfigPayload {
  agents: AgentConfigEntry[];
  issues: string[];
  active_provider: string;
  providers: Array<{
    id: string;
    label: string;
    base_url: string;
    key_env: string[];
    has_key: boolean;
    default_model: string;
    models: Array<{ key: string; id: string; label: string; reasoning: boolean }>;
  }>;
  config_file: boolean;
  legacy_env: string[];
  health?: Record<string, string>;
}

export async function fetchAgentConfigs(): Promise<AgentConfigPayload> {
  const res = await fetchWithRetry(`${API_BASE}/config/agents`);
  if (!res.ok) throw new Error(`Could not load agent config (HTTP ${res.status})`);
  return res.json();
}

export async function saveAgentConfig(key: string, override: AgentOverride): Promise<{ ok: boolean; effective: EffectiveAgent }> {
  const res = await fetch(`${API_BASE}/config/agents/${encodeURIComponent(key)}`, {
    method: 'PUT',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(override),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data?.error || `Could not save ${key}`);
  return data;
}

export async function deleteAgentConfig(key: string): Promise<void> {
  const res = await fetch(`${API_BASE}/config/agents/${encodeURIComponent(key)}`, { method: 'DELETE' });
  if (!res.ok) {
    const data = await res.json().catch(() => ({}));
    throw new Error(data?.error || `Could not reset ${key}`);
  }
}

export interface HarnessInfo {
  harness_root: string;
  install_path: string;
  error?: string | null;
  workspaces?: Array<{ id: string; name: string; path: string }>;
}

export interface MigrationCandidate {
  id: string;
  path: string;
  active: boolean;
  has_chats_db: boolean;
  has_sessions: boolean;
  has_config: boolean;
}

export async function fetchHarness(): Promise<HarnessInfo> {
  const res = await fetchWithRetry(`${API_BASE}/harness`);
  if (!res.ok) throw new Error(`Could not load harness info (HTTP ${res.status})`);
  return res.json();
}

export async function fetchMigrationStatus(): Promise<{ active: string; candidates: MigrationCandidate[] }> {
  const res = await fetchWithRetry(`${API_BASE}/migration/status`);
  if (!res.ok) throw new Error(`Could not load migration status (HTTP ${res.status})`);
  return res.json();
}

export async function importMigration(source: string): Promise<{ ok: boolean; copied: string[]; message: string }> {
  const res = await fetch(`${API_BASE}/migration/import`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ source }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data?.error || 'Migration failed');
  return data;
}

export function invokeDesktop<T>(name: string, args?: Record<string, unknown>): Promise<T> {
  const invoke = (window as unknown as { __TAURI__?: { core?: { invoke?: (name: string, args?: unknown) => Promise<T> } } }).__TAURI__?.core?.invoke;
  if (!invoke) return Promise.reject(new Error('Desktop bridge unavailable'));
  return invoke(name, args);
}

export async function fetchDesktopHarness(): Promise<{ harness_root: string; install_path: string; default_root: string; dev: boolean; error?: string | null }> {
  return invokeDesktop('harness_info');
}

export async function setDesktopHarnessRoot(path: string): Promise<string> {
  return invokeDesktop('set_harness_root', { path });
}

/** Download one portable backup zip (no keys, no models, consistent DB). */
export async function exportStarkData(): Promise<string> {
  const res = await fetch(`${API_BASE}/data/export`);
  if (!res.ok) {
    const data = await res.json().catch(() => ({}));
    throw new Error((data as { error?: string })?.error || `Export failed (HTTP ${res.status})`);
  }
  const blob = await res.blob();
  const disposition = res.headers.get('content-disposition') || '';
  const match = disposition.match(/filename="([^"]+)"/);
  const name = match?.[1] || `stark-backup-${new Date().toISOString().slice(0, 10)}.zip`;
  const url = URL.createObjectURL(blob);
  const anchor = document.createElement('a');
  anchor.href = url;
  anchor.download = name;
  anchor.click();
  URL.revokeObjectURL(url);
  return name;
}

/** Upload a backup. Staged; takes effect the next time the server starts. */
export async function importStarkData(file: File, projectsRoot?: string): Promise<{
  ok: boolean;
  restart_required: boolean;
  from_root: string;
  to_root: string;
}> {
  const form = new FormData();
  form.append('file', file);
  if (projectsRoot?.trim()) form.append('projects_root', projectsRoot.trim());
  const res = await fetch(`${API_BASE}/data/import`, { method: 'POST', body: form });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error((data as { error?: string })?.error || `Import failed (HTTP ${res.status})`);
  return data as { ok: boolean; restart_required: boolean; from_root: string; to_root: string };
}

export async function pickFolder(): Promise<string | null> {
  try {
    const result = await invokeDesktop<string | null>('select_folder');
    return result ?? null;
  } catch {
    return null;
  }
}

export async function resetSession(): Promise<void> {
  await fetch(`${API_BASE}/session/reset`, { method: 'POST' });
}

export function streamPipeline(
  prompt: string,
  model: string,
  temperature: number,
  onState: (state: AppState) => void,
  onDone: () => void,
  onError: (err: string) => void,
  workspacePath: string = '',
  chatId: string = '',
  attachments: ChatAttachment[] = [],
  onDelta?: (delta: StreamDelta) => void,
  workspaceKind: string = '',
): AbortController {
  const controller = new AbortController();
  console.log('[streamPipeline] Starting fetch to /api/run, prompt:', prompt, 'model:', model);

  let settled = false;
  const finish = () => {
    if (settled) return;
    settled = true;
    onDone();
  };
  const fail = (message: string) => {
    if (settled) return;
    settled = true;
    onError(message);
  };

  fetch(`${API_BASE}/run`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ prompt, model, temperature, workspace_path: workspacePath, chat_id: chatId, attachments, workspace_kind: workspaceKind }),
    signal: controller.signal,
  })
    .then(async (res) => {
      console.log('[streamPipeline] Response received, status:', res.status, 'ok:', res.ok);
      if (!res.ok) {
        const txt = await res.text().catch(() => '');
        console.error('[streamPipeline] HTTP error:', res.status, txt);
        fail(`HTTP ${res.status}: ${txt}`);
        return;
      }
      const reader = res.body?.getReader();
      console.log('[streamPipeline] Reader obtained:', !!reader);
      if (!reader) {
        fail('No response body');
        return;
      }
      const decoder = new TextDecoder();
      let buffer = '';
      let currentEvent = '';
      console.log('[streamPipeline] Starting to read stream...');
      while (true) {
        const { done, value } = await reader.read();
        if (done) {
          console.log('[streamPipeline] Stream done, flushing buffer:', buffer);
          if (buffer.trim()) {
            if (buffer.startsWith('data: ')) {
              try {
                const parsed = JSON.parse(buffer.slice(6).trim());
                if (parsed.error) fail(parsed.error);
                else onState(parsed as AppState);
              } catch { /* skip */ }
            } else if (buffer.startsWith('event: done')) {
              finish();
            } else {
              try {
                const parsed = JSON.parse(buffer.trim());
                if (parsed.active_node) {
                  if (parsed.error) fail(parsed.error);
                  else onState(parsed as AppState);
                }
              } catch { /* skip */ }
            }
          }
          break;
        }
        buffer += decoder.decode(value, { stream: true });
        const lines = buffer.split('\n');
        buffer = lines.pop() ?? '';
        for (const line of lines) {
          if (line.startsWith('event: ')) {
            currentEvent = line.slice(7).trim();
            continue;
          }
          if (line.startsWith('data: ')) {
            try {
              const parsed = JSON.parse(line.slice(6).trim());
              if (currentEvent === 'delta') {
                if (onDelta) onDelta(parsed as StreamDelta);
              } else if (currentEvent === 'done') {
                console.log('[streamPipeline] Done payload:', parsed.status);
                if (parsed.status === 'error') {
                  fail(parsed.error || 'The agent pipeline failed. Check the backend logs for details.');
                } else {
                  finish();
                }
              } else if (parsed.error) {
                console.error('[streamPipeline] Backend pipeline error:', parsed.error);
                onState(parsed as AppState);
                fail(parsed.error);
              } else {
                onState(parsed as AppState);
              }
              currentEvent = '';
            } catch {
              console.log('[streamPipeline] JSON parse error for line:', line);
            }
          }
          if (line.trim() === '') {
            currentEvent = '';
          }
        }
      }
      console.log('[streamPipeline] Calling onDone...');
      finish();
    })
    .catch((err) => {
      console.error('[streamPipeline] Fetch error:', err.name, err.message);
      if (err.name !== 'AbortError') {
        fail(err.message ?? String(err));
      }
    });

  return controller;
}
