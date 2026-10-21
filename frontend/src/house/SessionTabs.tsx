import { Bot, Code2, MessageSquare, Plus, X } from 'lucide-react';
import type { SessionTab } from '../types';

interface SessionTabsProps {
  tabs: SessionTab[];
  activeTabId: string | null;
  runningTabId: string | null;
  onActivateTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  /** Instantly creates a new General chat session tab. */
  onNewSession: () => void;
}

/**
 * OpenCode-style session tab strip for the main workspace top bar.
 * One tab per open chat session — chat tabs (General) and coding tabs
 * (project chats, incl. ones the orchestrator auto-creates). Each tab
 * shows its type icon, title, a pulsing dot while its pipeline runs,
 * and a close button (closes the tab, never deletes the chat).
 */
export default function SessionTabs({
  tabs, activeTabId, runningTabId,
  onActivateTab, onCloseTab, onNewSession,
}: SessionTabsProps) {
  return (
    <div className="flex items-center gap-1 flex-1 min-w-0 overflow-x-auto no-scrollbar" data-testid="session-tabs">
      {tabs.map((tab) => {
        const isActive = tab.id === activeTabId;
        const isRunning = tab.id === runningTabId;
        const spawned = Boolean((tab as SessionTab).isSpawned || (tab as SessionTab).parentChatId);
        const Icon = spawned ? Bot : tab.type === 'coding' ? Code2 : MessageSquare;
        const tooltip = spawned
          ? `${tab.title} — spawned · ${tab.workspaceName}`
          : tab.type === 'coding' ? `${tab.title} — ${tab.workspaceName}` : tab.title;
        return (
          <div
            key={tab.id}
            role="tab"
            aria-selected={isActive}
            title={tooltip}
            onClick={() => onActivateTab(tab.id)}
            onAuxClick={(e) => { if (e.button === 1) { e.preventDefault(); onCloseTab(tab.id); } }}
            className={`group flex items-center gap-2 h-[30px] px-2.5 rounded-lg border shrink-0 max-w-[210px] cursor-pointer transition-all duration-150 select-none ${
              isActive
                ? 'bg-white/[0.10] border-white/[0.14] text-white shadow-[inset_0_1px_0_rgba(255,255,255,0.06)]'
                : 'bg-transparent border-transparent text-zinc-400 hover:bg-white/[0.05] hover:text-zinc-200'
            }`}
          >
            <Icon size={12} className={`shrink-0 ${isActive ? 'text-zinc-200' : 'text-zinc-500'}`} />
            <span className="truncate text-[12px] leading-none">{spawned ? `↳ ${tab.title}` : tab.title}</span>
            {isRunning && <span className="session-run-dot" title="Running" />}
            <button
              onClick={(e) => { e.stopPropagation(); onCloseTab(tab.id); }}
              title="Close session"
              aria-label={`Close ${tab.title}`}
              className={`grid place-items-center w-[18px] h-[18px] rounded-md shrink-0 transition-all ${
                isActive
                  ? 'text-zinc-500 hover:text-white hover:bg-white/[0.12]'
                  : 'text-zinc-600 opacity-0 group-hover:opacity-100 hover:text-white hover:bg-white/[0.10]'
              }`}
            >
              <X size={11} />
            </button>
          </div>
        );
      })}

      <button
        onClick={onNewSession}
        title="New session"
        aria-label="New session"
        className="flex items-center gap-1.5 h-[30px] px-2.5 rounded-lg border border-dashed border-white/[0.12] shrink-0 text-zinc-500 hover:text-zinc-200 hover:border-white/[0.28] hover:bg-white/[0.04] transition-all duration-150 cursor-pointer"
      >
        <Plus size={13} />
        <span className="text-[12px] leading-none hidden md:inline">New session</span>
      </button>
    </div>
  );
}
