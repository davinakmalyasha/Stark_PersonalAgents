import { useCallback, useEffect, useMemo, useState } from 'react';
import { fetchChatSubagents } from '../services/api';

const POLL_MS = 5000;

/**
 * childId → 'running' | 'done' for a chat's spawned subagents.
 * One list fetch on mount; polls only while something is still running.
 */
export function useSubagentStatuses(workspaceId: string | null, chatId: string | null) {
  const [statuses, setStatuses] = useState<Record<string, 'running' | 'done'>>({});
  const key = workspaceId && chatId ? `${workspaceId}:${chatId}` : '';

  // Reset when the chat changes (state-tracked previous key)
  const [prevKey, setPrevKey] = useState(key);
  if (prevKey !== key) {
    setPrevKey(key);
    setStatuses({});
  }

  const load = useCallback(() => {
    if (!workspaceId || !chatId) return;
    fetchChatSubagents(workspaceId, chatId)
      .then((list) => {
        const next: Record<string, 'running' | 'done'> = {};
        for (const s of list) next[s.child_id] = s.status === 'running' ? 'running' : 'done';
        setStatuses(next);
      })
      .catch(() => {});
  }, [workspaceId, chatId]);

  useEffect(() => { load(); }, [load]);

  const anyRunning = useMemo(() => Object.values(statuses).some((s) => s === 'running'), [statuses]);
  useEffect(() => {
    if (!anyRunning) return;
    const id = window.setInterval(load, POLL_MS);
    return () => window.clearInterval(id);
  }, [anyRunning, load]);

  return statuses;
}
