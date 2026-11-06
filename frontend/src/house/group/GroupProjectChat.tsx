import { useEffect, useMemo, useRef, useState } from 'react';
import type { KeyboardEvent as ReactKeyboardEvent } from 'react';
import {
  AlertTriangle, ArrowUp, AtSign, Bot, Check, ChevronDown, MessageSquare,
  Sparkles, Users, X,
} from 'lucide-react';
import Markdown from '../../components/Workspace/Markdown';
import type { GroupEvent, GroupItem, GroupMember, GroupMessage } from '../../types';
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
  projectStatus?: string;
  members?: GroupMember[];
  onSend?: (body: string, to: string) => Promise<void>;
}

type Filter = 'everyone' | 'you' | string;
type Target = 'all' | 'supervisor' | string;

const OWNER_NAMES = new Set(['owner', 'human', 'you', 'user']);
const SUPERVISOR_NAMES = new Set(['supervisor', 'stark', 'orchestrator']);
const NON_PARTICIPANTS = new Set(['all', 'everyone', 'group', 'owner', 'human', 'you', 'user', 'supervisor', 'stark', 'orchestrator']);

function rawName(value: string): string {
  return (value || '').trim().replace(/^member:/i, '').trim();
}

function isOwner(value: string): boolean {
  return OWNER_NAMES.has(rawName(value).toLowerCase());
}

function isSupervisor(value: string): boolean {
  return SUPERVISOR_NAMES.has(rawName(value).toLowerCase());
}

function actorLabel(value: string): string {
  if (isOwner(value)) return 'You';
  if (isSupervisor(value)) return 'Orchestrator';
  const label = displayActor(value);
  return label ? label.charAt(0).toUpperCase() + label.slice(1) : 'Member';
}

function targetLabel(value: string): string {
  if (value === 'all') return 'Everyone';
  if (value === 'supervisor') return 'Orchestrator';
  return actorLabel(value);
}

function sameActor(left: string, right: string): boolean {
  return rawName(left).toLowerCase() === rawName(right).toLowerCase();
}

