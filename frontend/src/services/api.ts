// ── Antigravity 2.0 API Client ──

import type { AppState, ModelOption, WorkspaceInfo, ChatSummary, ChatData, ChatMessage, UserProfile, WorkspaceRules, ChatAttachment, AgentTask, TaskSchedule, StreamDelta } from '../types';

const API_BASE = '/api';

// ── Retry helper — the API server may still be booting when the frontend first loads ──

async function fetchWithRetry(url: string, options?: RequestInit, maxRetries = 8, baseDelayMs = 500): Promise<Response> {
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    try {
      const res = await fetch(url, options);
      // Vite proxy returns 502/504 when the backend isn't ready — retry those
      if (res.ok) return res;
      if (res.status === 502 || res.status === 504 || res.status === 503) {
        throw new Error(`Proxy error: HTTP ${res.status} (backend not ready)`);
      }
      return res; // real HTTP response (even 404/500) — don't retry
    } catch (err: any) {
      if (attempt === maxRetries) throw err;
      const delay = baseDelayMs * Math.pow(2, attempt);
      console.log(`[api] Retry ${attempt + 1}/${maxRetries} for ${url} — waiting ${delay}ms (${err.message})`);
      await new Promise(r => setTimeout(r, delay));
    }
  }
  throw new Error('fetchWithRetry: unreachable');
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

export async function fetchModels(): Promise<ModelOption[]> {
  const res = await fetchWithRetry(`${API_BASE}/models`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.models ?? [];
}

export interface ProviderInfo {
  provider: 'deepseek' | 'opencode';
  base_url: string;
  model_flash: string;
  model_pro: string;
  has_key: boolean;
}

export async function fetchProvider(): Promise<ProviderInfo> {
  const res = await fetchWithRetry(`${API_BASE}/provider`);
  if (!res.ok) return { provider: 'deepseek', base_url: '', model_flash: '', model_pro: '', has_key: false };
  return res.json();
}

export async function setProvider(provider: 'deepseek' | 'opencode'): Promise<ProviderInfo> {
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

// ═══════════════════════════════════════════════════════════════════════
// Workspace API
// ═══════════════════════════════════════════════════════════════════════

export async function fetchWorkspaces(): Promise<WorkspaceInfo[]> {
  const res = await fetchWithRetry(`${API_BASE}/workspaces`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.workspaces ?? [];
}

export async function addWorkspace(folderPath: string, name: string = ''): Promise<WorkspaceInfo> {
  const res = await fetch(`${API_BASE}/workspaces`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ path: folderPath, name }),
  });
  if (!res.ok) throw new Error('Failed to add workspace');
  return res.json();
}

export async function removeWorkspace(id: string): Promise<void> {
  await fetch(`${API_BASE}/workspaces/${encodeURIComponent(id)}`, { method: 'DELETE' });
}

// ═══════════════════════════════════════════════════════════════════════
// Chat API
// ═══════════════════════════════════════════════════════════════════════

export async function fetchChats(workspaceId: string): Promise<ChatSummary[]> {
  const res = await fetchWithRetry(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats`);
  if (!res.ok) return [];
  const data = await res.json();
  return data.chats ?? [];
}

export async function createChat(workspaceId: string, title: string = '', model: string = 'Automatic Fallback'): Promise<ChatData> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ title, model }),
  });
  if (!res.ok) throw new Error('Failed to create chat');
  return res.json();
}

export async function fetchChat(workspaceId: string, chatId: string): Promise<ChatData> {
  const res = await fetchWithRetry(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}`);
  if (!res.ok) throw new Error('Chat not found');
  return res.json();
}

export async function deleteChat(workspaceId: string, chatId: string): Promise<void> {
  await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}`, {
    method: 'DELETE',
  });
}

export async function addChatMessage(
  workspaceId: string,
  chatId: string,
  role: string,
  content: string,
  metadata?: Record<string, unknown>,
): Promise<ChatMessage> {
  const res = await fetch(
    `${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/messages`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ role, content, metadata }),
    },
  );
  if (!res.ok) throw new Error('Failed to add message');
  return res.json();
}

export async function uploadChatAttachments(workspaceId: string, chatId: string, files: File[]): Promise<ChatAttachment[]> {
  const body = new FormData();
  files.forEach((file) => body.append('files', file, file.name));
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/attachments`, {
    method: 'POST', body,
  });
  if (!res.ok) throw new Error('Unable to upload attachment');
  const data = await res.json();
  return data.attachments ?? [];
}

export async function fetchTasks(workspaceId: string, chatId: string): Promise<AgentTask[]> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/tasks`);
  if (!res.ok) return [];
  return res.json();
}

export async function createTask(workspaceId: string, chatId: string, payload: { prompt: string; attachments: ChatAttachment[]; mode: 'saved' | 'scheduled'; schedule?: TaskSchedule }): Promise<AgentTask> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/tasks`, {
    method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload),
  });
  if (!res.ok) throw new Error('Failed to create task');
  return res.json();
}

