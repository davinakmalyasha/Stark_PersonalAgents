import { useEffect, useRef, useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import {
  ArrowUp, AudioLines, Check, ChevronRight, Clock3, FileText, FolderKanban,
  GripVertical, Image as ImageIcon, Loader2, MessageSquare, Network, Paperclip,
  Palette, Plus, X,
} from 'lucide-react';
import { DropOverlay } from '../components/Workspace/AttachmentDrop';
import AskQuestionPanel from '../components/Workspace/AskQuestionPanel';
import type { ChatMessage, ChatPreviewMessage, ChatSummary, GroupProjectRow, WorkspaceInfo } from '../types';
import { isInternalTransport } from '../utils/streamPresentation';
import type { BoardAgent } from '../types/board.types';
import { cleanChatTitle } from '../utils/chatTitle';
import type { CanvasItem } from './canvasTypes';
import { displayActor } from '../house/group/groupProjectUi';
import { resolveChatActor } from './canvasActors';
import CanvasChatBubble from './CanvasChatBubble';
import { dismissAskBatch, useAskBatchForChat } from '../hooks/usePendingAsk';

interface Props {
  item: CanvasItem;
  chat?: ChatSummary;
  workspace?: WorkspaceInfo;
  project?: GroupProjectRow;
  agents: BoardAgent[];
  running: boolean;
  elapsed?: string;
  onOpen: () => void;
  onRemove: () => void;
  onSend?: (text: string, files: File[]) => Promise<void> | void;
  onRewind?: (message: ChatPreviewMessage) => Promise<void> | void;
  /** Live thread from the full chat while this card is the running session. */
  liveMessages?: ChatMessage[];
  /** Live map context. */
  compact?: boolean;
  isActive?: boolean;
  childChats?: ChatSummary[];
  hiddenChildren?: number;
  onOpenChild?: (workspaceId: string, chatId: string) => void;
  onCloseTab?: () => void;
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

function chatStatus(chat: ChatSummary, running: boolean): CardStatus {
  if (running) return 'running';
  if (chat.status) return chat.status;
  if (chat.lastKind === 'aborted') return 'aborted';
  if (chat.lastRole === 'error') return 'failed';
  if (chat.lastRole === 'user') return 'idle';
  if (chat.lastRole === 'agent') return 'done';
  return 'idle';
}

function childStatus(chat: ChatSummary): CardStatus {
  if (chat.status) return chat.status;
  if (chat.lastKind === 'aborted') return 'aborted';
  if (chat.lastRole === 'error') return 'failed';
  if (chat.lastRole === 'agent') return 'done';
  return 'idle';
}

function StatusIcon({ status }: { status: CardStatus }) {
  if (status === 'running') return <Loader2 size={12} className="canvas-card-status-spin" />;
  if (status === 'queued') return <Clock3 size={11} />;
  if (status === 'done') return <Check size={12} strokeWidth={2.6} />;
  if (status === 'failed' || status === 'aborted') return <X size={12} strokeWidth={2.6} />;
  return <span className="canvas-card-status-dot" />;
}

function StatusBadge({ status }: { status: CardStatus }) {
  return (
    <span
      className={`canvas-card-status is-${status}`}
      data-status={status}
      role="status"
      title={STATUS_LABEL[status]}
      aria-label={`Status: ${STATUS_LABEL[status]}`}
    >
      <StatusIcon status={status} />
    </span>
  );
}

type CardMessage = ChatPreviewMessage & { pending?: string; live?: boolean };

interface HeaderAgents {
  workspace: WorkspaceInfo;
  chats: ChatSummary[];
  hidden: number;
  onOpenChat: (chatId: string) => void;
}

export default function CanvasItemCard({
  item, chat, workspace, project, agents, running, elapsed,
  onOpen, onRemove, onSend, onRewind, liveMessages,
  compact = false, isActive = false, childChats = [], hiddenChildren = 0,
  onOpenChild, onCloseTab,
}: Props) {
  const [draft, setDraft] = useState('');
  const [files, setFiles] = useState<File[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const dragDepth = useRef(0);
  const fileRef = useRef<HTMLInputElement | null>(null);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState('');
  const [optimistic, setOptimistic] = useState<Array<{ id: string; text: string; ts: number }>>([]);
  const [agentsOpen, setAgentsOpen] = useState(false);
  const agentsMenuRef = useRef<HTMLDivElement | null>(null);
  const askBatch = useAskBatchForChat(item.kind === 'chat' ? chat?.id : null);

  useEffect(() => {
    if (!agentsOpen) return;
    const onDown = (event: PointerEvent) => {
      if (!agentsMenuRef.current?.contains(event.target as Node)) setAgentsOpen(false);
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setAgentsOpen(false);
    };
    document.addEventListener('pointerdown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('pointerdown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [agentsOpen]);

  const addFiles = (incoming: FileList | File[]) => {
    const valid = Array.from(incoming).filter((file) => file.size <= 20 * 1024 * 1024);
    setFiles((current) => [...current, ...valid.filter((file) => !current.some((entry) => entry.name === file.name && entry.size === file.size))].slice(0, 8));
    if (valid.length !== Array.from(incoming).length) setSendError('Files must be 20 MB or smaller.');
  };

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    const text = draft.trim();
    if ((!text && !files.length) || sending || running || !onSend) return;
    const body = text || `Attached ${files.length} file${files.length === 1 ? '' : 's'}`;
    const pending = { id: `opt-${Date.now()}`, text: body, ts: Date.now() };
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

  const rewind = async (message: ChatPreviewMessage) => {
    if (!onRewind) return;
    try {
      await onRewind(message);
    } catch (err) {
      setSendError(err instanceof Error ? err.message : 'Rewind failed.');
    }
  };

  const closeLabel = item.kind === 'chat' && item.auto && item.isTab && onCloseTab
    ? 'Close tab'
    : 'Remove from canvas';
  const close = item.kind === 'chat' && item.auto && item.isTab && onCloseTab
    ? onCloseTab
    : onRemove;

  const header = (
    icon: ReactNode,
    title: string,
    subtitle: string,
    status: CardStatus,
    titleOpen?: () => void,
    headerAgents?: HeaderAgents,
  ) => {
    const totalAgents = headerAgents ? headerAgents.chats.length + headerAgents.hidden : 0;
    return (
      <div className="canvas-card-head" data-canvas-drag-handle>
        <GripVertical size={13} className="canvas-card-grip" />
        <span className="canvas-card-type-icon">{icon}</span>
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
        <div className="canvas-card-head-actions">
          <StatusBadge status={status} />
          {headerAgents && (
            <div className="canvas-card-agents-wrap" ref={agentsMenuRef}>
              <button
                type="button"
                className={`canvas-card-icon-button is-agents ${totalAgents > 0 ? 'has-agents' : ''}`}
                aria-label={`Connected agents (${totalAgents})`}
                aria-haspopup="menu"
                aria-expanded={agentsOpen}
                title={`Connected agents${totalAgents ? ` (${totalAgents})` : ''}`}
                onClick={(e) => {
                  e.stopPropagation();
                  setAgentsOpen((open) => !open);
                }}
              >
                <Network size={14} />
                {totalAgents > 0 && <span className="canvas-card-agents-count">{totalAgents}</span>}
              </button>
              {agentsOpen && (
                <div
                  className="canvas-card-agent-menu"
                  role="menu"
                  onPointerDown={(e) => e.stopPropagation()}
                >
                  <div className="canvas-card-agent-menu-title">Connected agents</div>
                  {headerAgents.chats.map((child) => {
                    const childActor = resolveChatActor(headerAgents.workspace, child);
                    const statusLabel = childStatus(child);
                    return (
                      <button
                        key={child.id}
                        type="button"
                        role="menuitem"
                        onClick={() => {
                          setAgentsOpen(false);
                          headerAgents.onOpenChat(child.id);
                        }}
                      >
                        <span className="canvas-card-agent-menu-avatar">{childActor.letter}</span>
                        <span className="canvas-card-agent-menu-copy">
                          <span className="canvas-card-agent-menu-name">{cleanChatTitle(child.title)}</span>
                          <span className={`canvas-card-agent-menu-meta is-${statusLabel}`}>
                            {STATUS_LABEL[statusLabel]} · {timeAgo(child.updatedAt)}
                          </span>
                        </span>
                        <ChevronRight size={11} className="canvas-card-agent-menu-arrow" />
                      </button>
                    );
                  })}
                  {headerAgents.chats.length === 0 && headerAgents.hidden === 0 && (
                    <div className="canvas-card-agent-menu-empty">No connected agents yet.</div>
                  )}
                  {headerAgents.hidden > 0 && (
                    <div className="canvas-card-agent-menu-empty">
                      +{headerAgents.hidden} more not shown on the canvas
                    </div>
                  )}
                </div>
              )}
            </div>
          )}
          <button
            type="button"
            onClick={(e) => { e.stopPropagation(); close(); }}
            className="canvas-card-icon-button is-close"
            title={closeLabel}
            aria-label={closeLabel}
          >
            <X size={14} />
          </button>
        </div>
      </div>
    );
  };

  if (item.kind === 'chat') {
    if (!chat || !workspace) {
      return (
        <div data-canvas-card data-item-id={item.id} className={`canvas-card is-unavailable ${compact ? 'is-compact' : ''}`}>
          {header(<MessageSquare size={16} />, 'Chat unavailable', item.workspaceId, 'idle')}
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
    const actor = resolveChatActor(workspace, chat);
    const ActorIcon = actor.icon;

    const baseMessages: CardMessage[] = [];
    if (chat.lastUser) baseMessages.push({ ...chat.lastUser });
    if (chat.lastAgent) baseMessages.push({ ...chat.lastAgent });
    if (!baseMessages.length && chat.lastMessage && !isInternalTransport(chat.lastMessage)) {
      const role = chat.lastRole === 'error' ? 'error' : chat.lastRole === 'agent' ? 'agent' : 'user';
      baseMessages.push({
        id: `fallback:${chat.id}`,
        role,
        content: chat.lastMessage,
        timestamp: chat.updatedAt,
      });
    }
    baseMessages.sort((left, right) => new Date(left.timestamp).getTime() - new Date(right.timestamp).getTime());

    const messages: CardMessage[] = [
      ...baseMessages,
      ...optimistic.map((row) => ({
        id: row.id,
        role: 'user',
        content: row.text,
        timestamp: new Date(row.ts).toISOString(),
        pending: sending ? 'Sending…' : running ? 'Queued' : 'Sent',
      })),
    ];
    const lastMessage = messages[messages.length - 1];
    const awaitingReply = running && (!lastMessage || lastMessage.role === 'user');
    if (awaitingReply) {
      const liveAgent = [...(liveMessages ?? [])].reverse().find(
        (message) => message.role === 'agent'
          && (message.metadata?.isStreaming || message.id.includes('live'))
          && message.content.trim(),
      );
      messages.push({
        id: `live:${chat.id}`,
        role: 'agent',
        content: liveAgent?.content?.trim() || currentTask || 'Working…',
        timestamp: new Date().toISOString(),
        live: true,
      });
    }

    return (
      <div
        data-canvas-card
        data-item-id={item.id}
        className={[
          'canvas-card',
          'is-chat',
          compact ? 'is-compact' : '',
          running ? 'is-running' : '',
          isActive ? 'is-active' : '',
        ].filter(Boolean).join(' ')}
      >
        <span className="canvas-card-port is-top" aria-hidden="true" />
        {header(
          <ActorIcon size={16} />,
          cleanChatTitle(chat.title),
          actor.label,
          status,
          onOpen,
          { workspace, chats: directChildren, hidden: hiddenChildren, onOpenChat: (chatId) => onOpenChild?.(workspace.id, chatId) },
        )}

        <div className="canvas-card-conversation">
          {messages.length === 0 ? (
            <div className="canvas-card-conversation-empty">No messages yet.</div>
          ) : messages.map((message) => (
            <CanvasChatBubble
              key={message.id}
              message={message}
              actor={actor}
              pending={message.live ? elapsed : message.pending}
              live={message.live}
              onRewind={message.role === 'user' && !message.pending && onRewind
                ? (target) => { void rewind(target); }
                : undefined}
            />
          ))}
        </div>

        {askBatch && !isActive ? (
          <div className="canvas-card-dock canvas-card-ask">
            <AskQuestionPanel
              key={askBatch.batch_id}
              batch={askBatch}
              compact
              onClose={() => dismissAskBatch(askBatch.batch_id)}
            />
          </div>
        ) : (
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
                    <button type="button" onClick={() => setFiles((all) => all.filter((entry) => entry !== file))} aria-label={`Remove ${file.name}`}>
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
                <Paperclip size={12} />
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
                {sending ? <Loader2 size={12} className="canvas-card-status-spin" /> : <ArrowUp size={12} />}
              </button>
            </div>
            {sendError && <div className="canvas-card-error">{sendError}</div>}
          </form>
        )}
        <span className="canvas-card-port is-bottom" aria-hidden="true" />
      </div>
    );
  }

  if (item.kind === 'project') {
    if (!project) {
      return (
        <div data-canvas-card data-item-id={item.id} className="canvas-card is-unavailable">
          {header(<FolderKanban size={16} />, 'Project unavailable', item.projectId, 'idle')}
          <div className="canvas-card-empty">This project is no longer available.</div>
        </div>
      );
    }
    const pct = project.items_total ? Math.round((project.items_done / project.items_total) * 100) : 0;
    const status = GROUP_STATUS[project.status] || 'idle';
    return (
      <div data-canvas-card data-item-id={item.id} className={`canvas-card ${project.awaiting_approval > 0 ? 'is-waiting' : ''}`}>
        <span className="canvas-card-port is-top" aria-hidden="true" />
        {header(
          <FolderKanban size={16} />,
          project.title || project.id,
          `Project · ${project.items_done}/${project.items_total} items`,
          status,
          onOpen,
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
        <span className="canvas-card-port is-bottom" aria-hidden="true" />
      </div>
    );
  }

  if (!workspace) {
    return (
      <div data-canvas-card data-item-id={item.id} className="canvas-card is-unavailable">
        {header(<Palette size={16} />, 'Addon unavailable', item.workspaceId, 'idle')}
      </div>
    );
  }

  return (
    <div data-canvas-card data-item-id={item.id} className="canvas-card">
      <span className="canvas-card-port is-top" aria-hidden="true" />
      {header(
        <Palette size={16} />,
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
      <span className="canvas-card-port is-bottom" aria-hidden="true" />
    </div>
  );
}
