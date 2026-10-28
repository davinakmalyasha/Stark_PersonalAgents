import type { AppState, ChatAttachment, StreamDelta, ActiveRun, UiState } from '../../types';
import { API_BASE, fetchWithRetry } from './base';

export interface AskOption { label: string; description?: string }
export interface AskQuestion {
  id: string;
  question: string;
  detail?: string;
  header?: string;
  multiSelect?: boolean;
  options?: (string | AskOption)[];
  intent?: { kind: string; approve?: string };
}
export interface AskBatch { batch_id: string; questions: AskQuestion[]; created: number }
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
  modes?: ProviderMode[];
  matrix?: Record<string, { backend?: string; model?: string; iter_effort?: string; think_bias?: boolean }>;
  base_url: string;
  model_flash: string;
  model_pro: string;
  has_key: boolean;
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

export async function setProvider(provider: ProviderMode): Promise<ProviderInfo> {
  const res = await fetch(`${API_BASE}/provider`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ provider }),
  });
  if (!res.ok) throw new Error('Failed to set provider');
  return res.json();
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
