import { useCallback, useEffect, useState } from 'react';
import WindowControls from '../components/Layout/WindowControls';
import { ArrowLeft, Menu, User } from 'lucide-react';
import HouseSidebar from '../house/HouseSidebar';
import { useSessions, tabId } from '../house/useSessions';
import { updateChatTitle, deleteChat } from '../services/api';
import '../house-workspace/houseControl.css';
import '../house/house.css';

/**
 * Personal Workspace — reserved shell for the upcoming personal tools.
 * Intentionally empty: the personal agent + integrations land here later
 * (git-pull and go). Navigation and routing already wired via #/personal.
 * The hamburger opens the shared room sidebar (search + all chats +
 * workspaces), same as the House Workspace.
 */
export default function PersonalWorkspace() {
  const [sidebarOpen, setSidebarOpen] = useState(false);

  // Chat index for the hamburger sidebar (same session layer as the main
  // workspace — routes are exclusive, so only one instance polls at a time).
  const sessions = useSessions();

  // Escape closes the sidebar
  useEffect(() => {
    if (!sidebarOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setSidebarOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [sidebarOpen]);

  const go = useCallback((hash: string) => {
    setSidebarOpen(false);
    window.location.hash = hash;
  }, []);

  const handleRenameChat = useCallback((workspaceId: string, chatId: string, title: string) => {
    updateChatTitle(workspaceId, chatId, title)
      .then(() => sessions.refreshSessions())
      .catch(() => {});
  }, [sessions]);

  const handleDeleteChat = useCallback((workspaceId: string, chatId: string) => {
    deleteChat(workspaceId, chatId)
      .then(() => {
        sessions.closeTab(tabId(workspaceId, chatId));
        return sessions.refreshSessions();
      })
      .catch(() => {});
  }, [sessions]);

  const openChatAtHome = useCallback((workspaceId: string, chatId: string) => {
    go(`#/?ws=${encodeURIComponent(workspaceId)}&chat=${encodeURIComponent(chatId)}`);
  }, [go]);

  return (
    <div className="hw-root fixed inset-0 flex flex-col overflow-hidden">
      {/* ── Header ── */}
      <header
        className="flex items-center justify-between px-4 sm:px-6 h-[52px] border-b house-hairline shrink-0"
        style={{ WebkitAppRegion: 'drag' } as React.CSSProperties}
      >
        <div className="flex items-center gap-2.5" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
          <button
            type="button"
            onClick={() => setSidebarOpen(true)}
            title="Chat history"
            aria-label="Open chat history"
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
          >
            <Menu size={16} />
          </button>
          <button
            type="button"
            onClick={() => { window.location.hash = ''; }}
            title="Back to home"
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="flex items-center gap-2.5">
            <div className="grid place-items-center w-7 h-7 rounded-lg bg-white text-[#0a0a0d]">
              <User size={14} />
            </div>
            <span className="house-font-display text-[15px] font-bold tracking-wide text-white">
              Personal Workspace
            </span>
          </div>
        </div>
        <WindowControls />
      </header>

      {/* ── Body: reserved ── */}
      <main className="flex-1 min-h-0 grid place-items-center">
        <div className="text-center px-6 select-none">
          <div className="grid place-items-center w-14 h-14 rounded-2xl hw-panel mx-auto mb-4 text-zinc-300">
            <User size={22} />
          </div>
          <div className="house-font-display text-[15px] font-semibold tracking-wide text-white">
            Personal agent
          </div>
          <p className="text-[12px] text-zinc-500 mt-2 max-w-xs leading-5">
            Reserved for your personal tools & life admin. This space activates
            when the personal integration lands.
          </p>
          <span className="inline-block mt-4 text-[10px] uppercase tracking-widest font-semibold text-zinc-600 border border-white/10 rounded-full px-3 py-1">
            coming soon
          </span>
        </div>
      </main>

      {/* Hamburger sidebar (room variant: search + all chats + workspaces) */}
      <HouseSidebar
        variant="room"
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        workspaces={sessions.workspaces}
        chatsByWs={sessions.chatsByWs}
        activeTabId={sessions.activeTabId}
        runningTabId={sessions.runningTabId}
        pinnedChats={sessions.pinnedChats}
        onTogglePin={sessions.togglePin}
        onOpenSession={openChatAtHome}
        onRenameChat={handleRenameChat}
        onDeleteChat={handleDeleteChat}
        onLaunchPersonal={() => go('#/personal')}
        onLaunchCoding={() => go('#/coding')}
        onLaunchHouse={() => go('#/house')}
      />
    </div>
  );
}
