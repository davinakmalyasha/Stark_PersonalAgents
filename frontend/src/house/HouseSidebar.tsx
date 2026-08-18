import { useRef, useState } from 'react';
import { Plus, MessageSquare, Pencil, Trash2, Check, Code2, Home, Lock, FolderOpen, MessagesSquare, ChevronRight } from 'lucide-react';
import type { ChatSummary } from '../types';

interface Props {
  open: boolean;
  onClose: () => void;
  chats: ChatSummary[];
  activeChatId: string | null;
  isRunning: boolean;
  workspaceCount: number | null;
  chatCount: number | null;
  onNewChat: () => void;
  onSelectChat: (chatId: string) => void;
  onRenameChat: (chatId: string, title: string) => void;
  onDeleteChat: (chatId: string) => void;
  onLaunchCoding: () => void;
}

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

export default function HouseSidebar({
  open, onClose, chats, activeChatId, isRunning,
  workspaceCount, chatCount,
  onNewChat, onSelectChat, onRenameChat, onDeleteChat, onLaunchCoding,
}: Props) {
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editTitle, setEditTitle] = useState('');
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null);
  const deleteTimer = useRef<number | null>(null);

  const startRename = (chat: ChatSummary) => {
    setConfirmDeleteId(null);
    setEditingId(chat.id);
    setEditTitle(chat.title || '');
  };

  const commitRename = () => {
    if (editingId && editTitle.trim()) onRenameChat(editingId, editTitle.trim());
    setEditingId(null);
  };

  const handleDelete = (chatId: string) => {
    if (confirmDeleteId === chatId) {
      onDeleteChat(chatId);
      setConfirmDeleteId(null);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
    } else {
      setConfirmDeleteId(chatId);
      if (deleteTimer.current) window.clearTimeout(deleteTimer.current);
      deleteTimer.current = window.setTimeout(() => {
        setConfirmDeleteId((cur) => (cur === chatId ? null : cur));
      }, 2500);
    }
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
        aria-label="Chat history"
      >
        {/* ── New chat ── */}
        <div className="px-3.5 pt-3.5 pb-4 border-b house-hairline">
          <button
            onClick={onNewChat}
            disabled={isRunning}
            className="flex items-center justify-center gap-2 w-full py-2.5 rounded-xl bg-white text-[#0a0a0d] text-[13px] font-semibold hover:bg-zinc-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer"
          >
            <Plus size={15} />
            New chat
          </button>
        </div>

        {/* ── Chat history ── */}
        <div className="px-4 pt-4 pb-2 text-[10px] uppercase tracking-widest font-semibold text-zinc-600">
          Chats
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto house-thread px-3 pb-3.5">
          {chats.length === 0 ? (
            <p className="text-[11.5px] text-zinc-600 italic px-2.5 py-3">No conversations yet.</p>
          ) : (
            chats.map((chat) => {
              const isActive = chat.id === activeChatId;
              const isEditing = editingId === chat.id;
              const isConfirming = confirmDeleteId === chat.id;
              return (
                <div
                  key={chat.id}
                  onClick={() => { if (!isEditing && !isRunning) onSelectChat(chat.id); }}
                  className={`house-sidebar-row group relative flex items-center gap-3 rounded-lg mb-1.5 px-3.5 py-3 cursor-pointer ${
                    isActive ? 'is-active' : ''
                  } ${isRunning ? 'pointer-events-none opacity-60' : ''}`}
                >
                  {isEditing ? (
                    <input
                      autoFocus
                      value={editTitle}
                      onChange={(e) => setEditTitle(e.target.value)}
                      onBlur={commitRename}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter') commitRename();
                        if (e.key === 'Escape') setEditingId(null);
                      }}
                      onClick={(e) => e.stopPropagation()}
                      className="min-w-0 flex-1 rounded-md border border-white/25 bg-black/40 px-2 py-1 text-[12px] text-white outline-none focus:border-white/40"
                    />
                  ) : (
                    <>
                      <MessageSquare size={12} className="flex-shrink-0 text-zinc-600" />
                      <span className={`flex-1 truncate text-[12.5px] ${isActive ? 'text-white' : 'text-zinc-400'}`}>
                        {chat.title || 'Untitled'}
                      </span>
                      <span className="text-[10px] text-zinc-600 flex-shrink-0 tabular-nums">{relativeTime(chat.updatedAt)}</span>
                      <div className="absolute right-1 hidden group-hover:flex items-center gap-0.5 bg-[#18181d] rounded-md border border-white/[0.06] px-0.5 py-0.5">
                        <button
                          title={isConfirming ? 'Click again to confirm' : 'Delete'}
                          onClick={(e) => { e.stopPropagation(); handleDelete(chat.id); }}
                          className={`grid place-items-center w-6 h-6 rounded-md transition-colors ${
                            isConfirming ? 'text-red-400 bg-red-500/15' : 'text-zinc-500 hover:text-white hover:bg-white/[0.07]'
                          }`}
                        >
                          {isConfirming ? <Check size={11} /> : <Trash2 size={11} />}
                        </button>
                        <button
                          title="Rename"
                          onClick={(e) => { e.stopPropagation(); startRename(chat); }}
                          className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] transition-colors"
                        >
                          <Pencil size={11} />
                        </button>
                      </div>
                    </>
                  )}
                </div>
              );
            })
          )}
        </div>

        {/* ── Workspaces ── */}
        <div className="border-t house-hairline px-4 pt-3.5 pb-2 mt-2 text-[10px] uppercase tracking-widest font-semibold text-zinc-600">
          Workspaces
        </div>
        <div className="px-3 pb-3.5 flex flex-col gap-1.5">
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
          <div className="house-sidebar-row flex items-center gap-2.5 rounded-lg px-2.5 py-2.5 opacity-55">
            <span className="house-room-icon grid place-items-center w-8 h-8 rounded-lg text-zinc-400">
              <Home size={15} />
            </span>
            <div className="flex-1 min-w-0">
              <div className="text-[12.5px] font-medium text-zinc-300">House Workspace</div>
              <div className="text-[10px] text-zinc-500">Coming soon</div>
            </div>
            <Lock size={12} className="text-zinc-600" />
          </div>
        </div>

        {/* ── Stats ── */}
        <div className="border-t house-hairline px-4 py-3.5">
          <div className="flex items-center justify-between text-[11px] text-zinc-500">
            <span className="flex items-center gap-1.5">
              <FolderOpen size={11} className="text-zinc-600" />
              {workspaceCount ?? '—'} workspaces
            </span>
            <span className="flex items-center gap-1.5">
              <MessagesSquare size={11} className="text-zinc-600" />
              {chatCount ?? '—'} conversations
            </span>
          </div>
        </div>
      </aside>
    </>
  );
}
