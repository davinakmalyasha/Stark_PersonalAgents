import { useState } from 'react';
import { X, Minimize2, Maximize2 } from 'lucide-react';
import type { ChatMessage, AppState, ModelOption } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import PromptDock from './PromptDock';
import MessageStream from './MessageStream';

interface Props {
  messages: ChatMessage[];
  isRunning: boolean;
  workspaceName: string | null;
  chatTitle: string | null;
  selectedModel: string;
  models: ModelOption[];
  state: AppState | null;
  hasActiveChat: boolean;
  notification: string | null;
  onSend: (message: string) => void;
  onCancel: () => void;
  onModelChange: (model: string) => void;
  onClearNotification: () => void;
  isVoiceMode?: boolean;
  onToggleVoice: () => void;
  voiceInputText?: string;
  onOpenStats: () => void;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  totalCacheHitTokens: number;
  activeContextTokens: number;
  traceSteps?: TraceStep[];
  workspaceId: string | null;
  chatId: string | null;
}

export default function ChatView({
  messages, isRunning,
  hasActiveChat, notification,
  onSend, onCancel, onClearNotification,
  isVoiceMode, onToggleVoice, voiceInputText, onOpenStats, totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens, traceSteps = [], workspaceId, chatId,
}: Props) {
  const [minimized, setMinimized] = useState(false);
  const [activeSubagent, setActiveSubagent] = useState<string | null>(null);

  return (
    <div 
      className="absolute bottom-6 left-3 right-3 z-40 w-auto max-w-none flex flex-col pointer-events-auto rounded-2xl border border-zinc-800/80 backdrop-blur-xl shadow-2xl transition-all duration-300"
      style={{
        backgroundColor: 'rgba(18, 18, 20, 0.85)',
        maxHeight: '80vh',
      }}
    >
      {/* Floating minimize/expand button (icon only, proper bg color, bigger icon) */}
      <button
        onClick={() => setMinimized((m) => !m)}
        className="absolute top-3.5 right-4 z-50 p-2 bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 hover:border-zinc-700/80 rounded-xl text-zinc-400 hover:text-zinc-100 transition-all cursor-pointer shadow-xl flex items-center justify-center"
        title={minimized ? "Expand chat history" : "Collapse chat history"}
        style={{
          width: '32px',
          height: '32px',
        }}
      >
        {minimized ? (
          <Maximize2 size={16} className="text-indigo-400" />
        ) : (
          <Minimize2 size={16} />
        )}
      </button>

      {/* Collapsible Area */}
      <div 
        className="flex-1 flex flex-col overflow-y-auto transition-all duration-300 ease-in-out"
        style={{
          maxHeight: minimized ? '0px' : '480px',
          opacity: minimized ? 0 : 1,
        }}
      >
        {/* Notifications */}
        {notification && (
          <div className="flex items-center gap-2.5 mx-4 mt-2 px-3 py-2 rounded-lg bg-blue-500/10 border border-blue-500/20 text-[11px] text-blue-300 flex-shrink-0">
            <span className="flex-1 truncate">{notification}</span>
            <button onClick={onClearNotification} className="p-0.5 text-blue-400 hover:text-blue-200">
              <X size={12} />
            </button>
          </div>
        )}

        {/* Message stream */}
        <MessageStream
          messages={messages}
          hasActiveChat={hasActiveChat}
          onOpenSubagent={(name) => setActiveSubagent(name)}
          activeSubagent={activeSubagent}
          traceSteps={traceSteps}
          workspaceId={workspaceId}
          chatId={chatId}
          onCloseSubagent={() => setActiveSubagent(null)}
        />
      </div>

      {/* Always visible Bottom Prompt Dock */}
      <div className="border-t border-zinc-800/50 flex-shrink-0">
        <PromptDock onSend={onSend} onCancel={onCancel} isRunning={isRunning} isVoiceMode={isVoiceMode} onToggleVoice={onToggleVoice} voiceInputText={voiceInputText} onOpenStats={onOpenStats} totalCost={totalCost} totalInput={totalInput} totalOutput={totalOutput} totalCacheHitTokens={totalCacheHitTokens} activeContextTokens={activeContextTokens} />
      </div>
    </div>
  );
}
