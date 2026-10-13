import { Folder, ChevronRight, ChevronDown, Plus, Trash2, Pencil, Bot, CornerDownRight } from 'lucide-react';
import type { WorkspaceInfo, ChatSummary } from '../../types';
import { useState, useRef, useEffect } from 'react';
import { updateChatTitle } from '../../services/api';
import { isSpawnedChat } from '../../utils/chatHierarchy';

interface Props {
  workspaces: WorkspaceInfo[];
  expandedWs: Set<string>;
  chatMap: Record<string, ChatSummary[]>;
  activeWorkspaceId: string | null;
  activeChatId: string | null;
  loading?: boolean;
  toggleWs: (ws: WorkspaceInfo) => void;
  handleCreateChat: (ws: WorkspaceInfo) => void;
  removeWorkspace: (id: string) => Promise<any>;
  loadWorkspaces: () => Promise<any>;
  handleDeleteChat: (e: React.MouseEvent, ws: WorkspaceInfo, chatId: string) => void;
  handleSelectChat: (ws: WorkspaceInfo, chatId: string) => void;
  relativeTime: (iso: string) => string;
  handleAddWorkspace: () => void;
  onChatTitleUpdated: (workspaceId: string, chatId: string, newTitle: string) => void;
  liveChatIds?: Set<string>;
  deletingChatId?: string | null;
}

function SkeletonRow({ indent }: { indent?: boolean }) {
  return (
    <div className={`flex items-center gap-1.5 w-full animate-pulse ${indent ? 'pl-[22px]' : ''}`}
      style={{ padding: '7px 8px' }}>
      <div className="w-3 h-3 rounded bg-zinc-700/60 flex-shrink-0" />
      <div className="h-3 rounded bg-zinc-700/60 flex-1" style={{ maxWidth: '70%' }} />
    </div>
  );
}

