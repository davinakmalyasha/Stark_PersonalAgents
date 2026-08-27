import type { MutableRefObject } from 'react';
import type { ChatMessage } from '../types';
import { extractSubagentSteps } from '../utils/logParser';
import Markdown from '../components/Workspace/Markdown';
import SubagentBlock from './SubagentBlock';
import { useSubagentStatuses } from './useSubagentStatuses';
import { getAssistantName } from './assistant';

interface HouseChatMessagesProps {
  threadRef: MutableRefObject<HTMLDivElement | null>;
  isLoading: boolean;
  messages: ChatMessage[];
  /** Drill-in for spawned subagents (trace messages carry childId steps). */
  onOpenSubagent?: (childId: string, label: string) => void;
  /** Session ids — power the live running/done status of spawn blocks. */
  workspaceId?: string | null;
  chatId?: string | null;
}

export default function HouseChatMessages({
  threadRef,
  isLoading,
  messages,
  onOpenSubagent,
  workspaceId,
  chatId,
}: HouseChatMessagesProps) {
  const assistantName = getAssistantName();
  const subagentStatuses = useSubagentStatuses(workspaceId ?? null, chatId ?? null);
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
        <div className="h-full flex items-center justify-center select-none px-6 py-6">
          <p className="text-[12px] text-zinc-600">No messages yet — say something below.</p>
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
            const subSteps = onOpenSubagent ? extractSubagentSteps(m.metadata?.steps) : [];
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
                {subSteps.length > 0 && (
                  <div className="flex flex-col gap-1.5 mb-1">
                    {subSteps.map((s) => (
                      <SubagentBlock
                        key={`${m.id}-${s.id}-${s.childId}`}
                        step={s}
                        status={subagentStatuses[s.childId!] ?? 'done'}
                        onOpen={onOpenSubagent!}
                      />
                    ))}
                  </div>
                )}
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
