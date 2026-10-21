import { useCallback, useEffect, useState } from 'react';
import type { GroupProjectRow } from '../types';
import { fetchGroupProjects } from '../services/api';

/** How often the panel re-reads its list. The supervisor ticks every 15s,
 *  so polling faster than this would be pure waste. */
const POLL_MS = 5000;

/**
 * Durable projects visible from one workspace.
 *
 * Scoping is decided on the server: a coding workspace sees only projects
 * created from its own folder, everything else sees them all. The hook only
 * carries the workspace id.
 */
export function useGroupProjects(workspaceId: string) {
  const [projects, setProjects] = useState<GroupProjectRow[]>([]);
  const [scoped, setScoped] = useState(false);

  const refresh = useCallback(async () => {
    const res = await fetchGroupProjects(workspaceId);
    setProjects(res.projects);
    setScoped(res.scoped);
  }, [workspaceId]);

  useEffect(() => {
    let cancelled = false;
    const tick = () => { if (!cancelled) refresh(); };
    tick();
    const id = window.setInterval(tick, POLL_MS);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [refresh]);

  return { projects, scoped, refresh };
}
