import { Menu } from 'lucide-react';
import type { SessionTab } from '../types';
import SessionTabs from './SessionTabs';
import WindowControls from '../components/Layout/WindowControls';

interface HouseHeaderProps {
  tabs: SessionTab[];
  activeTabId: string | null;
  runningTabId: string | null;
  onActivateTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onNewSession: () => void;
  onToggleSidebar: () => void;
}

/**
 * Main workspace top bar: sidebar toggle + session tab strip.
 * Projects/tasks live in the side panel, settings in the icon rail —
 * the top bar is reserved for chat.
 */
export default function HouseHeader({
  tabs, activeTabId, runningTabId,
  onActivateTab, onCloseTab, onNewSession,
  onToggleSidebar,
}: HouseHeaderProps) {
  const hasDesktopBridge = typeof window !== 'undefined' && !!(window as unknown as { antigravity?: unknown }).antigravity;
  const isDesktopShell = hasDesktopBridge || Boolean((window as unknown as { __TAURI__?: unknown }).__TAURI__);

  return (
    <header
      className={`relative z-20 flex items-center gap-2 px-3 sm:px-4 ${isDesktopShell ? 'pr-36' : ''} h-[52px] border-b house-hairline shrink-0`}
      style={{ WebkitAppRegion: 'drag' } as React.CSSProperties}
    >
      <button
        onClick={onToggleSidebar}
        title="Toggle sidebar"
        aria-label="Toggle sidebar"
        className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer shrink-0"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
      >
        <Menu size={16} />
      </button>

      {/* ── Session tab strip (the top bar IS the chat switcher) ── */}
      <div className="flex-1 min-w-0 flex items-center" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <SessionTabs
          tabs={tabs}
          activeTabId={activeTabId}
          runningTabId={runningTabId}
          onActivateTab={onActivateTab}
          onCloseTab={onCloseTab}
          onNewSession={onNewSession}
        />
      </div>
      <WindowControls className="absolute right-0 top-0 bottom-0 z-30" />
    </header>
  );
}
