import { API_BASE } from './base';

export interface CanvasWatch {
  id: string;
  mode: string;
  targets: string[];
  actionsUsed: number;
  maxActions: number;
  until?: string | null;
}

export async function fetchCanvasStatus(chatId: string): Promise<{ active: boolean; watch: CanvasWatch | null }> {
  const res = await fetch(`${API_BASE}/canvas/${encodeURIComponent(chatId)}/status`);
  if (!res.ok) return { active: false, watch: null };
  return res.json();
}
