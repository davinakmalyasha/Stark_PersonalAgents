import type { AgentNode } from '../../types';
import { API_BASE } from './base';

export async function fetchAgents(): Promise<AgentNode[]> {
  const res = await fetch(`${API_BASE}/agents`);
  if (!res.ok) return [];
  const data = await res.json().catch(() => ({}));
  return Array.isArray(data.agents) ? data.agents : [];
}

export async function cancelAgent(agentId: string): Promise<void> {
  await fetch(`${API_BASE}/agents/${encodeURIComponent(agentId)}/cancel`, { method: 'POST' });
}
