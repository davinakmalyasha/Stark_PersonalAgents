import type { MutableRefObject } from 'react';
import HouseChatMessages from './HouseChatMessages';
import PromptDock from '../components/Workspace/PromptDock';
import type { ChatMessage, ChatSummary } from '../types';
import '../house/house.css';

interface Props {
  chats: ChatSummary[];
  activeChatId: string | null;
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
}

export default function HouseChat({
  chats, activeChatId, messages, isRunning, isLoading, notification, threadRef,
  isVoiceMode,
  totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens,
  onSend, onCancel, onToggleVoice, onOpenStats,
}: Props) {
  const activeTitle = chats.find((c) => c.id === activeChatId)?.title;

  return (
    <div className="flex flex-col h-full min-h-0 w-full max-w-[1400px] mx-auto px-4 sm:px-8 lg:px-12">
      {/* ── Slim top bar: active conversation ── */}
      <div className="flex items-center gap-2.5 pb-3 pt-3.5 select-none shrink-0">
        <span className="text-[13px] font-medium text-zinc-300 truncate">
          {activeTitle || 'New conversation'}
        </span>
      </div>

      <HouseChatMessages
        threadRef={threadRef}
        isLoading={isLoading}
        messages={messages}
      />

      {notification && (
        <div className="py-1.5 text-[11px] text-zinc-400 bg-white/[0.05] border border-white/10 rounded px-3 mt-1 mb-2">
          {notification}
        </div>
      )}

      {/* ── Prompt dock (shared with the Coding Workspace) ── */}
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
}
