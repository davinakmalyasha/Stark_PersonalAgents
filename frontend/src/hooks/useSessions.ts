import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ActiveRun, ChatData, ChatSummary, ChatsFilter, SessionTab, WorkspaceInfo } from '../types';
import { fetchActiveRun, fetchChats, fetchUiState, fetchWorkspaces, createChat, saveUiState } from '../services/api';

const IDLE_RUN: ActiveRun = { running: false, workspace_path: '', chat_id: '', started_at: '' };
const RUN_POLL_MS = 2500;
// Phase 8: sessions poll raised 8s→30s; the diff-gate already skips React
// re-renders, but the backend still paid N SQLite chat-list queries per tick.
// 30s + hidden-tab skip cuts idle network/DB churn ~4x without missing new
// chats for any visible feature (new-chat auto-open still fires on focus).
const SESSIONS_POLL_MS = 30000;

export function tabId(workspaceId: string, chatId: string): string {
  return `${workspaceId}:${chatId}`;
}

function splitTabId(id: string): { workspaceId: string; chatId: string } | null {
  const i = id.indexOf(':');
  if (i <= 0 || i === id.length - 1) return null;
  return { workspaceId: id.slice(0, i), chatId: id.slice(i + 1) };
}

function toSessionTab(ws: WorkspaceInfo, chat: ChatSummary): SessionTab {
  return {
    id: tabId(ws.id, chat.id),
    type: ws.isGeneral || (ws as any).isHouse || (ws as any).isPersonal ? 'chat' : 'coding',
    title: chat.title || 'Untitled',
    workspaceId: ws.id,
    workspaceName: ws.name,
    workspacePath: ws.path,
    chatId: chat.id,
    updatedAt: chat.updatedAt,
    parentChatId: (chat as ChatSummary).parentChatId,
    parentWorkspaceId: (chat as ChatSummary).parentWorkspaceId,
    isSpawned: Boolean((chat as ChatSummary).parentChatId),
  };
}

function summaryOf(chat: ChatData): ChatSummary {
  return {
    id: chat.id,
    title: chat.title || 'New Conversation',
    messageCount: chat.messages?.length ?? 0,
    model: chat.model || 'Automatic Fallback',
    createdAt: chat.createdAt,
    updatedAt: chat.updatedAt,
    parentChatId: (chat as ChatData).parentChatId,
    parentWorkspaceId: (chat as ChatData).parentWorkspaceId,
  };
}

/**
 * Session-tab layer for the main workspace top bar.
 * Tabs are real backend chats: General chats = "chat" tabs, project-workspace
 * chats = "coding" tabs. Open/active tab layout, pinned chats and the sidebar
 * filter persist via /api/ui/state; running status comes from
 * /api/runs/active (one pipeline at a time).
 */
