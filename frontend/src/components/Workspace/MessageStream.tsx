import { useRef, useEffect, useMemo, useState } from 'react';
import { ArrowDown } from 'lucide-react';
import type { ChatMessage } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import MessageBubble from './MessageBubble';

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

export default function MessageStream({ messages, hasActiveChat, onOpenSubagent, onOpenSubagentSession, activeSubagent, traceSteps = [], workspaceId, chatId, onCloseSubagent, onOpenProject, onRetry, onRewind }: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const isAtBottomRef = useRef(true);
  const lastScrollHeightRef = useRef(0);

  useEffect(() => {
    if (scrollRef.current) {
      lastScrollHeightRef.current = scrollRef.current.scrollHeight;
    }
  }, []);

  useEffect(() => {
    if (!scrollRef.current || !isAtBottomRef.current) return;
    scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
  }, [messages]);

  useEffect(() => {
    if (!contentRef.current) return;
    const observer = new ResizeObserver(() => {
      if (isAtBottomRef.current && scrollRef.current) {
        scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      }
    });
    observer.observe(contentRef.current);
    return () => observer.disconnect();
  }, [messages, hasActiveChat]);

  const handleScroll = () => {
    if (!scrollRef.current) return;
    const el = scrollRef.current;
    const wasAtBottom = isAtBottomRef.current;
    const heightChanged = el.scrollHeight !== lastScrollHeightRef.current;
    let bottom = el.scrollHeight - el.clientHeight <= el.scrollTop + 60;
    if (heightChanged && wasAtBottom) bottom = true;
    lastScrollHeightRef.current = el.scrollHeight;
    isAtBottomRef.current = bottom;
    // Track how far above the bottom we are (for the jump-to-bottom pill).
    const dist = el.scrollHeight - el.clientHeight - el.scrollTop;
    setScrollDist(Math.max(0, dist));
  };

  // Tier D: jump-to-bottom pill — shown when the user scrolls up; counts new
  // messages that arrived while away from the bottom.
  const [scrollDist, setScrollDist] = useState(0);
  const [unread, setUnread] = useState(0);
  const wasAtBottomOnUpdate = useRef(true);
  const shownPill = scrollDist > 120;

  useEffect(() => {
    if (isAtBottomRef.current) {
      wasAtBottomOnUpdate.current = true;
      setUnread(0);
    } else {
      wasAtBottomOnUpdate.current = false;
      setUnread((u) => u + 1);
    }
  }, [messages]);

  useEffect(() => {
    if (!shownPill && wasAtBottomOnUpdate.current) setUnread(0);
  }, [shownPill]);

  const jumpToBottom = () => {
    if (!scrollRef.current) return;
    scrollRef.current.scrollTo({ top: scrollRef.current.scrollHeight, behavior: 'smooth' });
    setUnread(0);
    setScrollDist(0);
    isAtBottomRef.current = true;
  };

  const isEmpty = !hasActiveChat && messages.length <= 1;

  // General mode persists the assistant response first and the trace record
  // after the run completes. Keep an empty trace attached before that response
  // so the duration/activity summary reads like the project chat flow.
  const orderedMessages = messages.reduce<ChatMessage[]>((ordered, message) => {
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

  // Retry: each agent message knows the last user prompt before it.
  const retryMap = useMemo(() => {
    const map: Record<string, string> = {};
    let lastUserContent = '';
    for (const msg of orderedMessages) {
      if (msg.role === 'user' && msg.content) lastUserContent = msg.content;
      map[msg.id] = lastUserContent;
    }
    return map;
  }, [orderedMessages]);

  return (
    <div className="relative flex-1 min-h-0">
      <div
        ref={scrollRef}
        onScroll={handleScroll}
        className="h-full overflow-y-auto"
        style={{ paddingLeft: '40px', paddingRight: '40px', paddingTop: '16px', paddingBottom: '16px' }}
      >
        {isEmpty ? (
          <div className="flex items-center justify-center min-h-[60vh] h-full">
            <p className="text-[19px] font-medium tracking-tight text-white">What to do Boss?</p>
          </div>
        ) : (
          <div ref={contentRef} className="max-w-[1100px] mx-auto flex flex-col justify-start" style={{ width: '100%' }}>
            {orderedMessages.map((msg) => (
              <MessageBubble key={msg.id} message={msg} onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} activeSubagent={activeSubagent} traceSteps={traceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} onOpenProject={onOpenProject} onRetry={onRetry ? () => onRetry(retryMap[msg.id] || '') : undefined} onRewind={onRewind} />
            ))}
          </div>
        )}
      </div>

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
