import { useEffect, useRef, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import {
  ArrowUp, AudioLines, Bot, Check, ChevronDown, ChevronUp, CircleAlert, Clock3,
  FileText, FolderKanban, GripVertical, Image as ImageIcon, Loader2, MessageSquare,
  Palette, Paperclip, Plus, Square, X,
} from 'lucide-react';
import { DropOverlay } from '../components/Workspace/AttachmentDrop';
import type { ChatData, ChatSummary, GroupProjectRow, WorkspaceInfo } from '../types';
import { fetchChat } from '../services/api';
import { isInternalTransport } from '../utils/streamPresentation';
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
  onSend?: (text: string, files: File[]) => Promise<void> | void;
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

type CardStatus = 'running' | 'queued' | 'done' | 'failed' | 'aborted' | 'idle';

const STATUS_LABEL: Record<CardStatus, string> = {
  running: 'Running',
  queued: 'Queued',
  done: 'Done',
  failed: 'Failed',
  aborted: 'Aborted',
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
  paused: 'queued',
  stalled: 'queued',
  failed: 'failed',
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

function previewText(chat: ChatSummary): string {
  const text = chat.lastMessage || '';
  if (!text || isInternalTransport(text)) return '';
  return text;
}

function chatStatus(chat: ChatSummary, running: boolean): CardStatus {
  if (running) return 'running';
  if (chat.status) return chat.status;
  if (chat.lastKind === 'aborted') return 'aborted';
  if (chat.lastRole === 'error') return 'failed';
  if (chat.lastRole === 'user') return 'idle';
  if (chat.lastRole === 'agent') return 'done';
  return 'idle';
}

function StatusIcon({ status }: { status: CardStatus }) {
  if (status === 'running') return <Loader2 size={9} className="canvas-card-chip-spinner" />;
  if (status === 'queued') return <Clock3 size={9} />;
  if (status === 'done') return <Check size={9} />;
  if (status === 'failed') return <CircleAlert size={9} />;
  if (status === 'aborted') return <Square size={8} />;
  return <span className="canvas-card-chip-dot" />;
}

function StatusChip({ status }: { status: CardStatus }) {
  return (
    <span className={`canvas-card-chip is-${status}`} data-status={status}>
      <StatusIcon status={status} />
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
  const [files, setFiles] = useState<File[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const dragDepth = useRef(0);
  const fileRef = useRef<HTMLInputElement | null>(null);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState('');
  const [optimistic, setOptimistic] = useState<Array<{ id: string; text: string; ts: number }>>([]);
  const [detail, setDetail] = useState<{ key: string; chat: ChatData | null; error: string } | null>(null);

  // Lazy detail fetch: the summary row stays light; expanding loads the
  // full turn (latest user request + latest answer + counts).
  const detailKey = item.kind === 'chat' && chat && workspace ? `${workspace.id}:${chat.id}` : '';
  useEffect(() => {
    if (!expanded || !detailKey) return;
    if (detail?.key === detailKey) return;
    let cancelled = false;
    const key = detailKey;
    const separator = key.indexOf(':');
    const wsId = key.slice(0, separator);
    const chatId = key.slice(separator + 1);
    fetchChat(wsId, chatId)
      .then((full) => { if (!cancelled) setDetail({ key, chat: full, error: '' }); })
      .catch(() => { if (!cancelled) setDetail({ key, chat: null, error: 'Could not load details.' }); });
    return () => { cancelled = true; };
  }, [expanded, detailKey, detail?.key]);
  const detailChat = detail?.key === detailKey ? detail.chat : null;
  const detailLoading = expanded && !!detailKey && (!detail || detail.key !== detailKey);
  const detailError = detail?.key === detailKey ? detail.error : '';

  const addFiles = (incoming: FileList | File[]) => {
    const valid = Array.from(incoming).filter((file) => file.size <= 20 * 1024 * 1024);
    setFiles((current) => [...current, ...valid.filter((file) => !current.some((item) => item.name === file.name && item.size === file.size))].slice(0, 8));
    if (valid.length !== Array.from(incoming).length) setSendError('Files must be 20 MB or smaller.');
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const text = draft.trim();
    if ((!text && !files.length) || sending || running || !onSend) return;
    const body = text || `Attached ${files.length} file${files.length === 1 ? '' : 's'}`;
    const pending = { id: `opt-${Date.now()}`, text: body, ts: Date.now() };
    // Optimistic: clear the composer immediately and show our message with
    // a sent → running transition while the agent works.
    setDraft('');
    setFiles([]);
    setOptimistic((prev) => [...prev.slice(-2), pending]);
    setSending(true);
    setSendError('');
    try {
      await onSend(body, files);
    } catch (err) {
      setSendError(err instanceof Error ? err.message : 'Could not send.');
      setDraft(body);
    } finally {
      setSending(false);
      setOptimistic((prev) => prev.filter((row) => row.id !== pending.id));
    }
  };

  const closeLabel = item.kind === 'chat' && item.auto && item.isTab && onCloseTab
    ? 'Close tab'
    : 'Remove from canvas';
  const close = item.kind === 'chat' && item.auto && item.isTab && onCloseTab
    ? onCloseTab
    : onRemove;

  const header = (icon: ReactNode, title: string, subtitle: string, status: CardStatus, titleOpen?: () => void) => (
    <div className="canvas-card-head" data-canvas-drag-handle>
      <GripVertical size={12} className="canvas-card-grip" />
      <span className="canvas-card-icon">{icon}</span>
      {titleOpen ? (
        <button
          type="button"
          className="canvas-card-title-wrap is-link"
          onClick={(e) => { e.stopPropagation(); titleOpen(); }}
          title={`Open ${title}`}
        >
          <span className="canvas-card-title" title={title}>{title}</span>
          <span className="canvas-card-subtitle" title={subtitle}>{subtitle}</span>
        </button>
      ) : (
        <div className="canvas-card-title-wrap">
          <div className="canvas-card-title" title={title}>{title}</div>
          <div className="canvas-card-subtitle" title={subtitle}>{subtitle}</div>
        </div>
      )}
      <StatusChip status={status} />
      {item.kind === 'chat' && (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onToggleExpanded(); }}
          className="canvas-card-icon-button is-details"
          title={expanded ? 'Hide agents and details' : 'Show agents and details'}
          aria-label={expanded ? 'Hide agents and details' : 'Show agents and details'}
          aria-expanded={expanded}
        >
          {expanded ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
          {!compact && <span>Details</span>}
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
    const preview = previewText(chat);

    const detailMessages = (detailChat?.messages || []).filter(
      (message) => !isInternalTransport(message.content) && message.content.trim(),
    );
    const lastUserTurn = [...detailMessages].reverse().find((message) => message.role === 'user');
    const lastAnswer = [...detailMessages].reverse().find(
      (message) => (message.role === 'agent' || message.role === 'error') && !message.metadata?.isTrace,
    ) || [...detailMessages].reverse().find((message) => message.role === 'agent');
    const detailMessageCount = detailChat?.messages?.length ?? chat.messageCount ?? 0;

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
          onOpen,
        )}

        <div className="canvas-card-body">
          {running ? (
            <div className="canvas-card-live">
              <span className="canvas-card-live-dot" />
              <span className="canvas-card-live-text">{currentTask || preview || 'working…'}</span>
              {elapsed && <span className="canvas-card-live-time">{elapsed}</span>}
            </div>
          ) : (
            <div className="canvas-card-preview">
              {preview || (chat.lastRole === 'error' ? 'Run failed.' : 'No messages yet.')}
            </div>
          )}

          {optimistic.length > 0 && (
            <div className="canvas-card-pending">
              {optimistic.map((row) => (
                <div key={row.id} className="canvas-card-pending-row">
                  <span className="canvas-card-pending-you">You</span>
                  <span className="canvas-card-pending-text">{row.text}</span>
                  <span className="canvas-card-pending-state">
                    {sending ? 'Sending…' : running ? 'Running…' : 'Sent'}
                  </span>
                </div>
              ))}
            </div>
          )}

          {(!running || expanded) && (
            <div className="canvas-card-metrics">
              <span title="Messages"><MessageSquare size={9} />{detailChat?.messages?.length ?? chat.messageCount ?? 0}</span>
              {!!toolCalls && <span title="Tool calls">{toolCalls} tools</span>}
              {!!tokens && <span title="Tokens">{compactNumber(tokens)} tok</span>}
              {!!cost && <span title="Cost">${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}</span>}
              <span className="canvas-card-time"><Clock3 size={9} />{timeAgo(chat.updatedAt)}</span>
            </div>
          )}
        </div>

        {expanded && (
          <div className="canvas-card-details">
            <div className="canvas-card-agent-list">
              <div className="canvas-card-section-label">
                <span>Latest request</span>
              </div>
              {detailLoading ? (
                <div className="canvas-card-skeleton">Loading conversation…</div>
              ) : detailError ? (
                <div className="canvas-card-empty">{detailError}</div>
              ) : lastUserTurn ? (
                <div className="canvas-card-turn is-user">
                  <span className="canvas-card-turn-role">You</span>
                  <span className="canvas-card-turn-text">{lastUserTurn.content.slice(0, 500)}</span>
                </div>
              ) : (
                <div className="canvas-card-empty">No user request yet.</div>
              )}
              <div className="canvas-card-section-label">
                <span>Latest response</span>
              </div>
              {detailLoading ? (
                <div className="canvas-card-skeleton">Loading conversation…</div>
              ) : lastAnswer ? (
                <div className="canvas-card-turn is-answer">
                  <span className="canvas-card-turn-role">{lastAnswer.agentName || 'Stark'}</span>
                  <span className="canvas-card-turn-text">{lastAnswer.content.slice(0, 600) || 'Working…'}</span>
                </div>
              ) : (
                <div className="canvas-card-empty">No response yet.</div>
              )}
              <div className="canvas-card-section-label">
                <span>Detail</span>
                <span className="canvas-card-section-count">{detailMessageCount}</span>
              </div>
              <div className="canvas-card-meta-grid">
                <span>Model</span><span>{detailChat?.model || chat.model || '—'}</span>
                <span>Messages</span><span>{detailMessageCount}</span>
                <span>Tools</span><span>{toolCalls ?? '—'}</span>
                <span>Tokens</span><span>{tokens ? compactNumber(tokens) : '—'}</span>
                <span>Cost</span><span>{cost ? `$${cost < 0.01 ? cost.toFixed(4) : cost.toFixed(2)}` : '—'}</span>
                <span>Updated</span><span>{timeAgo(chat.updatedAt) || '—'}</span>
              </div>
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
              {visibleAgents.length === 0 && directChildren.length === 0 && hiddenChildren === 0 && (
                <div className="canvas-card-empty">No child agents in this run.</div>
              )}
            </div>
          </div>
        )}

        <form
          data-no-drag
          onSubmit={submit}
          className={`canvas-card-dock ${dragActive ? 'is-drag-active' : ''}`}
          onDragEnter={(event) => {
            if (!Array.from(event.dataTransfer.types || []).includes('Files')) return;
            event.preventDefault();
            event.stopPropagation();
            dragDepth.current += 1;
            setDragActive(true);
          }}
          onDragLeave={(event) => {
            if (!Array.from(event.dataTransfer.types || []).includes('Files')) return;
            event.preventDefault();
            event.stopPropagation();
            dragDepth.current -= 1;
            if (dragDepth.current <= 0) {
              dragDepth.current = 0;
              setDragActive(false);
            }
          }}
          onDragOver={(event) => {
            if (!Array.from(event.dataTransfer.types || []).includes('Files')) return;
            event.preventDefault();
            event.stopPropagation();
            event.dataTransfer.dropEffect = 'copy';
          }}
          onDrop={(event) => {
            if (!event.dataTransfer.files?.length) return;
            event.preventDefault();
            event.stopPropagation();
            dragDepth.current = 0;
            setDragActive(false);
            addFiles(event.dataTransfer.files);
          }}
        >
          <input
            ref={fileRef}
            type="file"
            multiple
            accept="image/*,audio/*,.pdf,.txt,.md,.json,.js,.ts,.tsx,.py,.html,.css,.csv,.doc,.docx,.zip"
            className="hidden"
            onChange={(event) => {
              if (event.target.files) addFiles(event.target.files);
              event.currentTarget.value = '';
            }}
          />
          {dragActive && <DropOverlay compact />}
          {files.length > 0 && (
            <div className="canvas-card-files">
              {files.map((file) => (
                <span className="canvas-card-file" key={`${file.name}-${file.size}`}>
                  {file.type.startsWith('image/') ? <ImageIcon size={9} /> : file.type.startsWith('audio/') ? <AudioLines size={9} /> : <FileText size={9} />}
                  <span>{file.name}</span>
                  <button type="button" onClick={() => setFiles((all) => all.filter((item) => item !== file))} aria-label={`Remove ${file.name}`}>
                    <X size={8} />
                  </button>
                </span>
              ))}
            </div>
          )}
          <div className="canvas-card-input-wrap">
            <button
              type="button"
              className="canvas-card-attach"
              onClick={() => fileRef.current?.click()}
              title="Attach files"
              aria-label="Attach files"
            >
              <Paperclip size={11} />
            </button>
            <input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              disabled={sending}
              placeholder={sending ? 'Sending…' : running ? 'Agent is running — you can type ahead…' : 'Message this chat…'}
              className="canvas-card-input"
              aria-label="Message this chat"
            />
            <button
              type="submit"
              disabled={sending || running || (!draft.trim() && !files.length)}
              className="canvas-card-send"
              aria-label="Send message"
              title={running ? 'Wait for this run to finish' : 'Send message'}
            >
              {sending ? <Loader2 size={11} className="canvas-card-chip-spinner" /> : <ArrowUp size={11} />}
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
