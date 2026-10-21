import type { AgentNote } from '../../types';

const API_BASE = '/api';

async function readError(res: Response): Promise<string> {
  try {
    const body = await res.json();
    return String(body?.error || res.statusText);
  } catch {
    return res.statusText;
  }
}

export async function fetchNotes(scopeType: 'chat' | 'group', scopeId: string): Promise<AgentNote[]> {
  const res = await fetch(`${API_BASE}/notes/${encodeURIComponent(scopeType)}/${encodeURIComponent(scopeId)}`);
  if (!res.ok) throw new Error(await readError(res));
  const body = await res.json();
  return Array.isArray(body?.notes) ? body.notes : [];
}

export async function createNote(payload: {
  scope_type: 'chat' | 'group';
  scope_id: string;
  title: string;
  body?: string;
  todos?: string[];
}): Promise<AgentNote> {
  const res = await fetch(`${API_BASE}/notes`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  if (!res.ok) throw new Error(await readError(res));
  return res.json();
}

export async function updateNote(noteId: string, payload: {
  title?: string;
  body?: string;
  todos?: { text: string; done: boolean }[];
}): Promise<AgentNote> {
  const res = await fetch(`${API_BASE}/notes/${encodeURIComponent(noteId)}`, {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  });
  if (!res.ok) throw new Error(await readError(res));
  return res.json();
}

export async function deleteNote(noteId: string): Promise<void> {
  const res = await fetch(`${API_BASE}/notes/${encodeURIComponent(noteId)}`, { method: 'DELETE' });
  if (!res.ok) throw new Error(await readError(res));
}
