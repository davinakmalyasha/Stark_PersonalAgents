import { API_BASE } from './base';

export interface CloudRun {
  running: boolean;
  chat_id?: string;
  workspace_path?: string;
  started_at?: string;
}

export interface CloudStatus {
  origin: string;
  cap: number;
  configured: boolean;
  cloud_host?: string | null;
  runs: CloudRun[];
}

export interface CloudHandoff {
  id: string;
  chat_id?: string | null;
  workspace_id: string;
  workspace_rel?: string | null;
  repo?: string | null;
  branch: string;
  task?: string | null;
  status: string;
  origin: string;
  package: string;
  error?: string | null;
  created_at: string;
  updated_at: string;
}

async function asJson(res: Response) {
  if (!res.ok) {
    const text = await res.text().catch(() => '');
    throw new Error(text || `Cloud request failed (${res.status})`);
  }
  return res.json();
}

export async function fetchCloudStatus(): Promise<CloudStatus> {
  const res = await fetch(`${API_BASE}/cloud/status`);
  const data = await asJson(res);
  return {
    origin: data.origin ?? 'local',
    cap: data.cap ?? 10,
    configured: !!data.configured,
    cloud_host: data.cloud_host ?? null,
    runs: Array.isArray(data.runs) ? data.runs : [],
  };
}

export async function fetchCloudHandoffs(): Promise<CloudHandoff[]> {
  const res = await fetch(`${API_BASE}/cloud/handoffs`);
  const data = await asJson(res);
  return Array.isArray(data?.handoffs) ? data.handoffs : [];
}

export async function sendToCloud(input: {
  chat_id?: string;
  workspace_id: string;
  task?: string;
  branch?: string;
}): Promise<CloudHandoff> {
  const res = await fetch(`${API_BASE}/cloud/send`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(input),
  });
  const data = await asJson(res);
  if (!data?.handoff) throw new Error('Cloud send returned no handoff.');
  return data.handoff as CloudHandoff;
}
