import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  Plus, MessageSquare, Pencil, Trash2, Check, Code2, Home, User, ChevronRight,
  ChevronDown, Search, Pin, PinOff, ChevronUp, LayoutGrid, Bot, CornerDownRight,
} from 'lucide-react';
import type { ChatSummary, ChatsFilter, SubagentInfo, WorkspaceInfo } from '../types';
import { tabId } from './useSessions';
import WorkspaceSwitcher from './WorkspaceSwitcher';
import { fetchChatSubagents } from '../services/api';
import {
  buildGlobalLookup,
  collectSpawnedChildrenOf,
  groupWorkspaceChats,
  isSpawnedChat,
} from '../utils/chatHierarchy';

interface Props {
  /** 'full' = main workspace (source dropdown + adaptive +). 'room' = House/
   * Personal workspace hamburger: search + all chats + workspaces, no dropdown. */
  variant?: 'full' | 'room';
  open: boolean;
  onClose: () => void;
  workspaces: WorkspaceInfo[];
  chatsByWs: Record<string, ChatSummary[]>;
  activeTabId: string | null;
  runningTabId: string | null;
  pinnedChats: string[];
  /** Full variant: active source filter + its callbacks. */
  chatsFilter?: ChatsFilter;
  onFilterChange?: (filter: ChatsFilter) => void;
  onTogglePin: (sessionTabId: string) => void;
  onOpenSession: (workspaceId: string, chatId: string) => void;
  /** Full variant: adaptive + actions. */
  onNewChat?: () => void;
  onNewProject?: () => void;
  onNewChatInProject?: (workspaceId: string) => void;
  onOpenRoom?: (room: 'house' | 'personal') => void;
  onRenameChat: (workspaceId: string, chatId: string, title: string) => void;
  onDeleteChat: (workspaceId: string, chatId: string) => void;
  onLaunchPersonal: () => void;
  onLaunchCoding: () => void;
  onLaunchHouse: () => void;
  onLaunchAdmin: () => void;
}

interface RowChat extends ChatSummary {
  sid: string; // session tab id
  ws: WorkspaceInfo;
  /** Nesting depth for same-workspace parent chains (0 = top-level). */
  depth?: number;
  /** Cross-workspace parent info for the "from X" badge. */
  parentTitle?: string;
  parentWsName?: string;
  /** Which top-level parent this spawned row is nested under (for keys). */
  nestParentSid?: string;
}

const FILTERS: { key: ChatsFilter; label: string; icon: typeof MessageSquare }[] = [
  { key: 'general', label: 'General', icon: MessageSquare },
  { key: 'coding', label: 'Coding', icon: Code2 },
  { key: 'house', label: 'House', icon: Home },
  { key: 'personal', label: 'Personal', icon: User },
];

function relativeTime(iso: string): string {
  if (!iso) return '';
  const diff = Date.now() - new Date(iso).getTime();
  const mins = Math.floor(diff / 60000);
  if (mins < 1) return 'now';
  if (mins < 60) return `${mins}m`;
  const hrs = Math.floor(mins / 60);
  if (hrs < 24) return `${hrs}h`;
  return `${Math.floor(hrs / 24)}d`;
}

/** Render caps — keeps the sidebar DOM light when a workspace has hundreds of
 * chats. The full list stays in memory (tabs resolve from it); only the visible
 * rows are capped, with a Load more button per section. */
const GENERAL_PAGE = 15;
const PROJECT_PAGE = 7;
const ROOM_PAGE = 15;

function LoadMoreButton({ remaining, onClick }: { remaining: number; onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="w-full mb-1.5 py-1.5 rounded-lg text-[11px] font-medium text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.04] transition-colors cursor-pointer"
    >
      Load more ({remaining})
    </button>
  );
}

/**
 * Chat management sidebar. Full variant (main workspace): a source dropdown
 * (General / Coding / House / Personal) switches which chats are listed; the
 * + adapts to the source. Room variant (House/Personal workspaces): no
 * dropdown — search + every chat in one list + the workspace launchers.
 * Rows support open, pin, rename and delete.
 */
