// ── House Workspace API client ──
// Thin wrapper over /api/house/*. The UI only talks to this module, so a
// future real smart-home backend only changes this file.

import type { HouseState, HouseDevice } from './house.types';
import { apiUrl } from '../services/api/base';

export async function fetchHouseState(): Promise<HouseState> {
  const res = await fetch(apiUrl('/api/house/state'));
  if (!res.ok) throw new Error(`House state failed: HTTP ${res.status}`);
  return res.json();
}

export async function setDeviceState(deviceId: string, state: Record<string, unknown>): Promise<HouseDevice> {
  const res = await fetch(apiUrl(`/api/house/devices/${encodeURIComponent(deviceId)}`), {
    method: 'PATCH',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ state }),
  });
  const data = await res.json();
  if (!res.ok) throw new Error(data.error || `Device update failed: HTTP ${res.status}`);
  return data as HouseDevice;
}

export async function setRoomState(roomId: string, state: Record<string, unknown>): Promise<{ applied: string[]; skipped: [string, string][] }> {
  const res = await fetch(apiUrl(`/api/house/rooms/${encodeURIComponent(roomId)}`), {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ state }),
  });
  const data = await res.json();
  if (!res.ok) throw new Error(data.error || `Room update failed: HTTP ${res.status}`);
  return data;
}

export interface SceneRunResult {
  sceneId: string;
  name: string;
  applied: string[];
  skipped: [string, string][];
}

export async function runScene(sceneId: string): Promise<SceneRunResult> {
  const res = await fetch(apiUrl(`/api/house/scenes/${encodeURIComponent(sceneId)}/run`), { method: 'POST' });
  const data = await res.json();
  if (!res.ok) throw new Error(data.error || `Scene failed: HTTP ${res.status}`);
  return data as SceneRunResult;
}

export async function resetHouse(): Promise<void> {
  const res = await fetch(apiUrl('/api/house/reset'), { method: 'POST' });
  if (!res.ok) throw new Error('House reset failed');
}
