import { API_BASE } from './base';

export async function undoRun(runId: string): Promise<{ status: string; conflicts?: string[] }> {
  const res = await fetch(`${API_BASE}/runs/${encodeURIComponent(runId)}/undo`, { method: 'POST' });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'This run cannot be undone.');
  return data;
}

export type SnapshotGroup = {
  run_id: string;
  created_at: number;
  files: Array<{ path: string; size: number }>;
  total_size: number;
};

export async function getChatSnapshots(chatId: string, workspacePath: string): Promise<SnapshotGroup[]> {
  const url = `${API_BASE}/chats/${encodeURIComponent(chatId)}/snapshots?workspace_path=${encodeURIComponent(workspacePath)}&limit=100`;
  const res = await fetch(url);
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Failed to list snapshots');
  return (data.groups || []) as SnapshotGroup[];
}

export async function restoreSnapshot(chatId: string, snapId: string, workspacePath: string, force = false): Promise<{ ok: boolean; message: string }> {
  const url = `${API_BASE}/chats/${encodeURIComponent(chatId)}/snapshots/${encodeURIComponent(snapId)}/restore?workspace_path=${encodeURIComponent(workspacePath)}&force=${force ? 'true' : 'false'}`;
  const res = await fetch(url, { method: 'POST' });
  const data = await res.json().catch(() => ({}));
  if (res.status === 409) {
    return { ok: false, message: data.message || 'File has changed since snapshot' };
  }
  if (!res.ok) throw new Error(data.error || 'Failed to restore snapshot');
  return data;
}

export async function restoreRun(chatId: string, runId: string, workspacePath: string): Promise<{ ok: boolean; messages: string[] }> {
  const url = `${API_BASE}/chats/${encodeURIComponent(chatId)}/runs/${encodeURIComponent(runId)}/restore?workspace_path=${encodeURIComponent(workspacePath)}`;
  const res = await fetch(url, { method: 'POST' });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Failed to restore run');
  return data;
}

export async function setChatMode(chatId: string, mode: 'plan' | 'build'): Promise<{ ok: boolean; mode: string }> {
  const res = await fetch(`${API_BASE}/chat/${encodeURIComponent(chatId)}/mode`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ mode }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Failed to set mode');
  return data;
}

export async function getChatState(chatId: string): Promise<{ chat_id: string; mode: 'plan' | 'build' }> {
  const res = await fetch(`${API_BASE}/chat/${encodeURIComponent(chatId)}/state`);
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Failed to get state');
  return data;
}
