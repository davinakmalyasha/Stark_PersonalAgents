import type {
  GroupOp,
  GroupProjectDetail,
  GroupProjectList,
} from '../../types';
import { API_BASE } from './base';

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

export async function fetchGroupProject(projectId: string, events = 60): Promise<GroupProjectDetail | null> {
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