export default function SidebarProjectList({
  workspaces, expandedWs, chatMap, activeWorkspaceId, activeChatId, loading,
  toggleWs, handleCreateChat, removeWorkspace, loadWorkspaces,
  handleDeleteChat, handleSelectChat, relativeTime, handleAddWorkspace,
  onChatTitleUpdated, liveChatIds, deletingChatId
}: Props) {
  const [editingChatId, setEditingChatId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');
  const [projectLimits, setProjectLimits] = useState<Record<string, number>>({});
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (editingChatId && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [editingChatId]);

  const startEditing = (e: React.MouseEvent, chatId: string, currentTitle: string) => {
    e.stopPropagation();
    setEditingChatId(chatId);
    setEditTitle(currentTitle);
  };

  const handleSaveTitle = async (ws: WorkspaceInfo, chatId: string) => {
    const trimmed = editTitle.trim();
    if (!trimmed) {
      setEditingChatId(null);
      return;
    }
    const prev = (chatMap[ws.id] ?? []).find((c) => c.id === chatId)?.title || '';
    // Optimistic: show new title instantly
    onChatTitleUpdated(ws.id, chatId, trimmed);
    setEditingChatId(null);
    try {
      await updateChatTitle(ws.id, chatId, trimmed);
    } catch (err) {
      console.error('Failed to update chat title:', err);
      onChatTitleUpdated(ws.id, chatId, prev);
    }
  };
  return (
    <div className="flex-1 overflow-y-auto" style={{ padding: '0 8px 10px' }}>
      {loading && (
        <div className="flex flex-col gap-1">
          <SkeletonRow />
          <SkeletonRow indent />
          <SkeletonRow indent />
          <SkeletonRow />
          <SkeletonRow indent />
        </div>
      )}
      {!loading && workspaces.map((ws) => {
        const isExpanded = expandedWs.has(ws.id);
        const isActive = ws.id === activeWorkspaceId;
        const chats = chatMap[ws.id] || [];

        return (
          <div key={ws.id} className="mb-1">
            <div className="flex items-center gap-1 group">
              <button
                onClick={() => toggleWs(ws)}
                className={`flex items-center gap-1.5 flex-1 text-[12px] rounded-md transition-colors text-left min-w-0 ${
                  isActive
                    ? 'text-zinc-200 font-semibold hover:bg-zinc-800/50'
                    : 'text-zinc-400 hover:bg-zinc-800/50 hover:text-zinc-200'
                }`}
                style={{ padding: '7px 8px' }}
              >
                {isExpanded ? (
                  <ChevronDown size={12} className="text-zinc-500 flex-shrink-0" />
                ) : (
                  <ChevronRight size={12} className="text-zinc-500 flex-shrink-0" />
                )}
                <span className="truncate font-medium">{ws.name}</span>
                <span className="text-[10px] text-zinc-600 ml-auto flex-shrink-0">{chats.length}</span>
              </button>
              <div className="flex items-center gap-1 pr-1">
                <button
                  onClick={(e) => { e.stopPropagation(); handleCreateChat(ws); }}
                  className="p-2 text-zinc-400 hover:text-zinc-100 hover:bg-zinc-800 opacity-0 group-hover:opacity-100 transition-all rounded-md cursor-pointer flex items-center justify-center"
                  title="New conversation"
                >
                  <Plus size={15} />
                </button>
                <button
                  onClick={(e) => { e.stopPropagation(); removeWorkspace(ws.id).then(loadWorkspaces); }}
                  className="p-2 text-zinc-400 hover:text-red-400 hover:bg-zinc-800 opacity-0 group-hover:opacity-100 transition-all rounded-md cursor-pointer flex items-center justify-center"
                  title="Remove project"
                >
                  <Trash2 size={14} />
                </button>
              </div>
            </div>

            {isExpanded && (
              <div style={{ marginLeft: '20px', marginTop: '2px', borderLeft: '1px solid rgba(63, 63, 70, 0.4)' }}>
                {chats.length === 0 ? (
                  <div
                    className="text-[11px] text-zinc-600 italic cursor-pointer hover:text-zinc-400 transition-colors"
                    style={{ padding: '8px 16px' }}
                    onClick={() => handleCreateChat(ws)}
                  >
                    No conversations yet
                  </div>
                ) : (() => {
                  // Group delegated children under their parent (same pattern as _general).
                  // Cross-workspace children show a "from X" badge resolved via the global map.
                  const byId = new Map(chats.map((c) => [c.id, c] as const));
                  const childrenByParent = new Map<string, typeof chats>();
                  const tops: typeof chats = [];
                  for (const c of chats) {
                    const p = c.parentChatId as string | undefined;
                    if (p && byId.has(p)) {
                      const arr = childrenByParent.get(p) || [];
                      arr.push(c);
                      childrenByParent.set(p, arr);
                    } else tops.push(c);
                  }
                  const titleById = new Map<string, string>();
                  for (const list of Object.values(chatMap)) {
                    for (const c of list) {
                      if (!titleById.has(c.id)) titleById.set(c.id, c.title || 'Untitled');
                    }
                  }
                  const ChatRow = ({ chat, indented }: { chat: typeof chats[number]; indented?: boolean }) => {
                    const isChatActive = chat.id === activeChatId && ws.id === activeWorkspaceId;
                    const isEditing = editingChatId === chat.id;
                    const isLive = !!liveChatIds?.has(chat.id);
                    const isDeleting = chat.id === deletingChatId;
                    const spawned = isSpawnedChat(chat);
                    const parentId = chat.parentChatId as string | undefined;
                    const parentTitle = parentId && !byId.has(parentId) ? titleById.get(parentId) : byId.get(parentId ?? '')?.title;
                    if (isEditing) {
                      return (
                        <div key={chat.id} className={`flex items-center gap-1.5 w-full text-[12px] rounded-r-md transition-colors text-left ${indented ? 'ml-3 border-l border-zinc-800/60' : ''}`} style={{ paddingLeft: indented ? '10px' : '14px', paddingRight: '8px', paddingTop: '6px', paddingBottom: '6px' }}>
                          <input ref={inputRef} value={editTitle} onChange={(e) => setEditTitle(e.target.value)} onBlur={() => handleSaveTitle(ws, chat.id)} onKeyDown={(e) => { if (e.key === 'Enter') handleSaveTitle(ws, chat.id); if (e.key === 'Escape') setEditingChatId(null); }} onClick={(e) => e.stopPropagation()} className="bg-zinc-900 border border-zinc-700/80 rounded px-1.5 py-0.5 text-[11px] text-zinc-100 focus:outline-none focus:border-blue-500/80 flex-1 min-w-0" autoFocus />
                        </div>
                      );
                    }
                    return (
                      <div key={chat.id} role="button" tabIndex={0} draggable={!isDeleting} onDragStart={(e) => { e.dataTransfer.setData('text/plain', `${ws.id}:${chat.id}`); e.dataTransfer.effectAllowed = 'move'; }} onClick={() => !isDeleting && handleSelectChat(ws, chat.id)} onKeyDown={(e) => { if (e.key === 'Enter') handleSelectChat(ws, chat.id); }} title={spawned ? `Auto-spawned${parentTitle ? ` from “${parentTitle}”` : ''}` : chat.title} className={`chat-row flex items-center gap-1.5 w-full text-[12px] rounded-r-md transition-colors text-left group cursor-pointer ${isChatActive ? 'bg-zinc-800 text-zinc-100' : 'text-zinc-400 hover:bg-zinc-800/40 hover:text-zinc-200'} ${indented ? 'ml-3 border-l border-zinc-800/60' : ''} ${isDeleting ? 'is-deleting' : ''} ${spawned && !indented ? 'bg-violet-400/[0.05]' : ''}`} style={{ paddingLeft: indented ? '10px' : '14px', paddingRight: '8px', paddingTop: indented ? '6px' : '8px', paddingBottom: indented ? '6px' : '8px' }}>
                        {isLive ? <span className="w-1.5 h-1.5 rounded-full bg-white animate-pulse flex-shrink-0" title="Stark is working in this chat" /> : indented ? <CornerDownRight size={10} className="flex-shrink-0 text-zinc-600" /> : spawned ? <Bot size={11} className="flex-shrink-0 text-violet-300/80" /> : null}
                        <span className="truncate flex-1">
                          <span className="block truncate">{chat.title || 'Untitled'}</span>
                          {spawned && (
                            <span className="mt-0.5 flex items-center gap-1">
                              <span className="inline-flex items-center rounded-full border border-violet-400/20 bg-violet-400/[0.08] px-1.5 py-px text-[9px] font-medium text-violet-200/80">auto</span>
                              <span className="truncate text-[10px] text-zinc-600">{parentTitle ? `from ${parentTitle}` : 'spawned'}</span>
                            </span>
                          )}
                        </span>
                        <div className="relative flex items-center h-5">
                          <span className="text-[10px] text-zinc-600 flex-shrink-0 ml-1 tabular-nums group-hover:opacity-0 transition-opacity duration-200">{relativeTime(chat.updatedAt)}</span>
                          <div className="absolute right-0 flex items-center gap-1 opacity-0 group-hover:opacity-100 transition-opacity duration-200">
                            <button onClick={(e) => startEditing(e, chat.id, chat.title)} className="p-1.5 text-zinc-400 hover:text-zinc-100 hover:bg-zinc-700 transition-all rounded-md flex-shrink-0 cursor-pointer" title="Rename conversation"><Pencil size={12} /></button>
                            <button onClick={(e) => handleDeleteChat(e, ws, chat.id)} className="p-1.5 text-zinc-400 hover:text-red-400 hover:bg-zinc-700 transition-all rounded-md flex-shrink-0 cursor-pointer" title="Delete conversation"><Trash2 size={12} /></button>
                          </div>
                        </div>
                      </div>
                    );
                  };
                  const limit = projectLimits[ws.id] ?? 7;
                  const visibleTops = tops.slice(0, limit);
                  const remainingTops = tops.length - visibleTops.length;
                  return (
                    <>
                      {visibleTops.map((top) => {
                        const kids = childrenByParent.get(top.id) || [];
                        if (!kids.length) return <ChatRow key={top.id} chat={top} />;
                        return (
                          <div key={top.id}>
                            <ChatRow chat={top} />
                            <div className="mt-0.5 flex flex-col gap-0.5">{kids.map((k) => <ChatRow key={k.id} chat={k} indented />)}</div>
                          </div>
                        );
                      })}
                      {remainingTops > 0 && (
                        <button
                          type="button"
                          onClick={() => setProjectLimits((prev) => ({ ...prev, [ws.id]: (prev[ws.id] ?? 7) + 7 }))}
                          className="w-full mb-1.5 py-1.5 rounded-lg text-[11px] font-medium text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.04] transition-colors cursor-pointer"
                        >
                          Load more ({remainingTops})
                        </button>
                      )}
                    </>
                  );
                })()}
              </div>
            )}
          </div>
        );
      })}

      {workspaces.length === 0 && (
        <div className="px-3 py-6 text-[12px] text-zinc-600 italic text-center leading-relaxed">
          <Folder size={24} className="mx-auto mb-2 text-zinc-700" />
          No projects yet.
          <br />
          <button onClick={handleAddWorkspace} className="text-blue-400 hover:text-blue-300 mt-1.5 transition-colors">
            + Add your first folder
          </button>
        </div>
      )}
    </div>
  );
}
