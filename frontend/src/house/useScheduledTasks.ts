import { useCallback, useEffect, useState } from 'react';
import type { ScheduledTask } from '../types';
import { fetchScheduledTasks } from '../services/api';

const POLL_MS = 30_000;

export function useScheduledTasks(workspaceId: string) {
  const [schedules, setSchedules] = useState<ScheduledTask[]>([]);

  const refresh = useCallback(async () => {
    try {
      setSchedules(await fetchScheduledTasks(workspaceId));
    } catch {
      // Keep the last successful snapshot visible during brief backend restarts.
    }
  }, [workspaceId]);

  useEffect(() => {
    const tick = () => { if (!document.hidden) void refresh(); };
    const initial = window.setTimeout(tick, 0);
    const timer = window.setInterval(tick, POLL_MS);
    document.addEventListener('visibilitychange', tick);
    return () => {
      window.clearTimeout(initial);
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', tick);
    };
  }, [refresh]);

  return { schedules, refresh };
}
