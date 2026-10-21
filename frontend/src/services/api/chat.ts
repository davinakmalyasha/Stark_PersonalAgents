import type { ChatSummary, ChatData, ChatMessage, ChatAttachment, SubagentInfo, SubagentSession } from '../../types';
import { API_BASE, fetchWithRetry } from './base';

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
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}`, {
    method: 'DELETE',
  });
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new Error(body || `Failed to delete chat (HTTP ${res.status})`);
  }
}

export async function fetchChatSubagents(workspaceId: string, chatId: string): Promise<SubagentInfo[]> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/subagents`);
  if (!res.ok) return [];
  const data = await res.json().catch(() => ({}));
  return Array.isArray(data.subagents) ? data.subagents : [];
}

export async function fetchSubagentSession(workspaceId: string, childId: string): Promise<SubagentSession | null> {
  const res = await fetch(`${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/subagents/${encodeURIComponent(childId)}`);
  if (!res.ok) return null;
  return res.json();
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

export interface RewindResult {
  ok: boolean;
  prompt: string;
  removed: { id: string; role: string }[];
  restore: {
    ok: boolean;
    restored: string[];
    removed: string[];
    conflicts: string[];
  };
}

export async function rewindChat(
  workspaceId: string,
  chatId: string,
  messageId: string,
  opts: { restoreFiles?: boolean; force?: boolean } = {},
): Promise<RewindResult> {
  const res = await fetch(
    `${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/rewind`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({
        message_id: messageId,
        restore_files: opts.restoreFiles !== false,
        force: !!opts.force,
      }),
    },
  );
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data?.error || `Rewind failed (HTTP ${res.status})`);
  return data as RewindResult;
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

export async function updateChatTitle(workspaceId: string, chatId: string, title: string): Promise<void> {
  const res = await fetch(
    `${API_BASE}/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/title`,
    {
      method: 'PUT',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ title }),
    },
  );
  if (!res.ok) {
    const body = await res.text().catch(() => '');
    throw new Error(body || `Failed to rename chat (HTTP ${res.status})`);
  }
}