export async function startTask(workspaceId: string, chatId: string, taskId: string): Promise<AgentTask> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/tasks/${encodeURIComponent(taskId)}/start`, { method: 'POST' });
  if (!res.ok) throw new Error('Failed to start task');
  return res.json();
}

export async function updateTaskStatus(workspaceId: string, chatId: string, taskId: string, status: AgentTask['status']): Promise<AgentTask> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/tasks/${encodeURIComponent(taskId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ status }),
  });
  if (!res.ok) throw new Error('Failed to update task');
  return res.json();
}

export async function updateTask(workspaceId: string, chatId: string, taskId: string, payload: Partial<Pick<AgentTask, 'prompt' | 'attachments' | 'mode' | 'schedule' | 'status'>>): Promise<AgentTask> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/tasks/${encodeURIComponent(taskId)}`, {
    method: 'PATCH', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify(payload),
  });
  if (!res.ok) throw new Error('Failed to update task');
  return res.json();
}

export async function deleteTask(workspaceId: string, chatId: string, taskId: string): Promise<void> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/tasks/${encodeURIComponent(taskId)}`, { method: 'DELETE' });
  if (!res.ok) throw new Error('Failed to delete task');
}

export async function undoRun(runId: string): Promise<{ status: string; conflicts?: string[] }> {
  const res = await fetch(`${API_BASE}/runs/${encodeURIComponent(runId)}/undo`, { method: 'POST' });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'This run cannot be undone.');
  return data;
}

export async function updateChatTitle(workspaceId: string, chatId: string, title: string): Promise<void> {
  await fetch(
    `${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/title`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ title }),
    },
  );
}

// ═══════════════════════════════════════════════════════════════════════
// Settings & Rules API
// ═══════════════════════════════════════════════════════════════════════

export async function fetchProfile(): Promise<UserProfile> {
  const res = await fetch(`${API_BASE}/settings/profile`);
  if (!res.ok) throw new Error('Failed to fetch profile');
  return res.json();
}

export async function saveProfile(profile: UserProfile): Promise<void> {
  const res = await fetch(`${API_BASE}/settings/profile`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(profile),
  });
  if (!res.ok) throw new Error('Failed to save profile');
}

export async function fetchWorkspaceRules(workspaceId: string): Promise<WorkspaceRules> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/rules`);
  if (!res.ok) throw new Error('Failed to fetch workspace rules');
  return res.json();
}

export async function saveWorkspaceRules(workspaceId: string, rules: WorkspaceRules): Promise<void> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/rules`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(rules),
  });
  if (!res.ok) throw new Error('Failed to save workspace rules');
}

/**
 * Stream the agentic pipeline via SSE.
 * Calls onState for every state update, onDelta for live token deltas,
 * onDone when finished, onError on failure.
 * Returns an AbortController to cancel mid-run.
 */
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

/** P10/P2: fetch pending ask_user batches (agents block while unanswered). */
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

/** P10/P2: resolve a pending ask_user batch with the human's answers. */
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

export interface WorkspaceFileEntry { type: 'dir' | 'file'; path: string; name: string; size?: number }

/** P10/Tier-3: list a workspace folder tree. */
export async function fetchWorkspaceFiles(workspaceId: string): Promise<WorkspaceFileEntry[]> {
  try {
    const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/files`);
    if (!res.ok) return [];
    const data = await res.json();
    return Array.isArray(data?.files) ? data.files : [];
  } catch {
    return [];
  }
}

/** P10/Tier-3: read a file's content from the workspace. */
export async function fetchWorkspaceFile(workspaceId: string, path: string): Promise<{ path: string; content: string; truncated: boolean } | null> {
  try {
    const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/file?path=${encodeURIComponent(path)}`);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
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
): AbortController {
  const controller = new AbortController();
  console.log('[streamPipeline] Starting fetch to /api/run, prompt:', prompt, 'model:', model);

  // A failed state may be followed by a normal-looking `done` event. Keep
  // completion and error mutually exclusive so failures cannot look successful.
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
    body: JSON.stringify({ prompt, model, temperature, workspace_path: workspacePath, chat_id: chatId, attachments }),
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
              // Fallback: treat as data if it starts with '{'
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
            console.log('[streamPipeline] Event:', currentEvent);
            continue;
          }
          if (line.startsWith('data: ')) {
            try {
              const parsed = JSON.parse(line.slice(6).trim());
              if (currentEvent === 'delta') {
                // P10: live token deltas (text / reasoning / end)
                if (onDelta) onDelta(parsed as StreamDelta);
                else if (parsed.kind === 'end' || parsed.text) {
                  // fallback (no listener): ignore deltas
                }
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
                console.log('[streamPipeline] State received, active_node:', parsed.active_node);
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
