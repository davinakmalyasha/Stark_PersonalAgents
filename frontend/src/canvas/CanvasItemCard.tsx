import { useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import {
  ArrowUp, Bot, ChevronDown, ChevronUp, Clock3, FolderKanban, GripVertical,
  MessageSquare, Palette, Plus, X,
} from 'lucide-react';
import type { ChatSummary, GroupProjectRow, WorkspaceInfo } from '../types';
import type { BoardAgent } from '../types/board.types';
import { cleanChatTitle } from '../utils/chatTitle';
import type { CanvasItem } from './canvasTypes';
import { displayActor } from '../house/group/groupProjectUi';

interface Props {
  item: CanvasItem;
  chat?: ChatSummary;
  workspace?: WorkspaceInfo;
  project?: GroupProjectRow;
  agents: BoardAgent[];
  running: boolean;
  elapsed?: string;
  expanded: boolean;
  onToggleExpanded: () => void;
  onOpen: () => void;
  onRemove: () => void;
  onSend?: (text: string) => Promise<void> | void;
  /** Live map context. */
  compact?: boolean;
  isActive?: boolean;
  childChats?: ChatSummary[];
  hiddenChildren?: number;
  onOpenChild?: (workspaceId: string, chatId: string) => void;
  onCloseTab?: () => void;
  tokens?: number;
  cost?: number;
  toolCalls?: number;
}

type CardStatus = 'running' | 'waiting' | 'done' | 'error' | 'idle';

const STATUS_LABEL: Record<CardStatus, string> = {
  running: 'Working',
  waiting: 'Queued',
  done: 'Done',
  error: 'Failed',
  idle: 'Idle',
};

const AGENT_DOT: Record<string, string> = {
  idle: 'bg-zinc-600',
  working: 'bg-white',
  completed: 'bg-emerald-400',
  failed: 'bg-rose-400',
};

const GROUP_STATUS: Record<string, CardStatus> = {
  running: 'running',
  done: 'done',
  paused: 'waiting',
  stalled: 'waiting',
  failed: 'error',
};

function timeAgo(iso?: string): string {
  if (!iso) return '';
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return '';
  const mins = Math.floor(Math.max(0, Date.now() - then) / 60000);
  if (mins < 1) return 'now';
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function compactNumber(value?: number): string {
  if (!value || value < 1) return '';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}k`;
  return String(Math.round(value));
}

function chatStatus(chat: ChatSummary, running: boolean): CardStatus {
  if (running) return 'running';
  if (chat.lastRole === 'error') return 'error';
  if (chat.lastRole === 'user') return 'waiting';
  return 'done';
}

function StatusChip({ status }: { status: CardStatus }) {
  return (
    <span className={`canvas-card-chip is-${status}`}>
      <span className="canvas-card-chip-dot" />
      {STATUS_LABEL[status]}
    </span>
  );
}

export default function CanvasItemCard({
  item, chat, workspace, project, agents, running, elapsed,
  expanded, onToggleExpanded, onOpen, onRemove, onSend,
  compact = false, isActive = false, childChats = [], hiddenChildren = 0,
  onOpenChild, onCloseTab, tokens, cost, toolCalls,
}: Props) {
  const [draft, setDraft] = useState('');
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState('');

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const text = draft.trim();
    if (!text || sending || running || !onSend) return;
    setSending(true);
    setSendError('');
    try {
      await onSend(text);
      setDraft('');
    } catch (err) {
      setSendError(err instanceof Error ? err.message : 'Could not send.');
    } finally {
      setSending(false);
    }
  };

  const closeLabel = item.kind === 'chat' && item.auto && item.isTab && onCloseTab
    ? 'Close tab'
    : 'Remove from canvas';
  const close = item.kind === 'chat' && item.auto && item.isTab && onCloseTab
    ? onCloseTab
    : onRemove;

  const header = (icon: ReactNode, title: string, subtitle: string, status: CardStatus) => (
    <div className="canvas-card-head">
      <GripVertical size={12} className="canvas-card-grip" />
      <span className="canvas-card-icon">{icon}</span>
      <div className="canvas-card-title-wrap">
        <div className="canvas-card-title" title={title}>{title}</div>
        <div className="canvas-card-subtitle" title={subtitle}>{subtitle}</div>
      </div>
      <StatusChip status={status} />
      {item.kind === 'chat' && (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onToggleExpanded(); }}
          className="canvas-card-icon-button"
          title={expanded ? 'Collapse details' : 'Show details'}
          aria-label={expanded ? 'Collapse details' : 'Show details'}
        >
          {expanded ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
        </button>
      )}
      <button
        type="button"
        onClick={(e) => { e.stopPropagation(); close(); }}
        className="canvas-card-icon-button is-close"
        title={closeLabel}
        aria-label={closeLabel}
      >
        <X size={11} />
      </button>
    </div>
  );

  if (item.kind === 'chat') {
    if (!chat || !workspace) {
      return (
        <div data-canvas-card data-item-id={item.id} className={`canvas-card is-unavailable ${compact ? 'is-compact' : ''}`}>
          {header(<MessageSquare size={12} />, 'Chat unavailable', item.workspaceId, 'idle')}
          <div className="canvas-card-empty">This chat was deleted or is no longer available.</div>
        </div>
      );
    }

    const visibleAgents = agents.filter(
      (agent) => agent.role === 'Subagent' && (agent.steps > 0 || agent.status === 'working' || agent.status === 'completed'),
    );
    const activeAgent = visibleAgents.find((agent) => agent.isActive);
    const currentTask = activeAgent?.task || visibleAgents.find((agent) => agent.status !== 'idle')?.task || '';
    const status = chatStatus(chat, running);
    const directChildren = childChats.filter((child) => child.id !== chat.id);
    const hasDetails = visibleAgents.length > 0 || directChildren.length > 0 || hiddenChildren > 0;

    return (
      <div
        data-canvas-card
        data-item-id={item.id}
        className={[
          'canvas-card',
          compact ? 'is-compact' : '',
          running ? 'is-running' : '',
          isActive ? 'is-active' : '',
          expanded ? 'is-expanded' : '',
        ].filter(Boolean).join(' ')}
      >
        {header(
          item.depth && item.depth > 0 ? <Bot size={12} /> : <MessageSquare size={12} />,
          cleanChatTitle(chat.title),
          workspace.isGeneral ? 'Main workspace' : workspace.name,
          status,
        )}

        <div className="canvas-card-body">
          {running ? (
            <div className="canvas-card-live">
              <span className="canvas-card-live-dot" />
              <span className="canvas-card-live-text">{currentTask || chat.lastMessage || 'working…'}</span>
              {elapsed && <span className="canvas-card-live-time">{elapsed}</span>}
            </div>
          ) : (
            <div className="canvas-card-preview">
              {chat.lastMessage || (chat.lastRole === 'error' ? 'Run failed.' : 'No messages yet.')}
            </div>
          )}

          <div className="canvas-card-metrics">
            <span title="Messages"><MessageSquare size={9} />{chat.messageCount || 0}</span>
            {!!toolCalls && <span title="Tool calls">{toolCalls} tools</span>}
            {!!tokens && <span title="Tokens">{compactNumber(tokens)} tok</span>}
            {!!cost && <span title="Cost">${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}</span>}
            <span className="canvas-card-time"><Clock3 size={9} />{timeAgo(chat.updatedAt)}</span>
          </div>
        </div>

        {expanded && (
          <div className="canvas-card-details">
            {hasDetails ? (
              <div className="canvas-card-agent-list">
                <div className="canvas-card-section-label">
                  <span>Agents</span>
                  {(visibleAgents.length + directChildren.length) > 0 && (
                    <span className="canvas-card-section-count">{visibleAgents.length + directChildren.length}</span>
                  )}
                </div>
                {visibleAgents.map((agent) => (
                  <div key={agent.id} className="canvas-card-agent-row">
                    <span className={`canvas-card-agent-dot ${AGENT_DOT[agent.status] || 'bg-zinc-600'} ${agent.isActive ? 'is-live' : ''}`} />
                    <span className="canvas-card-agent-name">{agent.label}</span>
                    <span className="canvas-card-agent-task">{agent.task || agent.status}</span>
                    <span className="canvas-card-agent-steps">{agent.steps}</span>
                  </div>
                ))}
                {directChildren.slice(0, 6).map((child) => (
                  <button
                    key={child.id}
                    type="button"
                    className="canvas-card-agent-row is-link"
                    onClick={(e) => { e.stopPropagation(); onOpenChild?.(workspace.id, child.id); }}
                    title={`Open ${child.title}`}
                  >
                    <span className={`canvas-card-agent-dot ${child.lastRole === 'error' ? 'bg-rose-400' : child.lastRole === 'user' ? 'bg-amber-300' : 'bg-zinc-500'}`} />
                    <span className="canvas-card-agent-name">{cleanChatTitle(child.title)}</span>
                    <span className="canvas-card-agent-task">{child.lastRole === 'error' ? 'failed' : child.lastRole === 'user' ? 'working' : 'done'}</span>
                    <span className="canvas-card-agent-steps">{timeAgo(child.updatedAt)}</span>
                  </button>
                ))}
                {hiddenChildren > 0 && (
                  <div className="canvas-card-overflow">+{hiddenChildren} more spawned agents</div>
                )}
              </div>
            ) : (
              <div className="canvas-card-empty">No child agents in this run.</div>
            )}
            <button type="button" onClick={(e) => { e.stopPropagation(); onOpen(); }} className="canvas-card-open">
              Open chat
            </button>
          </div>
        )}

        <form data-no-drag onSubmit={submit} className="canvas-card-dock">
          <div className="canvas-card-input-wrap">
            <input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              disabled={sending || running}
              placeholder={sending ? 'Sending…' : running ? 'Agent working…' : 'Message this chat…'}
              className="canvas-card-input"
            />
            <button
              type="submit"
              disabled={sending || running || !draft.trim()}
              className="canvas-card-send"
              aria-label="Send message"
            >
              <ArrowUp size={11} />
            </button>
          </div>
          {sendError && <div className="canvas-card-error">{sendError}</div>}
        </form>
      </div>
    );
  }

  if (item.kind === 'project') {
    if (!project) {
      return (
        <div data-canvas-card data-item-id={item.id} className="canvas-card is-unavailable">
          {header(<FolderKanban size={12} />, 'Project unavailable', item.projectId, 'idle')}
          <div className="canvas-card-empty">This project is no longer available.</div>
        </div>
      );
    }
    const pct = project.items_total ? Math.round((project.items_done / project.items_total) * 100) : 0;
    const status = GROUP_STATUS[project.status] || 'idle';
    return (
      <div data-canvas-card data-item-id={item.id} className={`canvas-card ${project.awaiting_approval > 0 ? 'is-waiting' : ''}`}>
        {header(
          <FolderKanban size={12} />,
          project.title || project.id,
          `Project · ${project.items_done}/${project.items_total} items`,
          status,
        )}
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onOpen(); }}
          className="canvas-card-project-body"
        >
          <div className="canvas-card-preview">
            {project.active_item
              ? `${displayActor(project.active_member || '')} · ${project.active_item.title}`
              : project.last_message?.body || project.goal || 'No activity yet.'}
          </div>
          <div className="canvas-card-progress"><div style={{ width: `${pct}%` }} /></div>
          <div className="canvas-card-metrics">
            <span>{project.items_done}/{project.items_total} items</span>
            <span className="canvas-card-time">{timeAgo(String(project.last_activity_ts ? project.last_activity_ts * 1000 : ''))}</span>
          </div>
          {project.awaiting_approval > 0 && (
            <div className="canvas-card-waiting">{project.awaiting_approval} waiting on you</div>
          )}
        </button>
      </div>
    );
  }

  if (!workspace) {
    return (
      <div data-canvas-card data-item-id={item.id} className="canvas-card is-unavailable">
        {header(<Palette size={12} />, 'Addon unavailable', item.workspaceId, 'idle')}
      </div>
    );
  }

  return (
    <div data-canvas-card data-item-id={item.id} className="canvas-card">
      {header(
        <Palette size={12} />,
        item.addonId === 'design' ? 'OpenDesign' : 'Addon',
        workspace.isGeneral ? 'Main workspace' : workspace.name,
        'idle',
      )}
      <button
        type="button"
        onClick={(e) => { e.stopPropagation(); onOpen(); }}
        className="canvas-card-addon-open"
      >
        <Plus size={10} /> Open addon
      </button>
    </div>
  );
}
