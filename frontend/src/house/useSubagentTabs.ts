import { useCallback, useState } from 'react';
import type { OpenSubagentTab } from './SubagentTabsPane';

export function useSubagentTabs() {
  const [state, setState] = useState<{ tabs: OpenSubagentTab[]; activeChildId: string }>({ tabs: [], activeChildId: '' });

  const open = useCallback((childId: string, label: string, workspaceId?: string) => {
    if (!childId) return;
    setState((current) => {
      const existing = current.tabs.find((tab) => tab.childId === childId);
      const tabs = existing
        ? current.tabs.map((tab) => tab.childId === childId ? { ...tab, label, workspaceId } : tab)
        : [...current.tabs, { childId, label, workspaceId }];
      return { tabs, activeChildId: childId };
    });
  }, []);

  const close = useCallback((childId: string) => {
    setState((current) => {
      const index = current.tabs.findIndex((tab) => tab.childId === childId);
      if (index < 0) return current;
      const tabs = current.tabs.filter((tab) => tab.childId !== childId);
      const activeChildId = current.activeChildId !== childId
        ? current.activeChildId
        : (tabs[Math.min(index, tabs.length - 1)]?.childId ?? '');
      return { tabs, activeChildId };
    });
  }, []);

  const clear = useCallback(() => {
    setState({ tabs: [], activeChildId: '' });
  }, []);

  const activate = useCallback((activeChildId: string) => {
    setState((current) => current.tabs.some((tab) => tab.childId === activeChildId)
      ? { ...current, activeChildId }
      : current);
  }, []);

  return { tabs: state.tabs, activeChildId: state.activeChildId, open, close, clear, activate };
}
