import { useState, useEffect, useRef } from 'react';
import type { AppState } from '../types';
import { fetchState } from '../services/api';

export function usePolling(intervalMs = 1000) {
  const [state, setState] = useState<AppState | null>(null);
  const mounted = useRef(true);

  useEffect(() => {
    mounted.current = true;
    let timer: ReturnType<typeof setInterval>;

    const poll = async () => {
      try {
        const s = await fetchState();
        if (mounted.current) setState(s);
      } catch {
        // silent — server may not be running yet
      }
    };

    poll(); // immediate first fetch
    timer = setInterval(poll, intervalMs);

    return () => {
      mounted.current = false;
      clearInterval(timer);
    };
  }, [intervalMs]);

  return state;
}
