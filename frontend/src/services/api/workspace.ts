import type { WorkspaceInfo } from '../../types';
import { API_BASE, fetchWithRetry } from './base';

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

export interface WorkspaceFileEntry { type: 'dir' | 'file'; path: string; name: string; size?: number }

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

export async function fetchWorkspaceFile(workspaceId: string, path: string): Promise<{ path: string; content: string; truncated: boolean } | null> {
  try {
    const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/file?path=${encodeURIComponent(path)}`);
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}
