import { useCallback, useMemo, useRef, useState } from 'react';
import {
  Plus, MessageSquare, Pencil, Trash2, Check, Code2, Home, ShieldCheck, User, ChevronRight,
  ChevronDown, Search, Pin, PinOff, ChevronUp, Palette,
} from 'lucide-react';
import type { ChatSummary, ChatsFilter, WorkspaceInfo } from '../types';
import { tabId } from './useSessions';

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
  /** Optional: design-module launcher (Main workspace only — rooms omit it). */
  onLaunchDesign?: () => void;
}

interface RowChat extends ChatSummary {
  sid: string; // session tab id
  ws: WorkspaceInfo;
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
  onLaunchPersonal, onLaunchCoding, onLaunchHouse, onLaunchAdmin, onLaunchDesign,
}: Props) {
  const [editing, setEditing] = useState<{ sid: string; title: string } | null>(null);
  const [confirmDeleteSid, setConfirmDeleteSid] = useState<string | null>(null);
  const [filter, setFilter] = useState('');
  const [collapsedWs, setCollapsedWs] = useState<Set<string>>(new Set());
  const [filterOpen, setFilterOpen] = useState(false);
  const filterRef = useRef<HTMLDivElement | null>(null);
  const deleteTimer = useRef<number | null>(null);

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
      onDeleteChat(row.ws.id, row.id);
      setConfirmDeleteSid(null);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
    } else {
      setConfirmDeleteSid(row.sid);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      deleteTimer.current = window.setTimeout(() => {
        setConfirmDeleteSid((cur) => (cur === row.sid ? null : cur));
      }, 2500);
    }
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
  const renderRow = (row: RowChat, showWs: boolean) => {
    const isEditing = editing?.sid === row.sid;
    const isConfirming = confirmDeleteSid === row.sid;
    const isActive = row.sid === activeTabId;
    const isRunning = row.sid === runningTabId;
    const pinned = isPinned(row.sid);
    const Icon = row.ws.isGeneral ? MessageSquare : Code2;
    return (
      <div
        key={row.sid}
        onClick={() => { if (!isEditing) { onOpenSession(row.ws.id, row.id); onClose(); } }}
        className={`house-sidebar-row group relative flex items-center gap-2.5 rounded-lg mb-1 pl-3 pr-2 py-2 cursor-pointer ${
          isActive ? 'is-active' : ''
        }`}
        title={showWs ? `${row.title || 'Untitled'} — ${row.ws.name}` : row.title || 'Untitled'}
      >
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
            {isRunning ? (
              <span className="session-run-dot shrink-0" title="Running" />
            ) : (
              <Icon size={12} className={`flex-shrink-0 ${row.ws.isGeneral ? 'text-zinc-600' : 'text-zinc-500'}`} />
            )}
            <span className={`flex-1 min-w-0 truncate text-[12.5px] ${isActive ? 'text-white' : 'text-zinc-400'}`}>
              {row.title || 'Untitled'}
            </span>
            {pinned && <Pin size={10} className="text-zinc-500 shrink-0 rotate-45" />}
            {showWs && <span className="text-[9.5px] text-zinc-600 shrink-0 truncate max-w-[64px]">{row.ws.name}</span>}
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

  const renderBody = () => {
    // Room variant: one flat list of every chat (title or workspace match).
    if (variant === 'room') {
      if (searching) {
        const hits = allRows.filter((r) =>
          (r.title || 'untitled').toLowerCase().includes(q) || r.ws.name.toLowerCase().includes(q));
        if (hits.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats match “{filter.trim()}”.</div>;
        return hits.map((row) => renderRow(row, !row.ws.isGeneral));
      }
      if (allRows.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats yet.</div>;
      return allRows.map((row) => renderRow(row, !row.ws.isGeneral));
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
      return searchRows.map((row) => renderRow(row, chatsFilter === 'coding'));
    }

    if (chatsFilter === 'general') {
      if (generalRows.length === 0) return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No chats yet.</div>;
      return generalRows.map((row) => renderRow(row, false));
    }

    // Coding: collapsible per-project groups
    if (codingGroups.length === 0) {
      return <div className="px-3 py-3 text-[11.5px] text-zinc-600">No projects yet — use + to add one.</div>;
    }
    return codingGroups.map(({ ws, rows }) => {
      const collapsed = collapsedWs.has(ws.id);
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
              {rows.length === 0 ? (
                <div className="px-3 py-1.5 text-[11px] text-zinc-700">No chats yet</div>
              ) : (
                rows.map((row) => renderRow(row, false))
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

        {/* ── Workspaces ── */}
        <div className="border-t house-hairline px-4 pt-3.5 pb-2 mt-2 text-[10px] uppercase tracking-widest font-semibold text-zinc-600">
          Workspaces
        </div>
        <div className="px-3 pb-3.5 flex flex-col gap-1.5">
          <div
            role="button"
            tabIndex={0}
            onClick={onLaunchPersonal}
            onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onLaunchPersonal(); } }}
            className="house-sidebar-row flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 cursor-pointer"
          >
            <span className="house-room-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-200">
              <User size={15} />
            </span>
            <div className="flex-1 min-w-0">
              <div className="text-[12.5px] font-medium text-zinc-200">Personal Workspace</div>
              <div className="text-[10px] text-zinc-500">Personal tools & life admin</div>
            </div>
            <ChevronRight size={13} className="text-zinc-600" />
          </div>
          <div
            role="button"
            tabIndex={0}
            onClick={onLaunchCoding}
            onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onLaunchCoding(); } }}
            className="house-sidebar-row flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 cursor-pointer"
          >
            <span className="house-room-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-200">
              <Code2 size={15} />
            </span>
            <div className="flex-1 min-w-0">
              <div className="text-[12.5px] font-medium text-zinc-200">Coding Workspace</div>
              <div className="text-[10px] text-zinc-500">Multi-agent software development</div>
            </div>
            <ChevronRight size={13} className="text-zinc-600" />
          </div>
          <div
            role="button"
            tabIndex={0}
            onClick={onLaunchHouse}
            onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onLaunchHouse(); } }}
            className="house-sidebar-row flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 cursor-pointer"
          >
            <span className="house-room-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-200">
              <Home size={15} />
            </span>
            <div className="flex-1 min-w-0">
              <div className="text-[12.5px] font-medium text-zinc-200">House Workspace</div>
              <div className="text-[10px] text-zinc-500">Smart home control & monitoring</div>
            </div>
            <ChevronRight size={13} className="text-zinc-600" />
          </div>
          <div
            role="button"
            tabIndex={0}
            onClick={onLaunchAdmin}
            onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onLaunchAdmin(); } }}
            className="house-sidebar-row flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 cursor-pointer"
          >
            <span className="house-room-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-200">
              <ShieldCheck size={15} />
            </span>
            <div className="flex-1 min-w-0">
              <div className="text-[12.5px] font-medium text-zinc-200">Agents Admin</div>
              <div className="text-[10px] text-zinc-500">Proxy fleet — super-operator only</div>
            </div>
            <ChevronRight size={13} className="text-zinc-600" />
          </div>
          {onLaunchDesign && (
            <div
              role="button"
              tabIndex={0}
              onClick={onLaunchDesign}
              onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onLaunchDesign(); } }}
              className="house-sidebar-row flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 cursor-pointer"
              title="OpenDesign — agent-led UI design iteration for a workspace (straight to the UI subagent loop)"
            >
              <span className="house-room-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-200">
                <Palette size={15} />
              </span>
              <div className="flex-1 min-w-0">
                <div className="text-[12.5px] font-medium text-zinc-200">OpenDesign Module</div>
                <div className="text-[10px] text-zinc-500">Agent-led UI design iteration</div>
              </div>
              <ChevronRight size={13} className="text-zinc-600" />
            </div>
          )}
        </div>
      </aside>
    </>
  );
}
