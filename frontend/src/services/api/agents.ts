import type { AgentDetail, AgentMessage, AgentNode } from '../../types';
import { API_BASE } from './base';

export async function fetchAgents(): Promise<AgentNode[]> {
  const res = await fetch(`${API_BASE}/agents`);
  if (!res.ok) return [];
  const data = await res.json().catch(() => ({}));
  return Array.isArray(data.agents) ? data.agents : [];
}

export async function fetchAgent(agentId: string): Promise<AgentDetail | null> {
  const res = await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}`);
  if (!res.ok) return null;
  const data = await res.json().catch(() => null);
  if (!data || !data.agent) return null;
  return data as AgentDetail;
}

export async function cancelAgent(agentId: string): Promise<void> {
  await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}/cancel`, { method: 'POST' });
}

export async function sendAgentMessage(
  agentId: string,
  message: string,
): Promise<{ ok: boolean; steered?: boolean; error?: string }> {
  const res = await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}/message`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ message }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) return { ok: false, error: data?.error || `HTTP ${res.status}` };
  if (data?.error) return { ok: false, error: data.error };
  return { ok: true, steered: !!data?.steered };
}

export async function fetchAgentDiff(agentId: string): Promise<{
  agent?: AgentNode;
  diff?: { stat?: string; files?: Array<{ status: string; path: string }> } | null;
  error?: string;
}> {
  const res = await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}/diff`);
  const data = await res.json().catch(() => ({}));
  if (!res.ok) return { error: data?.error || `HTTP ${res.status}` };
  if (data?.error) return { error: data.error };
  return data;
}

export async function mergeAgent(agentId: string): Promise<{ ok: boolean; error?: string; result?: unknown }> {
  const res = await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}/merge`, {
    method: 'POST',
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) return { ok: false, error: data?.error || `HTTP ${res.status}` };
  if (data?.error) return { ok: false, error: data.error };
  return { ok: true, result: data?.result };
}

export async function discardAgent(agentId: string): Promise<{ ok: boolean; error?: string }> {
  const res = await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}/discard`, {
    method: 'POST',
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) return { ok: false, error: data?.error || `HTTP ${res.status}` };
  if (data?.error) return { ok: false, error: data.error };
  return { ok: true };
}

export type { AgentMessage };