function Avatar({ name, size = 'md', owner = false }: { name: string; size?: 'sm' | 'md'; owner?: boolean }) {
  return (
    <span className={`gp-avatar ${owner ? 'gp-avatar--owner' : avatarTone(name)} ${size === 'sm' ? 'gp-avatar--sm' : ''}`}>
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
  projectStatus = '', members = [], onSend,
}: Props) {
  const [filter, setFilter] = useState<Filter>('everyone');
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState('');
  const [target, setTarget] = useState<Target>('all');
  const [menu, setMenu] = useState<'target' | 'mention' | null>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const textRef = useRef<HTMLTextAreaElement | null>(null);
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

  const participants = useMemo(() => {
    const seen = new Map<string, string>();
    const add = (raw: string) => {
      const name = rawName(raw);
      if (!name || NON_PARTICIPANTS.has(name.toLowerCase())) return;
      const key = name.toLowerCase();
      if (!seen.has(key)) seen.set(key, raw);
    };
    for (const entry of entries) {
      add(entry.from);
      add(entry.to);
    }
    for (const member of members) {
      add(member.name);
      if (member.display) add(member.display);
    }
    return Array.from(seen.values());
  }, [entries, members]);

  const mentionMatch = draft.match(/(?:^|\s)@([^\s@]*)$/);
  const mentionQuery = mentionMatch ? mentionMatch[1].toLowerCase() : '';
  const targetOptions = useMemo(
    () => [
      { value: 'all' as const, label: 'Everyone', hint: 'The whole team' },
      { value: 'supervisor' as const, label: 'Orchestrator', hint: 'Runs the project board' },
      ...participants.map((name) => ({ value: name, label: actorLabel(name), hint: 'Project member' })),
    ].filter((option) => !mentionQuery || option.label.toLowerCase().includes(mentionQuery)),
    [participants, mentionQuery],
  );

  const filteredEntries = useMemo(() => {
    if (filter === 'everyone') return entries;
    return entries.filter((entry) => {
      if (filter === 'you') return isOwner(entry.from) || isOwner(entry.to);
      return sameActor(entry.from, filter) || sameActor(entry.to, filter);
    });
  }, [entries, filter]);

  const derived = messages.length === 0 && entries.length > 0;
  const approvalIds = new Set(entries.filter((entry) => entry.kind === 'approval').map((entry) => entry.itemId));
  const pendingCards = waiting.filter((item) => {
    if (approvalIds.has(item.id)) return false;
    if (filter === 'everyone' || filter === 'you') return true;
    return sameActor(String(item.lease_owner || item.owner_member || ''), filter);
  });
  const filterCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const entry of entries) {
      if (entry.kind !== 'message') continue;
      for (const raw of [entry.from, entry.to]) {
        if (!raw) continue;
        const name = rawName(raw);
        if (!name || NON_PARTICIPANTS.has(name.toLowerCase())) continue;
        const key = name.toLowerCase();
        counts.set(key, (counts.get(key) || 0) + 1);
      }
    }
    return counts;
  }, [entries]);

  useEffect(() => {
    if (!atBottomRef.current) return;
    const el = scrollRef.current;
    if (el) el.scrollTo({ top: el.scrollHeight, behavior: 'smooth' });
  }, [filteredEntries.length, filter]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    atBottomRef.current = el.scrollHeight - el.clientHeight - el.scrollTop < 80;
  };

  const chooseTarget = (value: Target) => {
    setTarget(value);
    setMenu(null);
    if (mentionMatch) {
      setDraft((valueBefore) => valueBefore.replace(/@[^\s@]*$/, '').replace(/\s+$/, ''));
    }
    textRef.current?.focus();
  };

  const submit = async () => {
    const body = draft.trim();
    if (!body || sending || !onSend) return;
    setSending(true);
    setSendError('');
    try {
      await onSend(body, target);
      setDraft('');
      setMenu(null);
      atBottomRef.current = true;
    } catch (err) {
      setSendError(err instanceof Error ? err.message : 'Could not send this message.');
    } finally {
      setSending(false);
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === 'Enter' && !event.shiftKey) {
      event.preventDefault();
      void submit();
    }
    if (event.key === 'Escape') setMenu(null);
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

  const showEmpty = filteredEntries.length === 0 && pendingCards.length === 0;
  const emptyTitle = filter === 'you' ? 'No messages from you yet' : 'No messages yet';
  const emptyCopy = filter === 'you'
    ? 'Your messages to the team will appear here.'
    : filter === 'everyone'
      ? 'Assignments, reports, and teammate messages will appear here.'
      : `Messages involving ${actorLabel(filter)} will appear here.`;
  const isLive = projectStatus === 'running' || projectStatus === 'active';

  return (
    <div className="gp-chat">
      <div className="gp-chat-top">
        <div className="gp-chat-header">
          <div>
            <div className="gp-chat-kicker"><MessageSquare size={12} /> Group conversation</div>
            <div className="gp-chat-subtitle">
              {derived ? 'Activity from the project event log' : 'Messages from Stark and the project members'}
            </div>
          </div>
          <div className="gp-chat-header-side">
            {waiting.length > 0 && (
              <span className="gp-chat-attention"><AlertTriangle size={11} /> {waiting.length} waiting</span>
            )}
            <span className={`gp-chat-live ${isLive ? 'is-live' : ''}`}>
              <span /> {isLive ? 'Live' : projectStatus || 'Offline'}
            </span>
          </div>
        </div>

        <div className="gp-participants" role="tablist" aria-label="Conversation participants">
          <button
            type="button"
            role="tab"
            aria-selected={filter === 'everyone'}
            className={filter === 'everyone' ? 'is-active' : ''}
            onClick={() => setFilter('everyone')}
          >
            <Users size={11} /> Everyone
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={filter === 'you'}
            className={filter === 'you' ? 'is-active' : ''}
            onClick={() => setFilter('you')}
          >
            <span className="gp-participant-avatar is-you">{initials('owner')}</span> You
          </button>
          {participants.map((name) => (
            <button
              key={name}
              type="button"
              role="tab"
              aria-selected={filter === name}
              className={filter === name ? 'is-active' : ''}
              onClick={() => setFilter(name)}
            >
              <span className={`gp-participant-avatar ${avatarTone(name)}`}>{initials(name)}</span>
              <span>{actorLabel(name)}</span>
              {!!filterCounts.get(rawName(name).toLowerCase()) && (
                <span className="gp-participant-count">{filterCounts.get(rawName(name).toLowerCase())}</span>
              )}
            </button>
          ))}
        </div>
      </div>

      <div ref={scrollRef} onScroll={onScroll} className="gp-chat-scroll">
        {showEmpty ? (
          <div className="gp-chat-empty">
            <div className="gp-chat-empty-icon"><Sparkles size={20} /></div>
            <div className="gp-chat-empty-title">{emptyTitle}</div>
            <div className="gp-chat-empty-copy">{emptyCopy}</div>
          </div>
        ) : (
          <div className="gp-chat-list" key={filter}>
            {filteredEntries.map((entry, index) => {
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
                        <Avatar name={entry.from} size="sm" />
                        <div className="min-w-0 flex-1">
                          <div className="gp-message-head">
                            <span>{actorLabel(entry.from)}</span>
                            <span className="gp-message-route">→ owner</span>
                            <time>{shortTime(entry.ts)}</time>
                          </div>
                          <div className="gp-message-body"><Markdown content={entry.body} /></div>
                        </div>
                      </div>
                    )}
                  </div>
                );
              }
              const supervisor = isSupervisor(entry.from);
              const owner = isOwner(entry.from);
              const previous = filteredEntries[index - 1];
              const continuation = !!previous
                && previous.kind === 'message'
                && sameActor(previous.from, entry.from)
                && entry.ts - previous.ts < 300;
              return (
                <div
                  key={entry.id}
                  className={[
                    'gp-message',
                    supervisor ? 'gp-message--supervisor' : '',
                    owner ? 'gp-message--owner' : '',
                    continuation ? 'is-continuation' : '',
                  ].filter(Boolean).join(' ')}
                >
                  <Avatar name={entry.from} owner={owner} />
                  <div className="gp-message-main">
                    <div className="gp-message-head">
                      <span className="gp-message-name">{actorLabel(entry.from)}</span>
                      <span className="gp-message-route">
                        to {isOwner(entry.to) ? 'owner' : targetLabel(entry.to)}
                      </span>
                      {entry.derived && <span className="gp-message-derived">log</span>}
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

      <div className="gp-composer-wrap">
        {sendError && <div className="gp-composer-error">{sendError}</div>}
        {menu && (
          <div className="gp-target-menu" role="menu" aria-label="Message target">
            <div className="gp-target-menu-head">
              {menu === 'mention' || mentionMatch ? <AtSign size={11} /> : <Users size={11} />}
              {mentionMatch ? `Mention ${mentionQuery ? `“${mentionQuery}”` : 'an agent'}` : 'Send to'}
            </div>
            {targetOptions.length === 0 && <div className="gp-target-empty">No matching agents.</div>}
            {targetOptions.map((option) => (
              <button
                key={option.value}
                type="button"
                role="menuitem"
                className={target === option.value ? 'is-active' : ''}
                onClick={() => chooseTarget(option.value)}
              >
                <span className="gp-target-avatar">{initials(option.value === 'all' ? 'Everyone' : option.value === 'supervisor' ? 'supervisor' : option.value)}</span>
                <span className="gp-target-copy">
                  <b>{option.label}</b>
                  <small>{option.hint}</small>
                </span>
                {target === option.value && <Check size={12} className="ml-auto text-emerald-400" />}
              </button>
            ))}
          </div>
        )}
        <div className="gp-composer">
          <button
            type="button"
            className="gp-composer-target"
            onClick={() => setMenu(menu === 'target' ? null : 'target')}
            aria-expanded={menu === 'target'}
          >
            <span className="gp-composer-to">to</span>
            <span className="gp-composer-target-name">{targetLabel(target)}</span>
            <ChevronDown size={11} />
          </button>
          <div className="gp-composer-input-wrap">
            <button
              type="button"
              className="gp-composer-at"
              onClick={() => { setMenu('mention'); textRef.current?.focus(); }}
              title="Mention an agent"
              aria-label="Mention an agent"
            >
              <AtSign size={13} />
            </button>
            <textarea
              ref={textRef}
              rows={1}
              value={draft}
              onChange={(event) => {
                const value = event.target.value;
                setDraft(value);
                if (/@([^\s@]*)$/.test(value)) setMenu('mention');
                else if (menu === 'mention') setMenu(null);
              }}
              onKeyDown={onKeyDown}
              placeholder={onSend ? 'Message the agents — @ to name one' : 'Group messaging is connecting…'}
              disabled={!onSend || sending}
              className="gp-composer-input"
            />
            <button
              type="button"
              className="gp-composer-send"
              onClick={() => { void submit(); }}
              disabled={!draft.trim() || sending || !onSend}
              aria-label="Send message"
            >
              <ArrowUp size={14} />
            </button>
          </div>
        </div>
        <div className="gp-composer-hint">
          <Bot size={10} />
          {onSend ? 'Messages reach the team on its next turn; replies land here.' : 'Waiting for the group messaging backend.'}
        </div>
      </div>
    </div>
  );
}
