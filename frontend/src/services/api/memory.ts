import { API_BASE } from './base';

export interface MemoryWorkspaceStatus {
  workspaceId?: string;
  stage1Count?: number;
  pendingCount?: number;
  entryCount?: number;
  topicCount?: number;
  hasSummary?: boolean;
  summaryChars?: number;
  indexChars?: number;
  updatedAt?: string;
  source?: string;
  model?: string;
  maxChars?: number;
}

export interface MemoryWorkspace {
  id: string;
  name: string;
  kind: string;
  path?: string;
  status: MemoryWorkspaceStatus;
}

export interface MemoryTopic {
  name: string;
  entries: number;
  usageCount: number;
  chars: number;
}

export interface MemoryRecord {
  id: string;
  chatId: string;
  agentId?: string | null;
  request: string;
  summary: string;
  rawMemory: string;
  createdAt: string;
  consolidatedAt?: string | null;
}

export interface MemoryEntry {
  id: string;
  workspaceId: string;
  chatId?: string;
  agentId?: string | null;
  kind: string;
  topic: string;
  text: string;
  source: string;
  model: string;
  pinned: boolean;
  usageCount: number;
  lastUsed?: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface MemoryDetail {
  status: MemoryWorkspaceStatus;
  index?: string | null;
  summary?: string | null;
  topics: MemoryTopic[];
  entries: MemoryEntry[];
  records: MemoryRecord[];
}

export interface MemoryList {
  enabled: boolean;
  maxChars: number;
  workspaces: MemoryWorkspace[];
}

export interface MemoryConsolidateResult {
  ok: boolean;
  summary?: string;
  entries?: number;
  source?: string;
  model?: string;
  fallback?: boolean;
  error?: string;
}

async function getJson<T>(path: string): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`);
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data?.error || `HTTP ${res.status}`);
  return data as T;
}

async function sendJson<T>(path: string, method: string, body?: unknown): Promise<T> {
  const res = await fetch(`${API_BASE}${path}`, {
    method,
    headers: body === undefined ? undefined : { 'Content-Type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data?.error || `HTTP ${res.status}`);
  return data as T;
}

export async function fetchMemoryList(): Promise<MemoryList> {
  return getJson<MemoryList>('/api/memory');
}

export async function fetchMemory(workspaceId: string, records = 20): Promise<MemoryDetail> {
  return getJson<MemoryDetail>(
    `/api/memory/${encodeURIComponent(workspaceId)}?records=${records}`,
  );
}

export async function saveMemorySummary(workspaceId: string, summary: string): Promise<void> {
  await sendJson(`/api/memory/${encodeURIComponent(workspaceId)}`, 'PUT', { summary });
}

export async function consolidateMemory(
  workspaceId: string,
  mode: 'model' | 'deterministic',
): Promise<MemoryConsolidateResult> {
  return sendJson<MemoryConsolidateResult>(
    `/api/memory/${encodeURIComponent(workspaceId)}/consolidate`,
    'POST',
    { mode },
  );
}

export async function addMemoryNote(
  workspaceId: string,
  text: string,
  kind = 'note',
  topic = 'notes',
  pinned = true,
): Promise<void> {
  await sendJson(`/api/memory/${encodeURIComponent(workspaceId)}/notes`, 'POST', {
    text,
    kind,
    topic,
    pinned,
  });
}

export async function forgetMemoryEntry(workspaceId: string, entryId: string): Promise<void> {
  await sendJson(
    `/api/memory/${encodeURIComponent(workspaceId)}/entries/${encodeURIComponent(entryId)}`,
    'DELETE',
  );
}

export async function pinMemoryEntry(
  workspaceId: string,
  entryId: string,
  pinned: boolean,
): Promise<void> {
  await sendJson(
    `/api/memory/${encodeURIComponent(workspaceId)}/entries/${encodeURIComponent(entryId)}/pin`,
    'POST',
    { pinned },
  );
}

export async function fetchMemoryTopic(
  workspaceId: string,
  topic: string,
): Promise<{ topic: string; content: string }> {
  return getJson(
    `/api/memory/${encodeURIComponent(workspaceId)}/topics/${encodeURIComponent(topic)}`,
  );
}

export async function clearMemory(workspaceId: string): Promise<void> {
  await sendJson(`/api/memory/${encodeURIComponent(workspaceId)}`, 'DELETE');
}
