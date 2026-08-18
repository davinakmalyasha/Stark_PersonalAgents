import { useState, useEffect, useCallback } from 'react';
import { createPortal } from 'react-dom';
import { MessageSquare, Plus, Trash2, Pencil, MoreHorizontal, ChevronDown } from 'lucide-react';
import { PanelLeftClose, FolderPlus } from 'lucide-react';
import { fetchWorkspaces, addWorkspace, removeWorkspace, fetchChats, createChat, deleteChat, fetchChat, updateChatTitle } from '../../services/api';
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
  const [editingGeneralChatId, setEditingGeneralChatId] = useState<string | null>(null);
  const [generalChatName, setGeneralChatName] = useState('');
  const [loading, setLoading] = useState(true);
  const [deleteConfirm, setDeleteConfirm] = useState<{
    isOpen: boolean;
    workspace: WorkspaceInfo | null;
    chatId: string | null;
  }>({ isOpen: false, workspace: null, chatId: null });

  // Split into General and Projects
  const generalWs = workspaces.find((w) => w.isGeneral);
  const projectWs = workspaces.filter((w) => !w.isGeneral);

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
    if (workspace && chatId) {
      await deleteChat(workspace.id, chatId);
      const chats = await fetchChats(workspace.id);
      setChatMap((prev) => ({ ...prev, [workspace.id]: chats }));
    }
    setDeleteConfirm({ isOpen: false, workspace: null, chatId: null });
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

  const saveGeneralChatName = async (chat: ChatSummary) => {
    const title = generalChatName.trim();
    if (title && generalWs) {
      await updateChatTitle(generalWs.id, chat.id, title);
      handleLocalChatTitleUpdated(generalWs.id, chat.id, title);
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
          {/* ── General Chat Section (always expanded) ── */}
          <div className="group flex items-center justify-between" style={{ padding: '18px 14px 8px' }}>
            <button onClick={() => setChatsOpen((open) => !open)} className="flex items-center gap-1 text-[11px] font-bold text-zinc-500 uppercase tracking-widest hover:text-zinc-300 transition-colors" title={chatsOpen ? 'Collapse chats' : 'Expand chats'}>
              <span>Chats</span><ChevronDown size={14} className={`transition-transform duration-200 ${chatsOpen ? '' : '-rotate-90'}`} />
            </button>
            <div className="flex items-center opacity-0 group-hover:opacity-100 transition-opacity"><button onClick={() => generalWs && handleCreateChat(generalWs)} className="p-1 text-zinc-500 hover:text-zinc-200" title="New chat"><Plus size={15}/></button><button onClick={onToggleCollapse} className="p-1 text-zinc-500 hover:text-zinc-200" title="Close Sidebar"><PanelLeftClose size={15}/></button></div>
          </div>

          {/* General item */}
          {generalWs && (
            <div className={`sidebar-section-collapse ${chatsOpen ? 'is-open' : ''}`}>
            <div className="mx-2 mb-4">
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
                {chatMap['_general']?.map((chat) => {
                  const isChatActive = chat.id === activeChatId && activeWorkspaceId === '_general';
                  const isEditing = editingGeneralChatId === chat.id;
                  return (
                    <div
                      key={chat.id}
                      onClick={() => handleSelectChat(generalWs, chat.id)}
                      className={`chat-row relative flex items-center gap-1.5 w-full text-[12px] rounded-md transition-colors cursor-pointer group ${
                        isChatActive ? 'bg-zinc-800 text-zinc-100' : 'text-zinc-400 hover:bg-zinc-800/40 hover:text-zinc-200'
                      }`}
                      style={{ padding: '8px 9px' }}
                    >
                      <MessageSquare size={11} className="flex-shrink-0 text-zinc-600" />
                      {isEditing ? <input value={generalChatName} autoFocus onClick={(event) => event.stopPropagation()} onChange={(event) => setGeneralChatName(event.target.value)} onBlur={() => saveGeneralChatName(chat)} onKeyDown={(event) => { if (event.key === 'Enter') saveGeneralChatName(chat); if (event.key === 'Escape') setEditingGeneralChatId(null); }} className="min-w-0 flex-1 rounded border border-zinc-600 bg-zinc-900 px-1 py-0.5 text-[11px] text-zinc-100 outline-none" /> : <span className="truncate flex-1">{chat.title || 'Untitled'}</span>}
                      {!isEditing && <><span className="text-[10px] text-zinc-600 flex-shrink-0 ml-1 tabular-nums group-hover:opacity-0">{relativeTime(chat.updatedAt)}</span><div className="absolute right-1 hidden items-center gap-0.5 rounded bg-zinc-800 group-hover:flex"><button onClick={(event) => { event.stopPropagation(); setGeneralChatName(chat.title || ''); setEditingGeneralChatId(chat.id); }} className="p-1 text-zinc-400 hover:text-white" title="Rename"><Pencil size={11}/></button><button onClick={(event) => handleDeleteChat(event, generalWs, chat.id)} className="p-1 text-zinc-400 hover:text-white" title="Delete"><Trash2 size={11}/></button></div></>}
                    </div>
                  );
                })}
              </div>
            </div></div>
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
