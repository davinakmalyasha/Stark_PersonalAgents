import type { MutableRefObject } from 'react';
import { Sparkles } from 'lucide-react';
import type { ChatMessage } from '../types';
import Markdown from '../components/Workspace/Markdown';
import { getAssistantName } from './assistant';

interface HouseChatMessagesProps {
  threadRef: MutableRefObject<HTMLDivElement | null>;
  isLoading: boolean;
  messages: ChatMessage[];
}

export default function HouseChatMessages({
  threadRef,
  isLoading,
  messages,
}: HouseChatMessagesProps) {
  const assistantName = getAssistantName();
  return (
    <div ref={threadRef} className="house-thread flex-1 min-h-[160px] overflow-y-auto px-1 pt-2 pb-4">
      {isLoading ? (
        <div className="h-full flex flex-col items-center justify-center gap-3 text-zinc-500 py-6">
          <div className="flex gap-1.5">
            <span className="house-typing-dot" />
            <span className="house-typing-dot" />
            <span className="house-typing-dot" />
          </div>
          <span className="text-[12px]">Connecting to {assistantName}...</span>
        </div>
      ) : messages.length === 0 ? (
        <div className="h-full flex flex-col items-center justify-center gap-2.5 text-center select-none px-6 py-6">
          <div className="flex items-center gap-2 text-[13px] text-zinc-400">
            <Sparkles size={15} className="text-zinc-300" />
            <span className="house-font-display font-semibold tracking-wide text-white">{assistantName}</span>
            <span className="house-status-dot" />
          </div>
          <p className="text-[12.5px] leading-5 text-zinc-500 max-w-sm">
            Your global assistant — always remembers you, everywhere you work. Ask anything, or open a workspace below.
          </p>
        </div>
      ) : (
        <div className="flex flex-col gap-5">
          {messages.map((m) => {
            if (m.role === 'user') {
              return (
                <div key={m.id} className="flex justify-end house-enter">
                  <div className="max-w-[85%] px-3.5 py-2 rounded-2xl rounded-br-xs bg-white/[0.09] border border-white/[0.08] text-[13.5px] leading-6 text-zinc-100 whitespace-pre-wrap">
                    {m.content}
                  </div>
                </div>
              );
            }
            const isLive = m.id === 'house-live';
            return (
              <div key={m.id} className="flex flex-col gap-1.5 house-enter">
                <div className="flex items-center gap-2">
                  <span className="house-font-display text-[11px] font-semibold tracking-widest text-zinc-400 uppercase">{assistantName}</span>
                  {m.duration === 'active' && (
                    <span className="flex gap-1">
                      <span className="house-typing-dot" />
                      <span className="house-typing-dot" />
                      <span className="house-typing-dot" />
                    </span>
                  )}
                </div>
                <div className="text-[13.5px] leading-6 text-zinc-200">
                  {isLive && !m.content.trim() ? (
                    <span className="text-zinc-500 italic text-[12.5px]">Working on it...</span>
                  ) : (
                    <Markdown content={m.content} />
                  )}
                </div>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
