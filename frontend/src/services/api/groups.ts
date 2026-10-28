import type {
  GroupOp,
  GroupProjectDetail,
  GroupProjectList,
  ScheduledTask,
} from '../../types';
import { API_BASE } from './base';

export interface CreateGroupRequest {
  goal: string;
  workspaceId: string;
}

export async function createGroupProject(input: CreateGroupRequest): Promise<{ id: string; path: string; title: string }> {
  const res = await fetch(`${API_BASE}/groups`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ goal: input.goal, workspace_id: input.workspaceId }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Could not create group project.');
  return {
    id: data.id || data.project?.id,
    path: data.path || data.project?.path || '',
    title: data.title || data.project?.goal || input.goal,
  };
}

export interface CreateScheduleInput {
  workspaceId: string;
  chatId: string;
  name: string;
  prompt: string;
  at: string;
  recurrence: 'none' | 'daily' | 'weekly' | 'monthly';
}

export async function fetchScheduledTasks(workspaceId = ''): Promise<ScheduledTask[]> {
  const query = workspaceId ? `?workspace_id=${encodeURIComponent(workspaceId)}` : '';
  const res = await fetch(`${API_BASE}/schedules${query}`);
  if (!res.ok) throw new Error('Could not load scheduled tasks.');
  const data = await res.json().catch(() => ({}));
  return Array.isArray(data.schedules) ? data.schedules : [];
}

export async function createSchedule(input: CreateScheduleInput): Promise<ScheduledTask> {
  const res = await fetch(`${API_BASE}/schedules`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      workspace_id: input.workspaceId,
      chat_id: input.chatId,
      name: input.name,
      prompt: input.prompt,
      at: input.at,
      recurrence: input.recurrence,
    }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Could not schedule task.');
  return data as ScheduledTask;
}

export async function cancelSchedule(scheduleId: string): Promise<void> {
  const res = await fetch(`${API_BASE}/schedules/${encodeURIComponent(scheduleId)}`, { method: 'DELETE' });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Could not cancel scheduled task.');
}

/** Projects visible from one workspace.
 *
 * The scoping rule lives on the server (`group_registry.list_projects_for`),
 * not here — a coding workspace sees only its own, everything else sees all.
 * Reads return an empty board rather than throwing: a board that fails to
 * load must not take the panel down with it.
 */
export async function fetchGroupProjects(workspaceId: string): Promise<GroupProjectList> {
  const query = workspaceId ? `?workspace_id=${encodeURIComponent(workspaceId)}` : '';
  try {
    const res = await fetch(`${API_BASE}/groups${query}`);
    if (!res.ok) return { projects: [], scoped: false };
    return await res.json();
  } catch {
    return { projects: [], scoped: false };
  }
}

export async function fetchGroupProject(projectId: string, events = 200): Promise<GroupProjectDetail | null> {
  try {
    const res = await fetch(
      `${API_BASE}/groups/${encodeURIComponent(projectId)}?events=${events}`,
    );
    if (!res.ok) return null;
    return await res.json();
  } catch {
    return null;
  }
}

/** Grant or reject a parked item. The only way an item leaves `awaiting_approval`. */
export async function resolveGroupApproval(
  projectId: string,
  itemId: string,
  grant: boolean,
  note = '',
): Promise<{ ok: boolean; message: string }> {
  const res = await fetch(
    `${API_BASE}/groups/${encodeURIComponent(projectId)}/items/${encodeURIComponent(itemId)}/approval`,
    {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ grant, note }),
    },
  );
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || 'Failed to resolve the approval');
  return data;
}

/** Pause, resume or close. Queued — applied on the next supervisor tick. */
export async function groupProjectOp(
  projectId: string,
  op: GroupOp,
  params: Record<string, unknown> = {},
): Promise<{ ok: boolean; queued: boolean; message: string }> {
  const res = await fetch(`${API_BASE}/groups/${encodeURIComponent(projectId)}/update`, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ op, params }),
  });
  const data = await res.json().catch(() => ({}));
  if (!res.ok) throw new Error(data.error || `Failed to ${op}`);
  return data;
}
