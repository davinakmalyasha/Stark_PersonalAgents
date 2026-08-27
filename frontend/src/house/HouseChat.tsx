import { useState } from 'react';
import type { MutableRefObject } from 'react';
import { ChevronDown, ChevronUp, ExternalLink, X } from 'lucide-react';
import HouseChatMessages from './HouseChatMessages';
import PromptDock from '../components/Workspace/PromptDock';
import type { ChatMessage } from '../types';
import '../house/house.css';

interface Props {
  title: string;
  subtitle?: string;
  messages: ChatMessage[];
  isRunning: boolean;
  isLoading: boolean;
  notification: string | null;
  threadRef: MutableRefObject<HTMLDivElement | null>;
  isVoiceMode: boolean;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  totalCacheHitTokens: number;
  activeContextTokens: number;
  onSend: (prompt: string, files: File[]) => void;
  onCancel: () => void;
  onToggleVoice: () => void;
  onOpenStats: () => void;
  /** 'home' = solid card centered in the main workspace; 'ide' = floating card over the mini-IDE; 'ide-panel' = docked right panel (same card chrome as home, fills height). */
  variant?: 'home' | 'ide' | 'ide-panel';
  defaultMinimized?: boolean;
  /** Shown in the ide header: jump to the full Coding Workspace for this chat. */
  onOpenInWorkspace?: () => void;
  /** Docked panel close (ide-panel only): returns to the chat card view. */
  onCloseIde?: () => void;
  /** Drill-in for spawned subagents (blocks in the thread). */
  onOpenSubagent?: (childId: string, label: string) => void;
  /** Session ids — power the live running/done status of spawn blocks. */
  workspaceId?: string | null;
  chatId?: string | null;
}

/**
 * The floating conversation panel shared by every session tab: messages on
 * top (collapsible — exactly like the coding workspace's ChatView), prompt
 * dock always visible below.
 */
export default function HouseChat({
  title, subtitle,
  messages, isRunning, isLoading, notification, threadRef,
  isVoiceMode,
  totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens,
  onSend, onCancel, onToggleVoice, onOpenStats,
  variant = 'home',
  defaultMinimized = false,
  onOpenInWorkspace,
  onCloseIde,
  onOpenSubagent,
  workspaceId,
  chatId,
}: Props) {
  const [minimized, setMinimized] = useState(defaultMinimized);

  const header = (
    <div className="flex items-center gap-2 select-none shrink-0">
      <span className="text-[13px] font-medium text-zinc-300 truncate">{title}</span>
      {subtitle && <span className="text-[11px] text-zinc-600 truncate hidden sm:inline">· {subtitle}</span>}
      {isRunning && <span className="session-run-dot shrink-0" title="Running" />}
      <span className="flex-1" />
      {onOpenInWorkspace && (
        <button
          onClick={onOpenInWorkspace}
          title="Open this chat in the Coding Workspace"
          aria-label="Open in Coding Workspace"
          className="grid place-items-center w-7 h-7 rounded-lg text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
        >
          <ExternalLink size={13} />
        </button>
      )}
      {variant === 'ide-panel' && onCloseIde && (
        <button
          onClick={onCloseIde}
          title="Close IDE — back to chat"
          aria-label="Close IDE"
          className="grid place-items-center w-7 h-7 rounded-lg text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
        >
          <X size={14} />
        </button>
      )}
      <button
        onClick={() => setMinimized((m) => !m)}
        title={minimized ? 'Expand conversation' : 'Collapse conversation'}
        aria-label={minimized ? 'Expand conversation' : 'Collapse conversation'}
        className="grid place-items-center w-7 h-7 rounded-lg text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
      >
        {minimized ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
      </button>
    </div>
  );

  const messagesArea = (
    <div
      className="min-h-0 flex flex-col overflow-hidden"
      style={{
        maxHeight: minimized ? 0 : 2000,
        opacity: minimized ? 0 : 1,
        transition: 'max-height 300ms ease-in-out, opacity 250ms ease-in-out',
      }}
      aria-hidden={minimized}
    >
      <HouseChatMessages
        threadRef={threadRef}
        isLoading={isLoading}
        messages={messages}
        onOpenSubagent={onOpenSubagent}
        workspaceId={workspaceId}
        chatId={chatId}
      />
    </div>
  );

  const notificationBar = notification ? (
    <div className="py-1.5 px-3 mb-1 text-[11px] text-zinc-400 bg-white/[0.05] border border-white/10 rounded shrink-0 flex items-center gap-2">
      <span className="flex-1 truncate">{notification}</span>
    </div>
  ) : null;

  const dock = (
    <div className="shrink-0 pt-1">
      <PromptDock
        onSend={onSend}
        onCancel={onCancel}
        isRunning={isRunning}
        isVoiceMode={isVoiceMode}
        onToggleVoice={onToggleVoice}
        voiceInputText=""
        onOpenStats={onOpenStats}
        totalCost={totalCost}
        totalInput={totalInput}
        totalOutput={totalOutput}
        totalCacheHitTokens={totalCacheHitTokens}
        activeContextTokens={activeContextTokens}
      />
    </div>
  );

  if (variant === 'ide') {
    return (
      <div className="absolute bottom-3 right-3 z-30 w-[min(600px,calc(100vw-360px))] flex flex-col rounded-2xl border house-hairline bg-[#101014]/[0.92] backdrop-blur-xl shadow-2xl px-3 pt-2.5 pb-1 ide-panel-enter">
        {header}
        {messagesArea}
        {notificationBar}
        {dock}
      </div>
    );
  }

  if (variant === 'ide-panel') {
    return (
      <div className="flex flex-col min-h-0 h-full px-3 pt-2.5 pb-2 overflow-hidden">
        {header}
        {messagesArea}
        {notificationBar}
        {dock}
      </div>
    );
  }

  // Home: a solid dark-grey card that follows the chat — auto height hugging
  // header + messages + dock exactly like the ide floating panel (single
  // max-height animation, so the dock never gets displaced mid-transition),
  // bottom-anchored, capped at the workspace height.
  return (
    <div className="flex flex-col justify-end h-full min-h-0 w-full max-w-[1400px] mx-auto px-4 sm:px-8 lg:px-10 pt-2 pb-3">
      <div className="house-chat-card flex flex-col min-h-0 max-h-full rounded-2xl border house-hairline px-3 pt-2.5 pb-1 overflow-hidden">
        {header}
        {messagesArea}
        {notificationBar}
        {dock}
      </div>
    </div>
  );
}
