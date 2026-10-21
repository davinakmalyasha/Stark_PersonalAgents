import { useState } from 'react';
import type { FormEvent, ReactNode } from 'react';
import { ArrowUp, ChevronDown, ChevronUp, Clock3, FolderKanban, GripVertical, MessageSquare, Palette, Plus, X } from 'lucide-react';
import type { ChatSummary, GroupProjectRow, WorkspaceInfo } from '../types';
import type { BoardAgent } from '../types/board.types';
import { cleanChatTitle } from '../utils/chatTitle';
import type { CanvasItem } from './canvasTypes';

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
}

const AGENT_DOT: Record<string, string> = {
  idle: 'bg-zinc-600',
  working: 'bg-sky-400',
  completed: 'bg-emerald-400',
  failed: 'bg-rose-400',
};

const GROUP_DOT: Record<string, string> = {
  running: 'bg-sky-400',
  done: 'bg-emerald-400',
  paused: 'bg-violet-400',
  stalled: 'bg-amber-400',
  failed: 'bg-rose-400',
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

export default function CanvasItemCard({
  item, chat, workspace, project, agents, running, elapsed,
  expanded, onToggleExpanded, onOpen, onRemove, onSend,
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

  const header = (icon: ReactNode, title: string, subtitle: string, dot?: string) => (
    <div className="flex items-center gap-2 border-b border-white/[0.06] px-2.5 py-2">
      <GripVertical size={12} className="flex-none cursor-grab text-zinc-700 active:cursor-grabbing" />
      {icon}
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-1.5">
          {dot && <span className={`h-1.5 w-1.5 flex-none rounded-full ${dot} ${running ? 'animate-pulse' : ''}`} />}
          <span className="truncate text-[11.5px] font-semibold text-zinc-200">{title}</span>
        </div>
        <div className="truncate text-[9.5px] text-zinc-600">{subtitle}</div>
      </div>
      {item.kind === 'chat' && (
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onToggleExpanded(); }}
          className="grid h-5 w-5 flex-none place-items-center rounded text-zinc-600 hover:bg-white/[0.07] hover:text-white"
          title={expanded ? 'Collapse' : 'Details'}
        >
          {expanded ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
        </button>
      )}
      <button
        type="button"
        onClick={(e) => { e.stopPropagation(); onRemove(); }}
        className="grid h-5 w-5 flex-none place-items-center rounded text-zinc-700 hover:bg-rose-500/10 hover:text-rose-300"
        title="Remove from canvas"
      >
        <X size={11} />
      </button>
    </div>
  );

  if (item.kind === 'chat') {
    if (!chat || !workspace) {
      return (
        <div data-canvas-card data-item-id={item.id} className="w-[280px] rounded-xl border border-white/[0.08] bg-[#111216]">
          {header(<MessageSquare size={12} className="flex-none text-zinc-600" />, 'Chat unavailable', item.workspaceId, 'bg-zinc-600')}
          <div className="p-2.5 text-[10px] text-zinc-600">This chat was deleted or is no longer available.</div>
        </div>
      );
    }
    // Only show agents that actually spawned here. The fixed roster exists for
    // the agent strip, but a plain Q&A chat should not advertise four idle
    // subagents that never ran.
    const visibleAgents = agents.filter(
      (agent) => agent.role === 'Subagent' && (agent.steps > 0 || agent.status === 'working' || agent.status === 'completed'),
    );
    const activeAgent = visibleAgents.find((a) => a.isActive);
    const currentTask = activeAgent?.task || visibleAgents.find((a) => a.status !== 'idle')?.task || '';
    const statusDot = running ? 'bg-sky-400' : chat.lastRole === 'error' ? 'bg-rose-400' : 'bg-zinc-600';
    return (
      <div
        data-canvas-card
        data-item-id={item.id}
        className={`rounded-xl border bg-[#111216] shadow-[0_14px_35px_rgba(0,0,0,0.35)] ${
          expanded ? 'w-[350px]' : 'w-[280px]'
        } ${running ? 'border-sky-400/35' : 'border-white/[0.08]'}`}
      >
        {header(
          <MessageSquare size={12} className="flex-none text-zinc-500" />,
          cleanChatTitle(chat.title),
          workspace.isGeneral ? 'Main workspace' : workspace.name,
          statusDot,
        )}
        <div className="px-2.5 py-2">
          {running ? (
            <div className="flex items-center gap-1.5 text-[10.5px] text-sky-300">
              <span className="h-1.5 w-1.5 flex-none animate-pulse rounded-full bg-sky-400" />
              <span className="truncate">{currentTask || 'working…'}</span>
              {elapsed && <span className="ml-auto flex-none font-mono text-[9.5px] tabular-nums">{elapsed}</span>}
            </div>
          ) : (
            <div className="line-clamp-2 text-[10.5px] leading-4 text-zinc-500">
              {chat.lastMessage || (chat.lastRole === 'error' ? 'Run failed.' : 'No messages yet.')}
            </div>
          )}
          <div className="mt-1.5 flex items-center gap-2.5 text-[9px] text-zinc-600 tabular-nums">
            <span className="flex items-center gap-1"><MessageSquare size={9} />{chat.messageCount || 0}</span>
            <span className="ml-auto flex items-center gap-1"><Clock3 size={9} />{timeAgo(chat.updatedAt)}</span>
          </div>
        </div>
        {expanded && (
          <div className="border-t border-white/[0.06] px-2.5 pb-2.5 pt-2">
            {visibleAgents.length > 0 && (
              <>
                <div className="mb-1.5 text-[8.5px] font-semibold uppercase tracking-widest text-zinc-600">Subagents</div>
                <div className="space-y-1">
                  {visibleAgents.map((agent) => (
                    <div key={agent.id} className="flex items-center gap-1.5 text-[10px]">
                      <span className={`h-1.5 w-1.5 flex-none rounded-full ${AGENT_DOT[agent.status] || 'bg-zinc-600'} ${agent.isActive ? 'animate-pulse' : ''}`} />
                      <span className="min-w-0 flex-1 truncate text-zinc-400">{agent.label}</span>
                      <span className="max-w-[120px] truncate text-[9px] text-zinc-600">{agent.task || agent.status}</span>
                      <span className="flex-none text-[9px] tabular-nums text-zinc-700">{agent.steps}</span>
                    </div>
                  ))}
                </div>
              </>
            )}
            <button
              type="button"
              onClick={(e) => { e.stopPropagation(); onOpen(); }}
              className="mt-2 flex h-7 w-full items-center justify-center rounded-md bg-white text-[10.5px] font-semibold text-[#0a0a0d] hover:bg-zinc-200"
            >
              Open chat
            </button>
          </div>
        )}
        <form data-no-drag onSubmit={submit} className="border-t border-white/[0.06] p-2">
          <div className="flex items-center gap-1.5 rounded-lg border border-white/[0.08] bg-white/[0.02] px-1.5 py-1 focus-within:border-white/[0.18]">
            <input
              value={draft}
              onChange={(e) => setDraft(e.target.value)}
              disabled={sending || running}
              placeholder={sending ? 'Sending…' : running ? 'Agent working…' : 'Message this chat…'}
              className="min-w-0 flex-1 bg-transparent text-[10.5px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none disabled:opacity-60"
            />
            <button
              type="submit"
              disabled={sending || running || !draft.trim()}
              className="grid h-5 w-5 flex-none place-items-center rounded bg-white text-[#0a0a0d] disabled:bg-white/[0.08] disabled:text-zinc-600"
              aria-label="Send message"
            >
              <ArrowUp size={11} />
            </button>
          </div>
          {sendError && <div className="mt-1 text-[9px] text-rose-400">{sendError}</div>}
        </form>
      </div>
    );
  }

  if (item.kind === 'project') {
    if (!project) {
      return (
        <div data-canvas-card data-item-id={item.id} className="w-[260px] rounded-xl border border-white/[0.08] bg-[#111216]">
          {header(<FolderKanban size={12} className="flex-none text-zinc-600" />, 'Project unavailable', item.projectId)}
          <div className="p-2.5 text-[10px] text-zinc-600">This project is no longer available.</div>
        </div>
      );
    }
    const pct = project.items_total ? Math.round((project.items_done / project.items_total) * 100) : 0;
    return (
      <div
        data-canvas-card
        data-item-id={item.id}
        className={`w-[260px] rounded-xl border bg-[#111216] shadow-[0_14px_35px_rgba(0,0,0,0.35)] ${
          project.awaiting_approval > 0 ? 'border-amber-400/35' : 'border-white/[0.08]'
        }`}
      >
        {header(
          <FolderKanban size={12} className="flex-none text-zinc-500" />,
          project.title || project.id,
          'Project',
          GROUP_DOT[project.status] || 'bg-zinc-600',
        )}
        <button
          type="button"
          onClick={(e) => { e.stopPropagation(); onOpen(); }}
          className="w-full px-2.5 py-2 text-left hover:bg-white/[0.02]"
        >
          <div className="h-1 overflow-hidden rounded-full bg-white/[0.06]">
            <div className="h-full rounded-full bg-zinc-400" style={{ width: `${pct}%` }} />
          </div>
          <div className="mt-1.5 flex items-center justify-between text-[9.5px] text-zinc-600 tabular-nums">
            <span>{project.items_done}/{project.items_total} items</span>
            <span>{project.status}</span>
          </div>
          {project.awaiting_approval > 0 && (
            <div className="mt-1 text-[9px] font-semibold text-amber-300">{project.awaiting_approval} needs you</div>
          )}
        </button>
      </div>
    );
  }

  if (!workspace) {
    return (
      <div data-canvas-card data-item-id={item.id} className="w-[240px] rounded-xl border border-white/[0.08] bg-[#111216]">
        {header(<Palette size={12} className="flex-none text-zinc-600" />, 'Addon unavailable', item.workspaceId)}
      </div>
    );
  }

  return (
    <div data-canvas-card data-item-id={item.id} className="w-[240px] rounded-xl border border-white/[0.08] bg-[#111216] shadow-[0_14px_35px_rgba(0,0,0,0.35)]">
      {header(
        <Palette size={12} className="flex-none text-zinc-500" />,
        item.addonId === 'design' ? 'OpenDesign' : 'Addon',
        workspace.isGeneral ? 'Main workspace' : workspace.name,
        'bg-violet-400',
      )}
      <button
        type="button"
        onClick={(e) => { e.stopPropagation(); onOpen(); }}
        className="flex w-full items-center justify-center gap-1.5 px-2.5 py-2 text-[10px] text-zinc-300 hover:bg-white/[0.03]"
      >
        <Plus size={10} /> Open addon
      </button>
    </div>
  );
}
