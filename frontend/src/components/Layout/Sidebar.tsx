import { useState, useEffect, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { MessageSquare, Plus, Trash2, Pencil, MoreHorizontal, ChevronDown, Bot, CornerDownRight } from 'lucide-react';
import { PanelLeftClose, FolderPlus } from 'lucide-react';
import { isSpawnedChat } from '../../utils/chatHierarchy';
import { apiUrl, fetchWorkspaces, addWorkspace, removeWorkspace, fetchChats, createChat, deleteChat, fetchChat, updateChatTitle } from '../../services/api';
import type { WorkspaceInfo, ChatSummary, ChatData } from '../../types';
import SidebarProjectList from './SidebarProjectList';
import SidebarAddFolderModal from './SidebarAddFolderModal';

interface Props {
  activeWorkspaceId: string | null;
  activeChatId: string | null;
  activeChatTitle?: string;
  collapsed: boolean;
  onSelectWorkspace: (ws: WorkspaceInfo) => void;
  onSelectChat: (ws: WorkspaceInfo, chat: ChatData) => void;
  onToggleCollapse: () => void;
  onChatTitleUpdated?: (chatId: string, newTitle: string) => void;
  /** P9: bump to re-fetch workspaces (e.g. a project was created mid-run). */
  refreshSignal?: number;
}

export default function Sidebar({
  activeWorkspaceId, activeChatId, activeChatTitle, collapsed,
  onSelectWorkspace, onSelectChat, onToggleCollapse, onChatTitleUpdated,
  refreshSignal = 0,
}: Props) {
  const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [expandedWs, setExpandedWs] = useState<Set<string>>(new Set());
  const [chatMap, setChatMap] = useState<Record<string, ChatSummary[]>>({});
  const [showAddWs, setShowAddWs] = useState(false);
  const [newWsPath, setNewWsPath] = useState('');
  const [newWsName, setNewWsName] = useState('');
  const [projectsOpen, setProjectsOpen] = useState(true);
  const [chatsOpen, setChatsOpen] = useState(true);
  // The coding workspace is project-first; keep the old chat view behind a
  // flag instead of rendering it (main chats live in the main workspace).
  const [showGeneralChats] = useState(false);
  const [editingGeneralChatId, setEditingGeneralChatId] = useState<string | null>(null);
  const [generalChatName, setGeneralChatName] = useState('');
  const [loading, setLoading] = useState(true);
  const [deleteConfirm, setDeleteConfirm] = useState<{
    isOpen: boolean;
    workspace: WorkspaceInfo | null;
    chatId: string | null;
  }>({ isOpen: false, workspace: null, chatId: null });
  const [deletingChatId, setDeletingChatId] = useState<string | null>(null);
  const [generalLimit, setGeneralLimit] = useState(15);

  // Split into General and Projects
  const generalWs = workspaces.find((w) => w.isGeneral);
  const projectWs = workspaces.filter((w) => !w.isGeneral && !w.isHouse && !w.isPersonal && !(w as { isDfd?: boolean }).isDfd);

  useEffect(() => {
    if (activeWorkspaceId && activeChatId && activeChatTitle) {
      setChatMap((prev) => {
        const chats = prev[activeWorkspaceId];
        if (!chats) return prev;
        const hasChange = chats.some((c) => c.id === activeChatId && c.title !== activeChatTitle);
        if (!hasChange) return prev;
        return {
          ...prev,
          [activeWorkspaceId]: chats.map((c) =>
            c.id === activeChatId ? { ...c, title: activeChatTitle } : c
          )
        };
      });
    }
  }, [activeChatId, activeWorkspaceId, activeChatTitle]);

  const handleLocalChatTitleUpdated = useCallback((workspaceId: string, chatId: string, newTitle: string) => {
    setChatMap((prev) => {
      const chats = prev[workspaceId];
      if (!chats) return prev;
      return {
        ...prev,
        [workspaceId]: chats.map((c) => c.id === chatId ? { ...c, title: newTitle } : c)
      };
    });
    if (onChatTitleUpdated) {
      onChatTitleUpdated(chatId, newTitle);
    }
  }, [onChatTitleUpdated]);

  const loadWorkspaces = useCallback(async () => {
    setLoading(true);
    const ws = await fetchWorkspaces();
    setWorkspaces(ws);
    setExpandedWs((prev) => {
      const next = new Set(prev);
      ws.forEach((w) => next.add(w.id));
      return next;
    });
    for (const w of ws) {
      const chats = await fetchChats(w.id);
      setChatMap((prev) => ({ ...prev, [w.id]: chats }));
    }
    setLoading(false);
    return ws;
  }, []);

  useEffect(() => {
    loadWorkspaces();
  }, [loadWorkspaces]);

  // P9: re-fetch when a run created a new project workspace.
  useEffect(() => {
    if (refreshSignal > 0) loadWorkspaces();
  }, [refreshSignal, loadWorkspaces]);

  const toggleWs = useCallback(async (ws: WorkspaceInfo) => {
    setExpandedWs((prev) => {
      const next = new Set(prev);
      if (next.has(ws.id)) next.delete(ws.id);
      else next.add(ws.id);
      return next;
    });
    onSelectWorkspace(ws);
    if (!chatMap[ws.id]) {
      const chats = await fetchChats(ws.id);
      setChatMap((prev) => ({ ...prev, [ws.id]: chats }));
    }
  }, [chatMap, onSelectWorkspace]);

  const handleAddWorkspace = async () => {
    setNewWsPath('');
    setNewWsName('');
    setShowAddWs(true);
  };

  const browseForWorkspace = async () => {
    try {
      const folderPath = await (window as any).antigravity?.selectFolder?.();
      if (!folderPath) return;
      setNewWsPath(folderPath);
      if (!newWsName) setNewWsName(folderPath.split('\\').pop() || folderPath.split('/').pop() || '');
    } catch {}
  };

  const handleCreateChat = async (ws: WorkspaceInfo) => {
    const chat = await createChat(ws.id, 'New Conversation');
    const chats = await fetchChats(ws.id);
    setChatMap((prev) => ({ ...prev, [ws.id]: chats }));
    const fullChat = await fetchChat(ws.id, chat.id);
    onSelectChat(ws, fullChat);
  };

  const handleDeleteChat = useCallback((e: React.MouseEvent, ws: WorkspaceInfo, chatId: string) => {
    e.stopPropagation();
    setDeleteConfirm({ isOpen: true, workspace: ws, chatId });
  }, []);

  const confirmDeleteChat = async () => {
    const { workspace, chatId } = deleteConfirm;
    if (!workspace || !chatId) {
      setDeleteConfirm({ isOpen: false, workspace: null, chatId: null });
      return;
    }
    setDeleteConfirm({ isOpen: false, workspace: null, chatId: null });
    setDeletingChatId(chatId);
    await new Promise((r) => setTimeout(r, 220));
    try {
      await deleteChat(workspace.id, chatId);
      setChatMap((prev) => ({
        ...prev,
        [workspace.id]: (prev[workspace.id] ?? []).filter((c) => c.id !== chatId),
      }));
      if (activeChatId === chatId && activeWorkspaceId === workspace.id) {
        const remaining = (chatMap[workspace.id] ?? []).filter((c) => c.id !== chatId);
        const nextId = remaining[0]?.id;
        if (nextId) {
          const next = await fetchChat(workspace.id, nextId).catch(() => null);
          if (next) onSelectChat(workspace, next);
        }
      }
    } catch (err) {
      console.error('[Sidebar] deleteChat failed:', err);
      const chats = await fetchChats(workspace.id).catch(() => []);
      setChatMap((prev) => ({ ...prev, [workspace.id]: chats }));
    } finally {
      setDeletingChatId(null);
    }
  };

  const handleSelectChat = async (ws: WorkspaceInfo, chatId: string) => {
    const chat = await fetchChat(ws.id, chatId);
    onSelectChat(ws, chat);
  };

  const relativeTime = (iso: string) => {
    if (!iso) return '';
    const diff = Date.now() - new Date(iso).getTime();
    const mins = Math.floor(diff / 60000);
    if (mins < 1) return 'now';
    if (mins < 60) return `${mins}m`;
    const hrs = Math.floor(mins / 60);
    if (hrs < 24) return `${hrs}h`;
    return `${Math.floor(hrs / 24)}d`;
  };

  const submitAddWs = async () => {
    if (!newWsPath.trim()) return;
    await addWorkspace(newWsPath.trim(), newWsName.trim());
    setNewWsPath('');
    setNewWsName('');
    setShowAddWs(false);
    await loadWorkspaces();
  };

  // Live indicator — which chats currently have an agent streaming
  const [liveChatIds, setLiveChatIds] = useState<Set<string>>(new Set());
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      if (document.hidden) return;
      try {
        const res = await fetch(apiUrl('/api/runs/active'));
        const data = await res.json();
        const ids = new Set<string>();
        for (const r of (Array.isArray((data as any)?.runs) ? (data as any).runs : data?.running ? [data] : [])) {
          if (r.chat_id) ids.add(String(r.chat_id));
        }
        if (!cancelled) setLiveChatIds(ids);
      } catch {}
    };
    poll();
    const id = window.setInterval(poll, 2500);
    const onVis = () => { if (!document.hidden) poll(); };
    document.addEventListener('visibilitychange', onVis);
    return () => { cancelled = true; window.clearInterval(id); document.removeEventListener('visibilitychange', onVis); };
  }, []);

  const saveGeneralChatName = async (chat: ChatSummary) => {
    const title = generalChatName.trim();
    if (!title || !generalWs) { setEditingGeneralChatId(null); return; }
    const prevTitle = chat.title || '';
    // Optimistic: show new title instantly
    handleLocalChatTitleUpdated(generalWs.id, chat.id, title);
    setEditingGeneralChatId(null);
    try {
      await updateChatTitle(generalWs.id, chat.id, title);
    } catch (err) {
      console.error('[Sidebar] updateChatTitle failed:', err);
      handleLocalChatTitleUpdated(generalWs.id, chat.id, prevTitle);
    }
    setEditingGeneralChatId(null);
  };

  return (
    <aside
      className="sidebar-shell flex flex-col flex-shrink-0 select-none overflow-hidden transition-all duration-300 ease-in-out"
      style={{
        width: collapsed ? 56 : 248,
        backgroundColor: '#151518',
        borderColor: 'rgba(63, 63, 70, 0.45)'
      }}
    >
      {collapsed ? (
        <div className="flex flex-col items-center py-4 gap-4 h-full">
          <button onClick={onToggleCollapse} className="w-10 h-10 flex items-center justify-center text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800/60 rounded-xl transition-all cursor-pointer">
            <PanelLeftClose size={16} />
          </button>
          <button onClick={handleAddWorkspace} className="w-10 h-10 flex items-center justify-center text-zinc-400 hover:text-zinc-200 hover:bg-zinc-800/60 rounded-xl transition-all cursor-pointer" title="Add Folder">
            <FolderPlus size={16} />
          </button>
          <div className="flex-1" />
        </div>
      ) : (
        <div className="flex flex-col h-full overflow-hidden">
          {/* The coding workspace is project-first: main chats live in the main workspace. */}
          {showGeneralChats && (
          <>
          {/* ── General Chat Section (always expanded) ── */}
          <div className="group flex items-center justify-between" style={{ padding: '18px 14px 8px' }}>
            <button onClick={() => setChatsOpen((open) => !open)} className="flex items-center gap-1 text-[11px] font-bold text-zinc-500 uppercase tracking-widest hover:text-zinc-300 transition-colors" title={chatsOpen ? 'Collapse chats' : 'Expand chats'}>
              <span>Chats</span><ChevronDown size={14} className={`transition-transform duration-200 ${chatsOpen ? '' : '-rotate-90'}`} />
            </button>
            <div className="flex items-center opacity-0 group-hover:opacity-100 transition-opacity"><button onClick={() => generalWs && handleCreateChat(generalWs)} className="p-1 text-zinc-500 hover:text-zinc-200" title="New chat"><Plus size={15}/></button><button onClick={onToggleCollapse} className="p-1 text-zinc-500 hover:text-zinc-200" title="Close Sidebar"><PanelLeftClose size={15}/></button></div>
          </div>

          {/* General item — bounded height so 1000 chats scroll instead of pushing the layout */}
          {generalWs && (
            <div className={`sidebar-section-collapse ${chatsOpen ? 'is-open' : ''}`}>
            <div>
              <div className="mx-2 mb-4 max-h-[45vh] overflow-y-auto house-thread min-h-0">
                <div className="mt-1">
                {chatMap['_general']?.length === 0 && (
                  <div
                    className="text-[11px] text-zinc-600 italic cursor-pointer hover:text-zinc-400 transition-colors"
                    style={{ padding: '6px 8px 6px 12px' }}
                    onClick={() => handleCreateChat(generalWs)}
                  >
                    No conversations yet
                  </div>
                )}
                {(() => {
                  const chats = chatMap['_general'] || [];
                  // Build parent -> children map for grouped view. Same-ws
                  // children nest directly; cross-workspace spawn children
                  // (orchestrator → project agents) nest here too so the
                  // hierarchy reads in one place.
                  const byId = new Map(chats.map((c) => [c.id, c] as const));
                  const childrenByParent = new Map<string, typeof chats>();
                  const tops: typeof chats = [];
                  for (const c of chats) {
                    const p = (c as any).parentChatId as string | undefined;
                    if (p && byId.has(p)) {
                      const arr = childrenByParent.get(p) || [];
                      arr.push(c);
                      childrenByParent.set(p, arr);
                    } else {
                      tops.push(c);
                    }
                  }
                  // Cross-workspace spawned children of each general parent.
                  const wsList = workspaces;
                  const crossKids = new Map<string, { ws: WorkspaceInfo; chat: ChatSummary }[]>();
                  for (const w of wsList) {
                    if (w.id === '_general') continue;
                    for (const c of chatMap[w.id] ?? []) {
                      const p = (c as any).parentChatId as string | undefined;
                      if (p && byId.has(p)) {
                        const arr = crossKids.get(p) || [];
                        arr.push({ ws: w, chat: c });
                        crossKids.set(p, arr);
                      }
                    }
                  }
                  const Row = ({ chat, indented, wsOverride, fromTitle }: { chat: typeof chats[number]; indented?: boolean; wsOverride?: WorkspaceInfo; fromTitle?: string }) => {
                    const wsForRow = wsOverride ?? generalWs;
                    const isChatActive = chat.id === activeChatId && activeWorkspaceId === wsForRow.id;
                    const isEditing = !wsOverride && editingGeneralChatId === chat.id;
                    const isLive = liveChatIds.has(chat.id);
                    const isDeleting = chat.id === deletingChatId;
                    const spawned = isSpawnedChat(chat) || !!wsOverride;
                    const RowIcon = spawned ? Bot : MessageSquare;
                    return (
                      <div
                        key={`${wsForRow.id}:${chat.id}`}
                        draggable={!isDeleting}
                        onDragStart={(e) => {
                          e.dataTransfer.setData('text/plain', `${wsForRow.id}:${chat.id}`);
                          e.dataTransfer.effectAllowed = 'move';
                        }}
                        onClick={() => !isDeleting && handleSelectChat(wsForRow, chat.id as string)}
                        className={`chat-row relative flex items-center gap-1.5 w-full text-[12px] rounded-md transition-colors cursor-pointer group ${
                          isChatActive ? 'bg-zinc-800 text-zinc-100' : 'text-zinc-400 hover:bg-zinc-800/40 hover:text-zinc-200'
                        } ${indented ? 'ml-3 pl-2 border-l border-zinc-800/60' : ''} ${isDeleting ? 'is-deleting' : ''} ${spawned ? 'bg-violet-400/[0.05]' : ''}`}
                        style={{ padding: indented ? '6px 8px' : '8px 9px' }}
                        title={spawned ? `Auto-spawned${fromTitle ? ` from “${fromTitle}”` : wsOverride ? ` · ${wsOverride.name}` : ''}` : chat.title}
                      >
                        {isLive
                          ? <span className="w-2 h-2 rounded-full bg-white animate-pulse flex-shrink-0" title="Stark is working in this chat" />
                          : indented
                            ? <CornerDownRight size={10} className="flex-shrink-0 text-zinc-600" />
                            : <RowIcon size={indented ? 10 : 11} className={`flex-shrink-0 ${spawned ? 'text-violet-300/80' : indented ? 'text-zinc-500' : 'text-zinc-600'}`} />}
                        {isEditing ? <input value={generalChatName} autoFocus onClick={(event) => event.stopPropagation()} onChange={(event) => setGeneralChatName(event.target.value)} onBlur={() => saveGeneralChatName(chat)} onKeyDown={(event) => { if (event.key === 'Enter') saveGeneralChatName(chat); if (event.key === 'Escape') setEditingGeneralChatId(null); }} className="min-w-0 flex-1 rounded border border-zinc-600 bg-zinc-900 px-1 py-0.5 text-[11px] text-zinc-100 outline-none" /> : (
                          <span className="truncate flex-1">
                            <span className="block truncate">{chat.title || 'Untitled'}</span>
                            {spawned && (
                              <span className="mt-0.5 flex items-center gap-1">
                                <span className="inline-flex items-center rounded-full border border-violet-400/20 bg-violet-400/[0.08] px-1.5 py-px text-[9px] font-medium text-violet-200/80">auto</span>
                                <span className="truncate text-[10px] text-zinc-600">{fromTitle ? `from ${fromTitle}` : wsOverride ? wsOverride.name : 'spawned'}</span>
                              </span>
                            )}
                          </span>
                        )}
                        {!isEditing && <><span className="text-[10px] text-zinc-600 flex-shrink-0 ml-1 tabular-nums group-hover:opacity-0">{relativeTime(chat.updatedAt)}</span><div className="absolute right-1 hidden items-center gap-0.5 rounded bg-zinc-800 group-hover:flex"><button onClick={(event) => { event.stopPropagation(); setGeneralChatName(chat.title || ''); setEditingGeneralChatId(chat.id); }} className="p-1 text-zinc-400 hover:text-white" title="Rename"><Pencil size={11}/></button><button onClick={(event) => handleDeleteChat(event, wsForRow, chat.id)} className="p-1 text-zinc-400 hover:text-white" title="Delete"><Trash2 size={11}/></button></div></>}
                      </div>
                    );
                  };
                  const visibleTops = tops.slice(0, generalLimit);
                  const remainingTops = tops.length - visibleTops.length;
                  return (
                    <>
                      {visibleTops.map((top) => {
                        const kids = childrenByParent.get(top.id) || [];
                        const xkids = crossKids.get(top.id) || [];
                        if (!kids.length && !xkids.length) return <Row key={top.id} chat={top} />;
                        return (
                          <div key={top.id} className="mb-1">
                            <Row chat={top} />
                            <div className="mt-0.5 flex flex-col gap-0.5">
                              {kids.map((k) => <Row key={k.id} chat={k} indented fromTitle={top.title} />)}
                              {xkids.map(({ ws, chat }) => <Row key={`${ws.id}:${chat.id}`} chat={chat} indented wsOverride={ws} fromTitle={top.title} />)}
                            </div>
                          </div>
                        );
                      })}
                      {remainingTops > 0 && (
                        <button
                          type="button"
                          onClick={() => setGeneralLimit((n) => n + 15)}
                          className="w-full mb-1.5 py-1.5 rounded-lg text-[11px] font-medium text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.04] transition-colors cursor-pointer"
                        >
                          Load more ({remainingTops})
                        </button>
                      )}
                    </>
                  );
                })()}
              </div>
            </div>
            </div></div>
          )}

          </>
          )}

          {/* ── Separator ── */}
          <div style={{ borderTop: '1px solid rgba(63, 63, 70, 0.25)', margin: chatsOpen ? '4px 14px 14px' : '6px 14px 14px' }} />

          {/* ── Projects Section ── */}
          <div className="project-heading group flex items-center justify-between mb-2" style={{ padding: '0 14px' }}>
            <button onClick={() => setProjectsOpen((open) => !open)} className="flex items-center gap-1 text-[11px] font-bold text-zinc-500 uppercase tracking-widest hover:text-zinc-300 transition-colors" title={projectsOpen ? 'Collapse projects' : 'Expand projects'}>
              <span>Projects</span><ChevronDown size={14} className={`transition-transform duration-200 ${projectsOpen ? '' : '-rotate-90'}`} />
            </button>
            <div className="flex items-center opacity-0 group-hover:opacity-100 transition-opacity"><button className="p-1 text-zinc-500 hover:text-zinc-200" title="Project actions"><MoreHorizontal size={15}/></button><button onClick={handleAddWorkspace} className="p-1 text-zinc-500 hover:text-zinc-200" title="Create project"><Plus size={15}/></button></div>
          </div>
          <div className={`sidebar-section-collapse ${projectsOpen ? 'is-open' : ''}`}><SidebarProjectList
              workspaces={projectWs}
              expandedWs={expandedWs}
              chatMap={chatMap}
              activeWorkspaceId={activeWorkspaceId}
              activeChatId={activeChatId}
              loading={loading}
              toggleWs={toggleWs}
              handleCreateChat={handleCreateChat}
              removeWorkspace={removeWorkspace}
              loadWorkspaces={loadWorkspaces}
              handleDeleteChat={handleDeleteChat}
              handleSelectChat={handleSelectChat}
              relativeTime={relativeTime}
              handleAddWorkspace={handleAddWorkspace}
              onChatTitleUpdated={handleLocalChatTitleUpdated}
              liveChatIds={liveChatIds}
              deletingChatId={deletingChatId}
            /></div>

        </div>
      )}

      <SidebarAddFolderModal
        isOpen={showAddWs}
        value={newWsPath}
        name={newWsName}
        onChange={setNewWsPath}
        onNameChange={setNewWsName}
        onBrowse={browseForWorkspace}
        onClose={() => setShowAddWs(false)}
        onSubmit={submitAddWs}
      />

      {deleteConfirm.isOpen && createPortal(
        <div
          className="fixed inset-0 z-[200] flex items-center justify-center bg-black/70 backdrop-blur-md"
          onClick={() => setDeleteConfirm({ isOpen: false, workspace: null, chatId: null })}
        >
          <div
            className="bg-zinc-950 border border-white/[0.12] shadow-2xl w-full max-w-md mx-4"
            style={{ borderRadius: '18px', padding: '36px' }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-center w-12 h-12 rounded-2xl bg-red-500/10 border border-red-500/25 mb-6">
              <Trash2 size={19} className="text-red-400" />
            </div>
            <h3 className="text-[17px] font-bold tracking-tight text-zinc-100 mb-3">Delete Conversation</h3>
            <p className="text-[13px] text-zinc-400 leading-6 mb-7">
              Are you sure you want to delete this conversation? This action cannot be undone.
            </p>
            <div className="flex items-center justify-end gap-3 border-t border-white/[0.08] pt-5">
              <button
                onClick={() => setDeleteConfirm({ isOpen: false, workspace: null, chatId: null })}
                className="min-w-[84px] px-4 py-2.5 text-[13px] font-medium text-zinc-300 hover:text-white bg-zinc-900 border border-white/[0.10] hover:bg-zinc-800 transition-all rounded-lg cursor-pointer"
              >
                Cancel
              </button>
              <button
                onClick={confirmDeleteChat}
                className="min-w-[84px] px-4 py-2.5 text-[13px] font-semibold text-white bg-red-600 hover:bg-red-500 transition-all rounded-lg cursor-pointer shadow-[0_8px_20px_rgba(220,38,38,.18)]"
              >
                Delete
              </button>
            </div>
          </div>
        </div>,
        document.body
      )}
    </aside>
  );
}