export function useSessions() {
  const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [chatsByWs, setChatsByWs] = useState<Record<string, ChatSummary[]>>({});
  const [openTabIds, setOpenTabIds] = useState<string[]>([]);
  const [activeTabId, setActiveTabId] = useState<string | null>(null);
  const [activeRun, setActiveRun] = useState<ActiveRun>(IDLE_RUN);
  const [pinnedChats, setPinnedChats] = useState<string[]>([]);
  const [chatsFilter, setChatsFilterState] = useState<ChatsFilter>('general');
  const [booted, setBooted] = useState(false);
  const bootedRef = useRef(false);
  const knownChatIdsRef = useRef<Set<string>>(new Set());
  const lastRunJsonRef = useRef('');
  const lastSessionsJsonRef = useRef('');

  // ── Sessions snapshot: workspaces + their chats ──
  const refreshSessions = useCallback(async () => {
    const wsList = await fetchWorkspaces().catch(() => []);
    const pairs = await Promise.all(
      wsList.map(async (w) => [w.id, await fetchChats(w.id).catch(() => [])] as const),
    );
    const byWs: Record<string, ChatSummary[]> = {};
    for (const [id, chats] of pairs) byWs[id] = chats;
    return { wsList, byWs };
  }, []);

  const forceRefresh = useCallback(async () => {
    const { wsList, byWs } = await refreshSessions().catch(() => ({ wsList: [] as WorkspaceInfo[], byWs: {} as Record<string, ChatSummary[]> }));
    if (wsList.length === 0) return;
    const json = JSON.stringify({ w: wsList.map((w) => ({ id: w.id, name: w.name, path: w.path, isGeneral: w.isGeneral, exists: w.exists })), c: byWs });
    lastSessionsJsonRef.current = json;
    setWorkspaces(wsList);
    setChatsByWs(byWs);
    const known = new Set<string>();
    for (const ws of wsList) for (const c of byWs[ws.id] ?? []) known.add(tabId(ws.id, c.id));
    knownChatIdsRef.current = known;
    setOpenTabIds((prev) => prev.filter((id) => known.has(id)));
    setPinnedChats((prev) => prev.filter((id) => known.has(id)));
  }, [refreshSessions]);

  const removeChatLocal = useCallback((workspaceId: string, chatId: string) => {
    const sid = tabId(workspaceId, chatId);
    setChatsByWs((prev) => ({
      ...prev,
      [workspaceId]: (prev[workspaceId] ?? []).filter((c) => c.id !== chatId),
    }));
    setOpenTabIds((prev) => prev.filter((id) => id !== sid));
    setPinnedChats((prev) => prev.filter((id) => id !== sid));
    knownChatIdsRef.current.delete(sid);
  }, []);

  const renameChatLocal = useCallback((workspaceId: string, chatId: string, title: string) => {
    const now = new Date().toISOString();
    setChatsByWs((prev) => ({
      ...prev,
      [workspaceId]: (prev[workspaceId] ?? []).map((c) => (c.id === chatId ? { ...c, title, updatedAt: now } : c)),
    }));
  }, []);

  // ── Boot: load sessions + restore persisted layout ──
  useEffect(() => {
    let cancelled = false;
    (async () => {
      const [{ wsList, byWs }, ui] = await Promise.all([refreshSessions(), fetchUiState()]);
      if (cancelled) return;
      setWorkspaces(wsList);
      setChatsByWs(byWs);

      // Remember every existing chat so later polls can detect new ones
      // (e.g. project chats the orchestrator creates mid-run).
      const known = new Set<string>();
      for (const ws of wsList) for (const c of byWs[ws.id] ?? []) known.add(tabId(ws.id, c.id));
      knownChatIdsRef.current = known;
      lastSessionsJsonRef.current = JSON.stringify({ w: wsList.map((w) => ({ id: w.id, name: w.name, path: w.path, isGeneral: w.isGeneral, exists: w.exists })), c: byWs });

      // Restore open tabs — drop any that no longer resolve to a real chat.
      const restored = (ui.open_tabs ?? []).filter((id) => known.has(id));
      const general = wsList.find((w) => w.isGeneral);
      if (restored.length === 0 && general) {
        const chats = byWs[general.id] ?? [];
        if (chats.length > 0) restored.push(tabId(general.id, chats[0].id));
      }
      const active = ui.active_tab && restored.includes(ui.active_tab) ? ui.active_tab : (restored[0] ?? null);
      setOpenTabIds(restored);
      setActiveTabId(active);
      setPinnedChats(ui.pinned_chats ?? []);
      if (ui.chats_filter) setChatsFilterState(ui.chats_filter);
      bootedRef.current = true;
      setBooted(true);
    })();
    return () => { cancelled = true; };
  }, [refreshSessions]);

  // ── Persist layout (debounced) ──
  useEffect(() => {
    if (!bootedRef.current) return;
    const t = window.setTimeout(() => {
      saveUiState({
        open_tabs: openTabIds,
        active_tab: activeTabId,
        pinned_chats: pinnedChats,
        chats_filter: chatsFilter,
      });
    }, 400);
    return () => window.clearTimeout(t);
  }, [openTabIds, activeTabId, pinnedChats, chatsFilter]);

  // ── Poll: live run status (powers the running dot on tabs) — only
  // re-renders when the status actually changes (avoids the 2.5s tick that
  // was re-rendering House + the whole IDE for no reason).
  // Phase 8: skipped while the tab is hidden — no user-visible state to keep
  // fresh in the background.
  useEffect(() => {
    let cancelled = false;
    const tick = () => {
      if (document.hidden) return;
      fetchActiveRun().then((run) => {
        if (cancelled) return;
        const json = JSON.stringify(run);
        if (json === lastRunJsonRef.current) return;
        lastRunJsonRef.current = json;
        setActiveRun(run);
      }).catch(() => {});
    };
    tick();
    const id = window.setInterval(tick, RUN_POLL_MS);
    const onVis = () => { if (!document.hidden) tick(); };
    document.addEventListener('visibilitychange', onVis);
    return () => { cancelled = true; window.clearInterval(id); document.removeEventListener('visibilitychange', onVis); };
  }, []);

  // ── Poll: session changes (new chats auto-open, deleted chats prune) ──
  // Diff-gated: skip the state updates (and tab churn) when the fetched
  // snapshot is byte-identical to the last one. Phase 8: hidden-tab skip.
  useEffect(() => {
    let cancelled = false;
    const id = window.setInterval(async () => {
      if (document.hidden) return;
      const { wsList, byWs } = await refreshSessions().catch(() => ({ wsList: [], byWs: {} as Record<string, ChatSummary[]> }));
      if (cancelled || wsList.length === 0) return;
      const json = JSON.stringify({ w: wsList.map((w) => ({ id: w.id, name: w.name, path: w.path, isGeneral: w.isGeneral, exists: w.exists })), c: byWs });
      const isSame = json === lastSessionsJsonRef.current;
      if (isSame) return;
      lastSessionsJsonRef.current = json;
      setWorkspaces(wsList);
      setChatsByWs(byWs);

      const known = new Set<string>();
      const fresh: string[] = [];
      for (const ws of wsList) {
        for (const c of byWs[ws.id] ?? []) {
          const cid = tabId(ws.id, c.id);
          known.add(cid);
          if (!knownChatIdsRef.current.has(cid)) {
            // Auto-open new project chats (coding tasks); General/House/Personal chats only
            // open when the user creates them explicitly.
            // Spawned children (orchestrator → agent) never auto-open as top-bar
            // tabs — they live nested inside their parent (sidebar nest /
            // delegation card / drawer) to avoid flat sibling spam.
            const spawned = Boolean((c as ChatSummary).parentChatId);
            if (!ws.isGeneral && !(ws as any).isHouse && !(ws as any).isPersonal && !spawned) fresh.push(cid);
          }
        }
      }
      knownChatIdsRef.current = known;

      // Prune open tabs pointing at chats deleted elsewhere.
      setOpenTabIds((prev) => prev.filter((pid) => known.has(pid)));
      if (fresh.length > 0) {
        setOpenTabIds((prev) => [...prev, ...fresh.filter((fid) => !prev.includes(fid))]);
      }
      // Drop pins for deleted chats too.
      setPinnedChats((prev) => prev.filter((pid) => known.has(pid)));
    }, SESSIONS_POLL_MS);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [refreshSessions]);

  // ── Derived: open tabs in order ──
  const tabs = useMemo<SessionTab[]>(() => {
    const out: SessionTab[] = [];
    for (const id of openTabIds) {
      const parts = splitTabId(id);
      if (!parts) continue;
      const ws = workspaces.find((w) => w.id === parts.workspaceId);
      const chat = (ws ? chatsByWs[ws.id] : [])?.find((c) => c.id === parts.chatId);
      if (ws && chat) out.push(toSessionTab(ws, chat));
    }
    return out;
  }, [openTabIds, workspaces, chatsByWs]);

  const activeTab = useMemo<SessionTab | null>(
    () => tabs.find((t) => t.id === activeTabId) ?? null,
    [tabs, activeTabId],
  );

  // ── Derived: which tab is running (from the backend's active run) ──
  const runningTabId = useMemo<string | null>(() => {
    if (!activeRun.running) return null;
    const ws = activeRun.workspace_path
      ? workspaces.find((w) => w.path && activeRun.workspace_path
        && w.path.toLowerCase() === activeRun.workspace_path.toLowerCase())
      : workspaces.find((w) => w.isGeneral);
    if (!ws) return null;
    return tabId(ws.id, activeRun.chat_id);
  }, [activeRun, workspaces]);

  const isGlobalRunning = !!activeRun.running;

  // ── Actions ──
  const openTab = useCallback((tab: SessionTab) => {
    setOpenTabIds((prev) => (prev.includes(tab.id) ? prev : [...prev, tab.id]));
    setActiveTabId(tab.id);
    knownChatIdsRef.current.add(tab.id);
  }, []);

  const closeTab = useCallback((id: string) => {
    setOpenTabIds((prev) => {
      const next = prev.filter((t) => t !== id);
      setActiveTabId((cur) => {
        if (cur !== id) return cur;
        const idx = prev.indexOf(id);
        return next[Math.min(idx, next.length - 1)] ?? null;
      });
      return next;
    });
  }, []);

  const activateTab = useCallback((id: string) => {
    setActiveTabId(id);
  }, []);

  /** Open (and activate) any chat as a tab — sidebar selection path. */
  const openChat = useCallback((workspaceId: string, chatId: string) => {
    const ws = workspaces.find((w) => w.id === workspaceId);
    const chat = ws ? (chatsByWs[ws.id] ?? []).find((c) => c.id === chatId) : null;
    if (ws && chat) openTab(toSessionTab(ws, chat));
  }, [workspaces, chatsByWs, openTab]);

  /** Register a freshly created backend chat: cache it + open its tab. */
  const registerNewChat = useCallback((ws: WorkspaceInfo, chat: ChatData): SessionTab => {
    const summary = summaryOf(chat);
    const tab = toSessionTab(ws, summary);
    setChatsByWs((prev) => ({ ...prev, [ws.id]: [summary, ...(prev[ws.id] ?? [])] }));
    knownChatIdsRef.current.add(tab.id);
    setOpenTabIds((prev) => (prev.includes(tab.id) ? prev : [...prev, tab.id]));
    setActiveTabId(tab.id);
    return tab;
  }, []);

  /** "+ New session" — a fresh General chat, opened + activated immediately. */
  const newSession = useCallback(async (): Promise<SessionTab | null> => {
    const general = workspaces.find((w) => w.isGeneral);
    if (!general) return null;
    try {
      const chat = await createChat(general.id, 'New Conversation');
      return registerNewChat(general, chat);
    } catch {
      return null;
    }
  }, [workspaces, registerNewChat]);

  /** New chat inside a specific (project) workspace — cascade "New chat". */
  const newChatIn = useCallback(async (workspaceId: string): Promise<SessionTab | null> => {
    const ws = workspaces.find((w) => w.id === workspaceId);
    if (!ws) return null;
    try {
      const chat = await createChat(ws.id, 'New Chat');
      return registerNewChat(ws, chat);
    } catch {
      return null;
    }
  }, [workspaces, registerNewChat]);

  // ── Pins + sidebar filter ──
  const togglePin = useCallback((id: string) => {
    setPinnedChats((prev) => (prev.includes(id) ? prev.filter((p) => p !== id) : [...prev, id]));
  }, []);

  const setChatsFilter = useCallback((filter: ChatsFilter) => {
    setChatsFilterState(filter);
  }, []);

  return {
    workspaces, chatsByWs,
    tabs, activeTab, activeTabId,
    runningTabId, isGlobalRunning, activeRun,
    pinnedChats, togglePin, chatsFilter, setChatsFilter,
    openTab, closeTab, activateTab, openChat,
    newSession, newChatIn, refreshSessions, forceRefresh, removeChatLocal, renameChatLocal, booted,
  };
}