export default function HouseSidebar({
  variant = 'full',
  open, onClose, workspaces, chatsByWs,
  activeTabId, runningTabId, pinnedChats, chatsFilter = 'general', onFilterChange, onTogglePin,
  onOpenSession, onNewChat, onNewProject, onNewChatInProject, onOpenRoom,
  onRenameChat, onDeleteChat,
  onLaunchPersonal, onLaunchCoding, onLaunchHouse, onLaunchAdmin,
}: Props) {
  const [editing, setEditing] = useState<{ sid: string; title: string } | null>(null);
  const [confirmDeleteSid, setConfirmDeleteSid] = useState<string | null>(null);
  const [deletingSid, setDeletingSid] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [collapsedWs, setCollapsedWs] = useState<Set<string>>(new Set());
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef<HTMLDivElement | null>(null);
  const deleteTimer = useRef<number | null>(null);
  const [workspaceModalOpen, setWorkspaceModalOpen] = useState(false);
  const workspaceTriggerRef = useRef<HTMLButtonElement>(null);
  const [generalLimit, setGeneralLimit] = useState(GENERAL_PAGE);
  const [roomLimit, setRoomLimit] = useState(ROOM_PAGE);
  const [projectLimits, setProjectLimits] = useState<Record<string, number>>({});
  /** Collapsed spawn-nests: parent session-tab-id -> collapsed. Default expanded. */
  const [collapsedSpawns, setCollapsedSpawns] = useState<Set<string>>(new Set());
  /** Durable subagents per parent chat (workspace agents → specialists). */
  const [subagentsBySid, setSubagentsBySid] = useState<Record<string, SubagentInfo[]>>({});
  const [subagentsLoading, setSubagentsLoading] = useState<Set<string>>(new Set());
  const subagentsRequested = useRef<Set<string>>(new Set());

  const isPinned = (sid: string) => pinnedChats.includes(sid);
  const general = workspaces.find((w) => w.isGeneral);
  const projects = workspaces.filter((w) => !w.isGeneral);
  const activeFilter = FILTERS.find((f) => f.key === chatsFilter) ?? FILTERS[0];

  const withWs = (ws: WorkspaceInfo, c: ChatSummary): RowChat => ({ ...c, sid: tabId(ws.id, c.id), ws });
  const pinnedSort = useCallback(
    (rows: RowChat[]): RowChat[] =>
      [...rows].sort((a, b) => Number(pinnedChats.includes(b.sid)) - Number(pinnedChats.includes(a.sid))),
    [pinnedChats],
  );

  /** Global chat lookup so a spawned child can name its parent anywhere. */
  const globalLookup = useMemo(
    () => buildGlobalLookup(workspaces, chatsByWs),
    [workspaces, chatsByWs],
  );

  const withParentInfo = useCallback((row: RowChat): RowChat => {
    const parentId = (row as ChatSummary).parentChatId;
    if (!parentId) return row;
    const hit = globalLookup.get(parentId);
    if (!hit) return row;
    return { ...row, parentTitle: hit.title, parentWsName: hit.wsName };
  }, [globalLookup]);

  /** Nests the user explicitly expanded — auto-collapse seeding never touches these. */
  const userExpanded = useRef<Set<string>>(new Set());
  const toggleSpawnCollapsed = useCallback((sid: string) => {
    setCollapsedSpawns((prev) => {
      const next = new Set(prev);
      if (next.has(sid)) {
        next.delete(sid);
        userExpanded.current.add(sid);
      } else {
        next.add(sid);
        userExpanded.current.delete(sid);
      }
      return next;
    });
  }, []);

  // Default-collapsed spawn nests: seed every parent that has spawned children
  // as collapsed so lower agents live inside their parent instead of reading
  // as flat siblings. User expands explicitly; seeding only ever adds.
  useEffect(() => {
    const parents = new Set<string>();
    const byId = new Set<string>();
    for (const ws of workspaces) {
      for (const c of chatsByWs[ws.id] ?? []) byId.add(c.id);
    }
    for (const ws of workspaces) {
      for (const c of chatsByWs[ws.id] ?? []) {
        const p = (c as ChatSummary).parentChatId;
        if (p && byId.has(p)) parents.add(tabId(workspaces.find((w) => (chatsByWs[w.id] ?? []).some((x) => x.id === p))?.id ?? ws.id, p));
      }
    }
    // Cross-workspace children (orchestrator → project) nest under general parents.
    if (general) {
      const lookup = buildGlobalLookup(workspaces, chatsByWs);
      for (const c of chatsByWs[general.id] ?? []) {
        const hasKids = (collectSpawnedChildrenOf(c.id, workspaces, chatsByWs, lookup) ?? []).length > 0;
        if (hasKids) parents.add(tabId(general.id, c.id));
      }
    }
    if (parents.size === 0) return;
    setCollapsedSpawns((prev) => {
      let changed = false;
      const next = new Set(prev);
      for (const sid of parents) {
        // Only auto-collapse on first sight — never re-collapse a nest the
        // user explicitly expanded this session.
        if (!next.has(sid) && !userExpanded.current.has(sid)) {
          next.add(sid);
          changed = true;
        }
      }
      return changed ? next : prev;
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workspaces, chatsByWs]);

  /** Lazily load durable subagents (workspace agent → specialists) for a parent. */
  const ensureSubagents = useCallback((row: RowChat) => {
    if (subagentsRequested.current.has(row.sid)) return;
    subagentsRequested.current.add(row.sid);
    setSubagentsLoading((prev) => new Set(prev).add(row.sid));
    fetchChatSubagents(row.ws.id, row.id)
      .then((list) => {
        if (Array.isArray(list) && list.length > 0) {
          setSubagentsBySid((prev) => ({ ...prev, [row.sid]: list }));
        }
      })
      .catch(() => {})
      .finally(() => {
        setSubagentsLoading((prev) => {
          const next = new Set(prev);
          next.delete(row.sid);
          return next;
        });
      });
  }, []);

  // Prefetch subagent badges for visible parents (cheap list calls, cached).
  useEffect(() => {
    if (!open) return;
    const pool: RowChat[] = [];
    if (general) pool.push(...(chatsByWs[general.id] ?? []).slice(0, 15).map((c) => withWs(general, c)));
    for (const ws of projects.slice(0, 6)) {
      pool.push(...(chatsByWs[ws.id] ?? []).slice(0, 5).map((c) => withWs(ws, c)));
    }
    for (const row of pool) ensureSubagents(row);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, chatsByWs, general?.id]);

  // ── Rows for the active filter ──
  const generalRows = useMemo(
    () => (general ? pinnedSort((chatsByWs[general.id] ?? []).map((c) => withWs(general, c))) : []),
    [general, chatsByWs, pinnedSort],
  );
  const codingGroups = useMemo(
    () => projects.map((ws) => ({ ws, rows: pinnedSort((chatsByWs[ws.id] ?? []).map((c) => withWs(ws, c))) })),
    [projects, chatsByWs, pinnedSort],
  );

  // Room variant: every chat in one list — pinned first, then most recent.
  const allRows = useMemo(() => (
    [
      ...(general ? (chatsByWs[general.id] ?? []).map((c) => withWs(general, c)) : []),
      ...projects.flatMap((ws) => (chatsByWs[ws.id] ?? []).map((c) => withWs(ws, c))),
    ].sort((a, b) => {
      const p = Number(pinnedChats.includes(b.sid)) - Number(pinnedChats.includes(a.sid));
      if (p !== 0) return p;
      return new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime();
    })
  ), [general, projects, chatsByWs, pinnedChats]);

  const searching = filter.trim().length > 0;
  const q = filter.trim().toLowerCase();
  const searchRows = useMemo(() => {
    if (!searching) return [];
    let pool: RowChat[] = [];
    if (chatsFilter === 'general' && general) pool = (chatsByWs[general.id] ?? []).map((c) => withWs(general, c));
    else if (chatsFilter === 'coding') pool = projects.flatMap((ws) => (chatsByWs[ws.id] ?? []).map((c) => withWs(ws, c)));
    return pinnedSort(pool.filter((r) => (r.title || 'untitled').toLowerCase().includes(q)));
  }, [searching, chatsFilter, general, projects, chatsByWs, q, pinnedSort]);

  const startRename = (row: RowChat) => {
    setConfirmDeleteSid(null);
    setEditing({ sid: row.sid, title: row.title || '' });
  };

  const commitRename = (row: RowChat) => {
    if (editing && editing.sid === row.sid && editing.title.trim() && editing.title.trim() !== row.title) {
      onRenameChat(row.ws.id, row.id, editing.title.trim());
    }
    setEditing(null);
  };

  const handleDelete = (row: RowChat) => {
    if (confirmDeleteSid === row.sid) {
      if (deletingSid) return;
      setDeletingSid(row.sid);
      setConfirmDeleteSid(null);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      window.setTimeout(() => {
        onDeleteChat(row.ws.id, row.id);
        setDeletingSid(null);
      }, 220);
    } else {
      setConfirmDeleteSid(row.sid);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      deleteTimer.current = window.setTimeout(() => {
        setConfirmDeleteSid((cur) => (cur === row.sid ? null : cur));
      }, 2500);
    }
  };

  // ── Leaf actions: session leaves share the normal chat trio (pin / delete /
  // rename), operating on the owning child chat. Keyed per-leaf so confirm +
  // editing states never collide across leaves of the same chat.
  const leafKeyOf = (chat: RowChat, leafId: string) => `${chat.sid}::${leafId}`;

  const startLeafRename = (chat: RowChat, leafKey: string) => {
    setConfirmDeleteSid(null);
    setEditing({ sid: leafKey, title: chat.title || '' });
  };

  const commitLeafRename = (chat: RowChat, leafKey: string) => {
    if (editing && editing.sid === leafKey && editing.title.trim() && editing.title.trim() !== (chat.title || '')) {
      onRenameChat(chat.ws.id, chat.id, editing.title.trim());
    }
    setEditing(null);
  };

  const handleLeafDelete = (chat: RowChat, leafKey: string) => {
    if (confirmDeleteSid === leafKey) {
      if (deletingSid) return;
      setDeletingSid(leafKey);
      setConfirmDeleteSid(null);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      window.setTimeout(() => {
        onDeleteChat(chat.ws.id, chat.id);
        setDeletingSid(null);
      }, 220);
    } else {
      setConfirmDeleteSid(leafKey);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      deleteTimer.current = window.setTimeout(() => {
        setConfirmDeleteSid((cur) => (cur === leafKey ? null : cur));
      }, 2500);
    }
  };

  /** Hover trio for a session leaf — same affordances as a normal chat row. */
  const renderLeafActions = (chat: RowChat, leafKey: string) => {
    const pinned = isPinned(chat.sid);
    const isConfirming = confirmDeleteSid === leafKey;
    return (
      <div className="absolute right-1 hidden group-hover/sub:flex items-center gap-0.5 bg-[#18181d] rounded-md border border-white/[0.06] px-0.5 py-0.5">
        <button
          title={pinned ? 'Unpin child chat' : 'Pin child chat'}
          onClick={(e) => { e.stopPropagation(); onTogglePin(chat.sid); }}
          className={`grid place-items-center w-6 h-6 rounded-md transition-colors ${
            pinned ? 'text-zinc-200' : 'text-zinc-500 hover:text-white hover:bg-white/[0.07]'
          }`}
        >
          {pinned ? <PinOff size={11} /> : <Pin size={11} />}
        </button>
        <button
          title={isConfirming ? 'Click again to delete this child chat' : 'Delete child chat'}
          onClick={(e) => { e.stopPropagation(); handleLeafDelete(chat, leafKey); }}
          className={`grid place-items-center w-6 h-6 rounded-md transition-colors ${
            isConfirming ? 'text-red-400 bg-red-500/15' : 'text-zinc-500 hover:text-white hover:bg-white/[0.07]'
          }`}
        >
          {isConfirming ? <Check size={11} /> : <Trash2 size={11} />}
        </button>
        <button
          title="Rename child chat"
          onClick={(e) => { e.stopPropagation(); startLeafRename(chat, leafKey); }}
          className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] transition-colors"
        >
          <Pencil size={11} />
        </button>
      </div>
    );
  };

  const toggleCollapsed = (wsId: string) => {
    setCollapsedWs((prev) => {
      const next = new Set(prev);
      if (next.has(wsId)) next.delete(wsId); else next.add(wsId);
      return next;
    });
  };

  const closeFilterDropdown = (e: MouseEvent) => {
    if (filterRef.current && !filterRef.current.contains(e.target as Node)) setFilterOpen(false);
  };

  // ── One chat row (shared by every view) ──
  // Hierarchy: spawned chats (orchestrator → workspace agents, workspace →
  // sub-delegations) render indented with a connector + Bot icon + "from X"
  // badge so they never read as plain top-level chats.
  const renderRow = (row: RowChat, showWs: boolean, opts?: { depth?: number; hideParentBadge?: boolean }) => {
    const isEditing = editing?.sid === row.sid;
    const isConfirming = confirmDeleteSid === row.sid;
    const isDeleting = deletingSid === row.sid;
    const isActive = row.sid === activeTabId;
    const isRunning = row.sid === runningTabId;
    const pinned = isPinned(row.sid);
    const spawned = isSpawnedChat(row);
    const depth = Math.min(opts?.depth ?? row.depth ?? 0, 4);
    const Icon = spawned ? Bot : row.ws.isGeneral ? MessageSquare : Code2;
    const parentLabel = row.parentTitle
      ? `Spawned from “${row.parentTitle}”${row.parentWsName ? ` (${row.parentWsName})` : ''}`
      : spawned
        ? 'Auto-spawned by a higher agent'
        : showWs ? `${row.title || 'Untitled'} — ${row.ws.name}` : row.title || 'Untitled';
    return (
      <div
        key={row.sid}
        onClick={() => { if (!isEditing && !isDeleting) { onOpenSession(row.ws.id, row.id); onClose(); } }}
        className={`house-sidebar-row group relative flex items-center gap-2 rounded-lg mb-1 pr-2 py-2 cursor-pointer ${
          isActive ? 'is-active' : ''
        } ${isDeleting ? 'is-deleting' : ''} ${spawned ? 'is-spawned' : ''}`}
        style={{ paddingLeft: 12 + depth * 14 }}
        title={parentLabel}
      >
        {depth > 0 && (
          <span className="pointer-events-none absolute left-0 top-0 bottom-0" style={{ left: 12 + (depth - 1) * 14 }} aria-hidden="true">
            <span className="block h-full w-px bg-white/[0.08]" />
          </span>
        )}
        {isEditing ? (
          <input
            autoFocus
            value={editing.title}
            onChange={(e) => setEditing({ sid: row.sid, title: e.target.value })}
            onBlur={() => commitRename(row)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commitRename(row);
              if (e.key === 'Escape') setEditing(null);
            }}
            onClick={(e) => e.stopPropagation()}
            className="min-w-0 flex-1 rounded-md border border-white/25 bg-black/40 px-2 py-1 text-[12px] text-white outline-none focus:border-white/40"
          />
        ) : (
          <>
            {depth > 0 && <CornerDownRight size={11} className="flex-shrink-0 text-zinc-600" />}
            {isRunning ? (
              <span className="session-run-dot shrink-0" title="Running" />
            ) : (
              <Icon size={12} className={`flex-shrink-0 ${spawned ? 'text-violet-300/80' : row.ws.isGeneral ? 'text-zinc-600' : 'text-zinc-500'}`} />
            )}
            <span className="flex-1 min-w-0">
              <span className={`block truncate text-[12.5px] ${isActive ? 'text-white' : 'text-zinc-400'}`}>
                {row.title || 'Untitled'}
              </span>
              {spawned && !opts?.hideParentBadge && (
                <span className="mt-0.5 flex items-center gap-1 min-w-0">
                  <span className="inline-flex items-center rounded-full border border-violet-400/20 bg-violet-400/[0.08] px-1.5 py-px text-[9px] font-medium text-violet-200/80">
                    auto
                  </span>
                  <span className="truncate text-[10px] text-zinc-600">
                    {row.parentTitle ? `from ${row.parentTitle}` : 'spawned by agent'}
                    {showWs && row.ws?.name ? ` · ${row.ws.name}` : ''}
                  </span>
                </span>
              )}
            </span>
            {pinned && <Pin size={10} className="text-zinc-500 shrink-0 rotate-45" />}
            {showWs && !spawned && <span className="text-[9.5px] text-zinc-600 shrink-0 truncate max-w-[64px]">{row.ws.name}</span>}
            <span className="text-[10px] text-zinc-600 flex-shrink-0 tabular-nums">{relativeTime(row.updatedAt)}</span>
            <div className="absolute right-1 hidden group-hover:flex items-center gap-0.5 bg-[#18181d] rounded-md border border-white/[0.06] px-0.5 py-0.5">
              <button
                title={pinned ? 'Unpin' : 'Pin'}
                onClick={(e) => { e.stopPropagation(); onTogglePin(row.sid); }}
                className={`grid place-items-center w-6 h-6 rounded-md transition-colors ${
                  pinned ? 'text-zinc-200' : 'text-zinc-500 hover:text-white hover:bg-white/[0.07]'
                }`}
              >
                {pinned ? <PinOff size={11} /> : <Pin size={11} />}
              </button>
              <button
                title={isConfirming ? 'Click again to confirm' : 'Delete'}
                onClick={(e) => { e.stopPropagation(); handleDelete(row); }}
                className={`grid place-items-center w-6 h-6 rounded-md transition-colors ${
                  isConfirming ? 'text-red-400 bg-red-500/15' : 'text-zinc-500 hover:text-white hover:bg-white/[0.07]'
                }`}
              >
                {isConfirming ? <Check size={11} /> : <Trash2 size={11} />}
              </button>
              <button
                title="Rename"
                onClick={(e) => { e.stopPropagation(); startRename(row); }}
                className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] transition-colors"
              >
                <Pencil size={11} />
              </button>
            </div>
          </>
        )}
      </div>
    );
  };

  /** Nested subagent leaves (workspace agent → specialists) under a chat. */
  const renderSubagents = (row: RowChat, depth: number) => {
    const list = subagentsBySid[row.sid] ?? [];
    if (list.length === 0) {
      if (subagentsLoading.has(row.sid)) {
        return (
          <div
            className="py-1 text-[10.5px] text-zinc-700 italic"
            style={{ paddingLeft: 12 + (depth + 1) * 14 }}
          >
            loading subagents…
          </div>
        );
      }
      return null;
    }
    return (
      <div className="mt-0.5 flex flex-col gap-0.5">
        {list.map((s) => {
          const leafKey = leafKeyOf(row, s.child_id);
          const isEditingLeaf = editing?.sid === leafKey;
          return (
            <div
              key={s.child_id}
              onClick={() => { if (!isEditingLeaf) { onOpenSession(row.ws.id, row.id); onClose(); } }}
              className="group/sub relative flex items-center gap-1.5 rounded-lg pr-2 py-1.5 cursor-pointer hover:bg-white/[0.04]"
              style={{ paddingLeft: 12 + (depth + 1) * 14 }}
              title={`Subagent ${s.label || s.name} — opens its chat`}
            >
              <CornerDownRight size={10} className="flex-shrink-0 text-zinc-700" />
              <Bot size={11} className="flex-shrink-0 text-zinc-600" />
              {isEditingLeaf ? (
                <input
                  autoFocus
                  value={editing.title}
                  onChange={(e) => setEditing({ sid: leafKey, title: e.target.value })}
                  onBlur={() => commitLeafRename(row, leafKey)}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') commitLeafRename(row, leafKey);
                    if (e.key === 'Escape') setEditing(null);
                  }}
                  onClick={(e) => e.stopPropagation()}
                  className="min-w-0 flex-1 rounded-md border border-white/25 bg-black/40 px-2 py-0.5 text-[11.5px] text-white outline-none focus:border-white/40"
                />
              ) : (
                <span className="flex-1 min-w-0 truncate text-[11.5px] text-zinc-500">
                  {s.label || s.name || s.child_id}
                </span>
              )}
              <span
                className={`h-1.5 w-1.5 rounded-full flex-shrink-0 ${s.status === 'running' ? 'bg-emerald-400 animate-pulse' : 'bg-zinc-700'}`}
                title={s.status}
              />
              {renderLeafActions(row, leafKey)}
            </div>
          );
        })}
      </div>
    );
  };

  /**
   * One spawned child rendered as session leaves only (no purple duplicate
   * chat row). Each leaf opens that child chat — same destination the purple
   * row had. A child with zero recorded sessions gets a single leaf-style
   * fallback row (Bot + chat title) so it stays reachable.
   */
  const renderKidLeaves = (kid: RowChat, depth: number) => {
    const leaves = renderSubagents(kid, depth);
    if (leaves) return leaves;
    const leafKey = leafKeyOf(kid, kid.id);
    const isEditingLeaf = editing?.sid === leafKey;
    return (
      <div className="mt-0.5 flex flex-col gap-0.5">
        <div
          key={kid.sid}
          onClick={() => { if (!isEditingLeaf) { onOpenSession(kid.ws.id, kid.id); onClose(); } }}
          className="group/sub relative flex items-center gap-1.5 rounded-lg pr-2 py-1.5 cursor-pointer hover:bg-white/[0.04]"
          style={{ paddingLeft: 12 + (depth + 1) * 14 }}
          title={`${kid.title || 'Untitled'} — opens child chat`}
        >
          <CornerDownRight size={10} className="flex-shrink-0 text-zinc-700" />
          <Bot size={11} className="flex-shrink-0 text-zinc-600" />
          {isEditingLeaf ? (
            <input
              autoFocus
              value={editing.title}
              onChange={(e) => setEditing({ sid: leafKey, title: e.target.value })}
              onBlur={() => commitLeafRename(kid, leafKey)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') commitLeafRename(kid, leafKey);
                if (e.key === 'Escape') setEditing(null);
              }}
              onClick={(e) => e.stopPropagation()}
              className="min-w-0 flex-1 rounded-md border border-white/25 bg-black/40 px-2 py-0.5 text-[11.5px] text-white outline-none focus:border-white/40"
            />
          ) : (
            <span className="flex-1 min-w-0 truncate text-[11.5px] text-zinc-500">
              {kid.title || 'Untitled'}
            </span>
          )}
          {renderLeafActions(kid, leafKey)}
        </div>
      </div>
    );
  };

  /** Prefetch session leaves for kids when a nest expands (lazy, cached). */
  const prefetchKids = useCallback((kids: RowChat[]) => {
    for (const k of kids) ensureSubagents(k);
  }, [ensureSubagents]);

  const renderBody = () => {
    // Room variant: nested-collapsed — spawned chats live inside their parent
    // instead of flat siblings. Search stays flat so children are findable.
    if (variant === 'room') {
      const annotate = (rows: RowChat[]) => rows.map(withParentInfo);
      if (searching) {
        const hits = annotate(allRows).filter((r) =>
          (r.title || 'untitled').toLowerCase().includes(q) || r.ws.name.toLowerCase().includes(q)
          || (r.parentTitle || '').toLowerCase().includes(q));
        if (hits.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats match “{filter.trim()}”.</div>;
        return hits.map((row) => renderRow(row, !row.ws.isGeneral, { depth: isSpawnedChat(row) ? 1 : 0 }));
      }
      if (allRows.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats yet.</div>;
      const annotated = annotate(allRows);
      const byId = new Map(annotated.map((r) => [r.id, r]));
      const tops: RowChat[] = [];
      const kidsByParent = new Map<string, RowChat[]>();
      for (const row of annotated) {
        const p = (row as ChatSummary).parentChatId;
        if (p && byId.has(p)) {
          const arr = kidsByParent.get(p) || [];
          arr.push({ ...row, depth: 1 });
          kidsByParent.set(p, arr);
        } else {
          tops.push(row);
        }
      }
      for (const arr of kidsByParent.values()) {
        arr.sort((a, b) => new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime());
      }
      const visibleTops = tops.slice(0, roomLimit);
      const remainingRows = tops.length - visibleTops.length;
      return (
        <>
          {visibleTops.map((parent) => {
            const kids = kidsByParent.get(parent.id) ?? [];
            const collapsed = kids.length > 0 && collapsedSpawns.has(parent.sid);
            return (
              <div key={parent.sid} className="mb-0.5">
                <div className="flex items-center gap-0.5">
                  {kids.length > 0 ? (
                    <button
                      onClick={(e) => { e.stopPropagation(); if (collapsed) prefetchKids(kids); toggleSpawnCollapsed(parent.sid); }}
                      title={collapsed ? `Expand ${kids.length} spawned` : 'Collapse spawned'}
                      aria-label={collapsed ? 'Expand spawned chats' : 'Collapse spawned chats'}
                      className="grid place-items-center w-5 h-5 rounded-md text-zinc-600 hover:text-zinc-200 hover:bg-white/[0.06] transition-colors cursor-pointer shrink-0"
                    >
                      {collapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
                    </button>
                  ) : (
                    <span className="w-5 shrink-0" />
                  )}
                  <div className="flex-1 min-w-0">{renderRow(parent, !parent.ws.isGeneral)}</div>
                </div>
                {!collapsed && kids.map((kid) => (
                  <div key={kid.sid} className="flex-1 min-w-0">
                    {renderKidLeaves(kid, 0)}
                  </div>
                ))}
              </div>
            );
          })}
          {remainingRows > 0 && (
            <LoadMoreButton remaining={remainingRows} onClick={() => setRoomLimit((n) => n + ROOM_PAGE)} />
          )}
        </>
      );
    }

    if (chatsFilter === 'house' || chatsFilter === 'personal') {
      const roomLabel = chatsFilter === 'house' ? 'House' : 'Personal';
      return (
        <div className="flex flex-col items-center gap-2.5 text-center px-6 py-8">
          <p className="text-[11.5px] text-zinc-600 leading-5">{roomLabel} has no chats — it's a room workspace.</p>
          <button
            onClick={() => { onOpenRoom?.(chatsFilter); onClose(); }}
            className="flex items-center gap-1.5 rounded-lg border border-white/10 bg-white/[0.04] px-3 py-1.5 text-[11.5px] text-zinc-300 hover:bg-white/[0.09] transition-colors cursor-pointer"
          >
            <ChevronRight size={12} />
            Open {roomLabel} Workspace
          </button>
        </div>
      );
    }

    if (searching) {
      if (searchRows.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats match “{filter.trim()}”.</div>;
      return searchRows.map((r) => {
        const row = withParentInfo(r);
        return renderRow(row, chatsFilter === 'coding', { depth: isSpawnedChat(row) ? 1 : 0 });
      });
    }

    if (chatsFilter === 'general') {
      if (!general) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats yet.</div>;
      // Same-workspace chains nest here too (e.g. 1+3 spawned from New
      // Conversation): spawned chats are leaves inside their parent, never
      // purple top-level siblings.
      const generalRaw = (chatsByWs[general.id] ?? []).map((c) => ({ ...c }));
      const { tops: generalTops, childrenByParent: generalKids } = groupWorkspaceChats(general, generalRaw, globalLookup);
      const generalTopRows: RowChat[] = pinnedSort(generalTops.map((t) => {
        const base = generalRows.find((r) => r.id === t.id);
        const row: RowChat = { ...(base ?? withWs(general, t)), depth: 0 };
        return withParentInfo(row);
      }));
      if (generalTopRows.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats yet.</div>;
      const visibleRows = generalTopRows.slice(0, generalLimit);
      const remainingRows = generalTopRows.length - visibleRows.length;
      return (
        <>
          {visibleRows.map((parent) => {
            // Same-workspace children first (leaves, not purple rows).
            const sameWsKids: RowChat[] = (generalKids.get(parent.id) ?? []).map((k) => {
              const base = generalRows.find((r) => r.id === k.id);
              const row: RowChat = { ...(base ?? withWs(general, k)), depth: 0, parentTitle: k.parentTitle, parentWsName: k.parentWsName };
              return withParentInfo(row);
            });
            // Orchestrator → workspace agents: cross-workspace children nest
            // here under their true parent chat (same-ws ones handled above).
            const kids = collectSpawnedChildrenOf(parent.id, workspaces, chatsByWs, globalLookup)
              .filter((k) => k.workspaceId !== general.id)
              .map((k) => {
                const ws = workspaces.find((w) => w.id === k.workspaceId);
                if (!ws) return null;
                const row: RowChat = { ...k, sid: tabId(ws.id, k.id), ws, nestParentSid: parent.sid };
                return row;
              })
              .filter((r): r is RowChat => r !== null);
            const allKids = [...sameWsKids, ...kids];
            const collapsed = collapsedSpawns.has(parent.sid);
            const subCount = (subagentsBySid[parent.sid] ?? []).length;
            const totalNested = allKids.length + subCount;
            return (
              <div key={parent.sid} className="mb-0.5">
                <div className="flex items-center gap-0.5">
                  {totalNested > 0 ? (
                    <button
                      onClick={(e) => { e.stopPropagation(); if (collapsed) prefetchKids(allKids); toggleSpawnCollapsed(parent.sid); }}
                      title={collapsed ? `Expand ${totalNested} spawned` : 'Collapse spawned'}
                      aria-label={collapsed ? 'Expand spawned chats' : 'Collapse spawned chats'}
                      className="grid place-items-center w-5 h-5 rounded-md text-zinc-600 hover:text-zinc-200 hover:bg-white/[0.06] transition-colors cursor-pointer shrink-0"
                    >
                      {collapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
                    </button>
                  ) : (
                    <span className="w-5 shrink-0" />
                  )}
                  <div className="flex-1 min-w-0">{renderRow(parent, false)}</div>
                </div>
                {!collapsed && totalNested > 0 && (
                  <div className="flex flex-col gap-0.5" role="group" aria-label={`Spawned from ${parent.title}`}>
                    {sameWsKids.map((kid) => (
                      <div key={kid.sid}>
                        {renderKidLeaves(kid, 0)}
                      </div>
                    ))}
                    {kids.map((kid) => {
                      const kidWsId = (kid as unknown as { workspaceId?: string }).workspaceId ?? kid.ws.id;
                      const wsOfKid = workspaces.find((w) => w.id === kidWsId) ?? kid.ws;
                      const showWsKid: RowChat = { ...kid, ws: wsOfKid };
                      return (
                        <div key={showWsKid.sid}>
                          {renderKidLeaves(showWsKid, showWsKid.depth ?? 1)}
                        </div>
                      );
                    })}
                    {renderSubagents(parent, 0)}
                  </div>
                )}
              </div>
            );
          })}
          {remainingRows > 0 && (
            <LoadMoreButton remaining={remainingRows} onClick={() => setGeneralLimit((n) => n + GENERAL_PAGE)} />
          )}
        </>
      );
    }

    // Coding: collapsible per-project groups with spawn hierarchy.
    // Same-workspace delegations nest under their parent; cross-workspace
    // children (orchestrator → this project) show a "from X" badge.
    if (codingGroups.length === 0) {
      return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No projects yet — use + to add one.</div>;
    }
    return codingGroups.map(({ ws, rows }) => {
      const collapsed = collapsedWs.has(ws.id);
      const rawChats = (chatsByWs[ws.id] ?? []).map((c) => ({ ...c }));
      const { tops, childrenByParent } = groupWorkspaceChats(ws, rawChats, globalLookup);
      const topRows: RowChat[] = pinnedSort(tops.map((t) => {
        const base = rows.find((r) => r.id === t.id);
        const row: RowChat = { ...(base ?? withWs(ws, t)), depth: 0 };
        return withParentInfo(row);
      }));
      const limit = projectLimits[ws.id] ?? PROJECT_PAGE;
      const visibleTops = topRows.slice(0, limit);
      const remainingRows = topRows.length - visibleTops.length;
      return (
        <div key={ws.id} className="mb-2.5">
          <div className="w-full flex items-center gap-1.5 px-1 pb-1 select-none">
            <button onClick={() => toggleCollapsed(ws.id)} className="flex items-center gap-1.5 flex-1 min-w-0 text-left cursor-pointer">
              {collapsed ? <ChevronRight size={11} className="text-zinc-600 shrink-0" /> : <ChevronDown size={11} className="text-zinc-600 shrink-0" />}
              <span className={`text-[10px] uppercase tracking-widest font-semibold truncate ${ws.exists ? 'text-zinc-600' : 'text-amber-500/70'}`}>{ws.name}</span>
              <span className="text-[9.5px] text-zinc-700 tabular-nums">{rows.length}</span>
            </button>
            <button
              onClick={() => onNewChatInProject?.(ws.id)}
              title={`New chat in ${ws.name}`}
              aria-label={`New chat in ${ws.name}`}
              className="grid place-items-center w-5 h-5 rounded-md text-zinc-600 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
            >
              <Plus size={11} />
            </button>
          </div>
          {!collapsed && (
            <div>
              {topRows.length === 0 ? (
                <div className="px-3 py-1.5 text-[11px] text-zinc-700">No chats yet</div>
              ) : (
                <>
                  {visibleTops.map((top) => {
                    const kids = (childrenByParent.get(top.id) ?? []).map((k) => {
                      const base = rows.find((r) => r.id === k.id);
                      const row: RowChat = { ...(base ?? withWs(ws, k)), depth: k.depth, parentTitle: k.parentTitle, parentWsName: k.parentWsName };
                      return withParentInfo(row);
                    });
                    const spawnCollapsed = collapsedSpawns.has(top.sid);
                    const subs = subagentsBySid[top.sid] ?? [];
                    const nestedTotal = kids.length + subs.length;
                    return (
                      <div key={top.sid} className="mb-0.5">
                        <div className="flex items-center gap-0.5">
                          {nestedTotal > 0 ? (
                            <button
                              onClick={(e) => { e.stopPropagation(); if (spawnCollapsed) prefetchKids(kids); toggleSpawnCollapsed(top.sid); }}
                              title={spawnCollapsed ? `Expand ${nestedTotal} nested` : 'Collapse nested'}
                              aria-label={spawnCollapsed ? 'Expand nested chats' : 'Collapse nested chats'}
                              className="grid place-items-center w-5 h-5 rounded-md text-zinc-600 hover:text-zinc-200 hover:bg-white/[0.06] transition-colors cursor-pointer shrink-0"
                            >
                              {spawnCollapsed ? <ChevronRight size={11} /> : <ChevronDown size={11} />}
                            </button>
                          ) : (
                            <span className="w-5 shrink-0" />
                          )}
                          <div className="flex-1 min-w-0">{renderRow(top, false, { depth: 0 })}</div>
                        </div>
                        {!spawnCollapsed && nestedTotal > 0 && (
                          <div className="flex flex-col gap-0.5" role="group" aria-label={`Nested under ${top.title}`}>
                            {kids.map((kid) => (
                              <div key={kid.sid} className="flex items-center gap-0.5">
                                <span className="w-5 shrink-0" />
                                <div className="flex-1 min-w-0">{renderKidLeaves(kid, Math.max(0, (kid.depth ?? 1) - 1))}</div>
                              </div>
                            ))}
                            {renderSubagents(top, 0)}
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {remainingRows > 0 && (
                    <LoadMoreButton
                      remaining={remainingRows}
                      onClick={() => setProjectLimits((prev) => ({ ...prev, [ws.id]: (prev[ws.id] ?? PROJECT_PAGE) + PROJECT_PAGE }))}
                    />
                  )}
                </>
              )}
            </div>
          )}
        </div>
      );
    });
  };

  return (
    <>
      {/* Backdrop */}
      <div
        onClick={onClose}
        className={`fixed inset-0 z-40 bg-black/50 backdrop-blur-[2px] transition-opacity duration-300 ${open ? 'opacity-100' : 'opacity-0 pointer-events-none'}`}
        aria-hidden="true"
      />

      {/* Panel */}
      <aside
        className={`house-sidebar-panel fixed inset-y-0 left-0 z-50 w-[292px] bg-[#0f0f13] border-r house-hairline flex flex-col transition-transform duration-300 ease-out select-none ${
          open ? 'translate-x-0' : '-translate-x-full'
        }`}
        aria-label="Chat management"
        onMouseDown={(e) => { if (filterOpen) closeFilterDropdown(e.nativeEvent); }}
      >
        {/* ── Header: source dropdown + adaptive + (full variant only) ── */}
        {variant === 'full' && (
          <div className="px-3.5 pt-4 pb-2 flex items-center gap-1.5">
          <div ref={filterRef} className="relative flex-1 min-w-0">
            <button
              onClick={() => setFilterOpen((v) => !v)}
              title="Switch chat source"
              aria-label="Switch chat source"
              className="flex items-center gap-1 text-left cursor-pointer group"
            >
              <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500 group-hover:text-zinc-300 transition-colors">
                {activeFilter.label}
              </span>
              {filterOpen ? <ChevronUp size={11} className="text-zinc-500" /> : <ChevronDown size={11} className="text-zinc-500" />}
            </button>
            {filterOpen && (
              <div className="absolute left-0 top-[calc(100%+6px)] z-50 w-[190px] rounded-xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl overflow-hidden session-flow-enter">
                {FILTERS.map(({ key, label, icon: Icon }) => (
                  <button
                    key={key}
                    onClick={() => { onFilterChange?.(key); setFilterOpen(false); }}
                    className={`w-full flex items-center gap-2.5 px-3 py-2 text-left transition-colors cursor-pointer ${
                      key === chatsFilter ? 'bg-white/[0.08] text-white' : 'text-zinc-400 hover:bg-white/[0.05] hover:text-zinc-200'
                    }`}
                  >
                    <Icon size={12} className="shrink-0" />
                    <span className="flex-1 text-[12px]">{label}</span>
                    {key === chatsFilter && <Check size={11} className="shrink-0" />}
                  </button>
                ))}
              </div>
            )}
          </div>
          <button
            onClick={() => {
              if (chatsFilter === 'general') onNewChat?.();
              else if (chatsFilter === 'coding') onNewProject?.();
              else onOpenRoom?.(chatsFilter);
            }}
            title={
              chatsFilter === 'general' ? 'New chat'
                : chatsFilter === 'coding' ? 'New project'
                  : `Open ${chatsFilter === 'house' ? 'House' : 'Personal'} Workspace`
            }
            aria-label="New"
            className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
          >
            <Plus size={13} />
          </button>
          </div>
        )}

        {/* ── Search ── */}
        <div className={`${variant === 'room' ? 'mt-3.5 ' : ''}mx-3.5 mb-2.5 flex items-center gap-1.5 px-2 py-1 rounded-md bg-white/[0.04] border border-white/[0.06]`}>
          <Search size={11} className="text-zinc-600 shrink-0" />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder={variant === 'room' ? 'Search all chats…' : `Search ${activeFilter.label.toLowerCase()} chats…`}
            className="flex-1 min-w-0 bg-transparent text-[11.5px] text-zinc-300 placeholder:text-zinc-600 focus:outline-none"
          />
          {searching && (
            <button onClick={() => setFilter('')} className="text-zinc-600 hover:text-zinc-300 text-[11px] cursor-pointer" aria-label="Clear search">×</button>
          )}
        </div>

        {/* ── List ── */}
        <div className="flex-1 min-h-0 overflow-y-auto house-thread px-2.5 pb-3.5">
          {renderBody()}
        </div>

        {/* ── Workspaces trigger (bottom) ── */}
        <div className="border-t house-hairline mt-2 px-3 py-3 shrink-0">
          <button
            ref={workspaceTriggerRef}
            onClick={() => setWorkspaceModalOpen((v) => !v)}
            aria-expanded={workspaceModalOpen}
            aria-haspopup="dialog"
            className={`w-full flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 border transition-colors cursor-pointer ${
              workspaceModalOpen
                ? 'bg-white/[0.06] border-white/[0.08] text-white'
                : 'bg-white/[0.02] border-white/[0.05] text-zinc-400 hover:bg-white/[0.05] hover:border-white/[0.08] hover:text-zinc-200'
            }`}
          >
            <span className="grid place-items-center w-7 h-7 rounded-lg bg-white/[0.04] border border-white/[0.06] shrink-0">
              <LayoutGrid size={13} />
            </span>
            <span className="flex-1 text-left text-[11px] font-semibold tracking-widest uppercase">Workspaces</span>
            <ChevronRight size={12} className={`text-zinc-500 transition-transform ${workspaceModalOpen ? 'rotate-90' : ''}`} />
          </button>
        </div>
        <WorkspaceSwitcher
          open={workspaceModalOpen && open}
          onClose={() => setWorkspaceModalOpen(false)}
          anchorRef={workspaceTriggerRef}
          onLaunchPersonal={onLaunchPersonal}
          onLaunchCoding={onLaunchCoding}
          onLaunchHouse={onLaunchHouse}
          onLaunchAdmin={onLaunchAdmin}
        />
      </aside>
    </>
  );
}
