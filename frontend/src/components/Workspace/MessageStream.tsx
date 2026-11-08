import { Fragment, useRef, useEffect, useMemo, useState } from 'react';
import { ArrowDown } from 'lucide-react';
import type { ChatMessage } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import { isInternalTransport } from '../../utils/streamPresentation';
import MessageBubble from './MessageBubble';
import ConversationRail from './ConversationRail';

interface Props {
  messages: ChatMessage[];
  hasActiveChat: boolean;
  onOpenSubagent: (name: string, task: string) => void;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
  activeSubagent?: string | null;
  traceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
  onOpenProject?: (ref: { workspace_id: string; chat_id: string }) => void;
  onRetry?: (prompt: string) => void;
  onRewind?: (message: ChatMessage) => void;
  /** Kept for compat — empty state is now fixed to "What to do Boss?" */
  emptyTitle?: string;
  emptyHint?: string;
}

function dayKey(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? '' : date.toDateString();
}

function dayLabel(value: string): string {
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return 'Today';
  if (date.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return date.toLocaleDateString([], { month: 'short', day: 'numeric', year: date.getFullYear() === today.getFullYear() ? undefined : 'numeric' });
}

export default function MessageStream({ messages, hasActiveChat, onOpenSubagent, onOpenSubagentSession, activeSubagent, traceSteps = [], workspaceId, chatId, onCloseSubagent, onOpenProject, onRetry, onRewind }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  /** Follow streaming/updates until the user scrolls away from the bottom.
   *  Scrolling back down (or clicking the pill) resumes following. */
  const followRef = useRef(true);
  const [scrollDist, setScrollDist] = useState(0);
  const [unread, setUnread] = useState(0);
  const shownPill = scrollDist > 120;

  function stickToBottom() {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }

  // Follow content growth: streaming text, reasoning, tool expansion,
  // images loading — anything that increases the scroll height.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const observer = new ResizeObserver(() => {
      if (followRef.current) el.scrollTop = el.scrollHeight;
    });
    if (contentRef.current) observer.observe(contentRef.current);
    observer.observe(el);
    return () => observer.disconnect();
  }, [hasActiveChat]);

  useEffect(() => {
    if (followRef.current) stickToBottom();
  }, [messages]);

  // Switching chats resets follow mode and starts pinned to the newest turn.
  useEffect(() => {
    followRef.current = true;
    setUnread(0);
    setScrollDist(0);
    stickToBottom();
  }, [chatId]);

  const handleScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    const dist = el.scrollHeight - el.clientHeight - el.scrollTop;
    // At the bottom → follow again (scroll-back resumes following, matching
    // a terminal/opencode style), slightly above → stop forcing.
    followRef.current = dist <= 40;
    setScrollDist(Math.max(0, dist));
    if (dist <= 40) setUnread(0);
  };

  useEffect(() => {
    if (followRef.current) {
      setUnread(0);
    } else {
      setUnread((u) => u + 1);
    }
  }, [messages]);

  const jumpToBottom = () => {
    const el = scrollRef.current;
    if (!el) return;
    followRef.current = true;
    el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
    setUnread(0);
    setScrollDist(0);
  };

  const visibleMessages = messages.filter((message) => !isInternalTransport(message.content));
  const isEmpty = !hasActiveChat && visibleMessages.length <= 1;

  // General mode persists the assistant response first and the trace record
  // after the run completes. Keep an empty trace attached before that response
  // so the duration/activity summary reads like the project chat flow.
  const orderedMessages = visibleMessages.reduce<ChatMessage[]>((ordered, message) => {
    const isEmptyTrace = message.role === 'agent'
      && !!message.metadata?.isTrace
      && !message.content.trim();
    const previous = ordered[ordered.length - 1];
    if (isEmptyTrace && previous?.role === 'agent' && !previous.metadata?.isTrace) {
      ordered.splice(ordered.length - 1, 0, message);
    } else {
      ordered.push(message);
    }
    return ordered;
  }, []);

  // Single error rendering: keep the red `role: 'error'` bubble, drop an
  // agent bubble that only repeats the same failure. The backend sometimes
  // persists the failure both as the agent's final text ("Error: …") and as
  // a separate error message — rendering both reads as a double error.
  const normalizeError = (text: string) => text.trim().replace(/\s+/g, ' ').toLowerCase();
  const errorTexts = new Set(
    orderedMessages
      .filter((m) => m.role === 'error' && m.content.trim())
      .map((m) => normalizeError(m.content)),
  );
  const dedupedMessages = orderedMessages.filter((message) => {
    if (message.role !== 'agent' || !message.content.trim() || errorTexts.size === 0) return true;
    const text = normalizeError(message.content);
    if (errorTexts.has(text)) return false;
    // Agent bubble that is just the error with an "Error:" prefix.
    if (text.startsWith('error:')) {
      for (const err of errorTexts) {
        if (err.includes(text.slice(6).trim()) || text.includes(err.replace(/^error:\s*/, ''))) return false;
      }
    }
    return true;
  });

  // Retry: each agent message knows the last user prompt before it.
  const retryMap = useMemo(() => {
    const map: Record<string, string> = {};
    let lastUserContent = '';
    for (const msg of dedupedMessages) {
      if (msg.role === 'user' && msg.content) lastUserContent = msg.content;
      map[msg.id] = lastUserContent;
    }
    return map;
  }, [dedupedMessages]);

  return (
    <div className="relative flex-1 min-h-0">
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        className="h-full overflow-y-auto pl-7 pr-3 md:pl-8 md:pr-4 py-3"
      >
        {isEmpty ? (
          <div className="flex items-center justify-center min-h-[60vh] h-full">
            <p className="text-[19px] font-medium tracking-tight text-white">What to do Boss?</p>
          </div>
        ) : (
          <div ref={contentRef} className="max-w-[1100px] mx-auto flex flex-col justify-start" style={{ width: '100%' }}>
            {dedupedMessages.map((msg, index) => {
              const currentDay = dayKey(msg.timestamp);
              const previousDay = index > 0 ? dayKey(dedupedMessages[index - 1].timestamp) : '';
              return (
                <Fragment key={msg.id}>
                  {currentDay && currentDay !== previousDay && (
                    <div className="chat-date-divider">
                      <span className="chat-date-line" />
                      <span className="chat-date-label">{dayLabel(msg.timestamp)}</span>
                      <span className="chat-date-line" />
                    </div>
                  )}
                  <div data-message-anchor={msg.id}>
                    <MessageBubble message={msg} onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} activeSubagent={activeSubagent} traceSteps={traceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} onOpenProject={onOpenProject} onRetry={onRetry ? () => onRetry(retryMap[msg.id] || '') : undefined} onRewind={onRewind} />
                  </div>
                </Fragment>
              );
            })}
          </div>
        )}
      </div>

      {!isEmpty && <ConversationRail messages={dedupedMessages} scrollRef={scrollRef} contentRef={contentRef} />}

      {/* Tier D: jump-to-bottom pill (unread count while scrolled up) */}
      <button
        type="button"
        onClick={jumpToBottom}
        className={`scroll-bottom-pill ${shownPill ? 'is-visible' : ''}`}
        aria-label="Jump to latest"
      >
        <ArrowDown size={13} />
        Latest
        {unread > 0 && <span className="pill-count">{unread > 99 ? '99+' : unread}</span>}
      </button>
    </div>
  );
}
