// ── Agents Admin API client ──
// Thin wrapper over /api/agentsadmin/*. The UI never sees the admin key
// (it lives in the LangChain server process and is forwarded to the
// proxy's MCP endpoint server-side).

import { apiUrl } from '../services/api/base';

export interface AdminResult {
  ok: boolean;
  text?: string;
  error?: string;
}

async function get<T = AdminResult>(path: string): Promise<T> {
  const res = await fetch(apiUrl(path));
  const data = await res.json().catch(() => null);
  if (!res.ok) return { ok: false, error: (data?.error) || `HTTP ${res.status}` } as T;
  return data as T;
}

export const fetchAdminHealth = () => get('/api/agentsadmin/health');
export const fetchClients = () => get('/api/agentsadmin/clients');
export const fetchBots = (tenantId = '') => get(`/api/agentsadmin/bots${tenantId ? `?tenant_id=${encodeURIComponent(tenantId)}` : ''}`);
export const fetchUsage = (tenantId: string) => get(`/api/agentsadmin/usage?tenant_id=${encodeURIComponent(tenantId)}`);
export const fetchDeadReminders = (tenantId = '') => get(`/api/agentsadmin/dead-reminders${tenantId ? `?tenant_id=${encodeURIComponent(tenantId)}` : ''}`);

export async function disableBot(botId: string): Promise<AdminResult> {
  const res = await fetch(apiUrl(`/api/agentsadmin/bots/${encodeURIComponent(botId)}/disable`), { method: 'POST' });
  const data = await res.json().catch(() => null);
  if (!res.ok) return { ok: false, error: data?.error || `HTTP ${res.status}` };
  return data as AdminResult;
}
