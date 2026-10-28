import { useEffect, useMemo, useRef, useState } from 'react';
import { AlertTriangle, Check, MessageSquare, Sparkles, X } from 'lucide-react';
import Markdown from '../../components/Workspace/Markdown';
import type { GroupEvent, GroupItem, GroupMessage } from '../../types';
import {
  avatarTone, deriveChatEntries, displayActor, initials, shortTime, type ChatEntry,
} from './groupProjectUi';

interface Props {
  messages: GroupMessage[];
  events: GroupEvent[];
  items: GroupItem[];
  waiting: GroupItem[];
  busyItem: string | null;
  onResolve: (itemId: string, grant: boolean) => void;
  loading: boolean;
  error: string | null;
}

function Avatar({ name, size = 'md' }: { name: string; size?: 'sm' | 'md' }) {
  return (
    <span className={`gp-avatar ${avatarTone(name)} ${size === 'sm' ? 'gp-avatar--sm' : ''}`}>
      {initials(name)}
    </span>
  );
}

function ChatBody({ body, itemId }: { body: string; itemId?: string | null }) {
  const [expanded, setExpanded] = useState(false);
  const isLong = body.length > 700 || body.split('\n').length > 10;
  return (
    <div>
      {itemId && <div className="gp-message-item">{itemId}</div>}
      <div className={`gp-message-body ${!expanded && isLong ? 'is-clamped' : ''}`}>
        <Markdown content={body} />
      </div>
      {isLong && (
        <button
          type="button"
          className="gp-message-expand"
          onClick={() => setExpanded((value) => !value)}
        >
          {expanded ? 'Show less' : 'Show full report'}
        </button>
      )}
    </div>
  );
}

function ApprovalCard({
  item, busyItem, onResolve,
}: { item: GroupItem; busyItem: string | null; onResolve: (id: string, grant: boolean) => void }) {
  const busy = busyItem === item.id;
  return (
    <div className="gp-approval-card">
      <div className="gp-approval-icon"><AlertTriangle size={15} /></div>
      <div className="min-w-0 flex-1">
        <div className="gp-approval-title">{item.title}</div>
        <div className="gp-approval-copy">
          {item.blocked_reason === 'awaiting_approval'
            ? 'This work item is paused until you approve or reject it.'
            : item.blocked_reason || 'Waiting for your decision.'}
        </div>
        <div className="gp-approval-actions">
          <button
            type="button"
            className="gp-button gp-button--primary"
            disabled={busy}
            onClick={() => onResolve(item.id, true)}
          >
            <Check size={13} /> {busy ? 'Working…' : 'Approve'}
          </button>
          <button
            type="button"
            className="gp-button"
            disabled={busy}
            onClick={() => onResolve(item.id, false)}
          >
            <X size={13} /> Reject
          </button>
        </div>
      </div>
    </div>
  );
}

export default function GroupProjectChat({
  messages, events, items, waiting, busyItem, onResolve, loading, error,
}: Props) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const atBottomRef = useRef(true);

  const entries = useMemo<ChatEntry[]>(() => {
    if (messages.length > 0) {
      return messages.map((message) => ({
        id: `message-${message.seq}`,
        kind: message.kind === 'approval' ? 'approval' : 'message',
        ts: message.ts,
        from: message.from,
        to: message.to,
        body: message.body,
        messageKind: message.kind,
        itemId: message.item_id,
      }));
    }
    return deriveChatEntries(events, items);
  }, [messages, events, items]);

  const derived = messages.length === 0 && entries.length > 0;
  const approvalIds = new Set(entries.filter((entry) => entry.kind === 'approval').map((entry) => entry.itemId));
  const pendingCards = waiting.filter((item) => !approvalIds.has(item.id));

  useEffect(() => {
    if (!atBottomRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
  }, [entries.length]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.clientHeight - el.scrollTop < 80;
  };

  if (loading && entries.length === 0) {
    return (
      <div className="gp-chat gp-chat--empty">
        <div className="gp-chat-empty">
          <div className="gp-skeleton h-8 w-8 rounded-full" />
          <div className="gp-skeleton h-3 w-36" />
          <div className="gp-skeleton h-3 w-52" />
        </div>
      </div>
    );
  }

  if (error && entries.length === 0) {
    return <div className="gp-chat gp-chat--empty"><div className="gp-chat-empty text-red-400">{error}</div></div>;
  }

  return (
    <div ref={scrollRef} onScroll={onScroll} className="gp-chat">
      <div className="gp-chat-header">
        <div>
          <div className="gp-chat-kicker"><MessageSquare size={12} /> Group conversation</div>
          <div className="gp-chat-subtitle">
            {derived ? 'Activity from the project event log' : 'Messages from Stark and the project members'}
          </div>
        </div>
        {waiting.length > 0 && (
          <span className="gp-chat-attention"><AlertTriangle size={11} /> {waiting.length} waiting</span>
        )}
      </div>

      {entries.length === 0 && waiting.length === 0 ? (
        <div className="gp-chat-empty">
          <div className="gp-chat-empty-icon"><Sparkles size={20} /></div>
          <div className="gp-chat-empty-title">No messages yet</div>
          <div className="gp-chat-empty-copy">
            Assignments, reports, and teammate messages will appear here.
          </div>
        </div>
      ) : (
        <div className="gp-chat-list">
          {entries.map((entry) => {
            if (entry.kind === 'activity') {
              return (
                <div key={entry.id} className="gp-activity-line">
                  <span>{entry.body}</span>
                  <time>{shortTime(entry.ts)}</time>
                </div>
              );
            }
            if (entry.kind === 'approval') {
              const item = items.find((candidate) => candidate.id === entry.itemId);
              return (
                <div key={entry.id} className="gp-approval-wrap">
                  {item && waiting.some((candidate) => candidate.id === item.id) ? (
                    <ApprovalCard item={item} busyItem={busyItem} onResolve={onResolve} />
                  ) : (
                    <div className="gp-message gp-message--approval">
                      <div className="gp-message-head">
                        <Avatar name={entry.from} size="sm" />
                        <span>{displayActor(entry.from)}</span>
                        <span className="gp-message-arrow">→ owner</span>
                        <time>{shortTime(entry.ts)}</time>
                      </div>
                      <div className="gp-message-body"><Markdown content={entry.body} /></div>
                    </div>
                  )}
                </div>
              );
            }
            const supervisor = entry.from === 'supervisor' || entry.from === 'owner';
            return (
              <div key={entry.id} className={`gp-message ${supervisor ? 'gp-message--supervisor' : ''}`}>
                <Avatar name={entry.from} />
                <div className="min-w-0 flex-1">
                  <div className="gp-message-head">
                    <span className="gp-message-name">{displayActor(entry.from)}</span>
                    <span className="gp-message-arrow">→ {displayActor(entry.to)}</span>
                    {entry.derived && <span className="gp-message-derived">derived</span>}
                    <time>{shortTime(entry.ts)}</time>
                  </div>
                  {entry.error ? (
                    <div className="gp-message-error">{entry.error}</div>
                  ) : (
                    <ChatBody body={entry.body} itemId={entry.itemId} />
                  )}
                </div>
              </div>
            );
          })}

          {pendingCards.map((item) => (
            <ApprovalCard key={`waiting-${item.id}`} item={item} busyItem={busyItem} onResolve={onResolve} />
          ))}
        </div>
      )}
    </div>
  );
}
