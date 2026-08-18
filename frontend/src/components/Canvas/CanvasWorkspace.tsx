import { useEffect, useState } from 'react';
import type { DragEvent } from 'react';
import { CalendarClock, Clock3, Loader2, Paperclip, Pencil, Play, Plus, Timer, Trash2 } from 'lucide-react';
import type { AgentTask } from '../../types';
import type { BoardAgent } from '../../types/board.types';
import AddTaskModal from './AddTaskModal';
import { COLUMN_ORDER, agentStatusDot, columnConfig } from './board.styles';

interface Props {
  tasks: AgentTask[];
  agents: BoardAgent[];
  isRunning: boolean;
  runStartedAt: number | null;
  selectedAgent: string | null;
  onSelectAgent: (id: string | null) => void;
  onStartTask: (task: AgentTask) => void;
  onEditTask: (task: AgentTask) => void;
  onDeleteTask: (task: AgentTask) => void;
  onStatusChange: (task: AgentTask, status: AgentTask['status']) => void;
  onOpenTaskModal: () => void;
  taskModalOpen: boolean;
  editingTask: AgentTask | null;
  onCloseTaskModal: () => void;
  onTaskCreated: (task: AgentTask) => void;
  onTaskUpdated: (task: AgentTask) => void;
  onStartNow: (prompt: string, files: File[]) => Promise<void> | void;
  workspaceId: string | null;
  chatId: string | null;
}

function useNow(active: boolean): number {
  const [now, setNow] = useState(0);
  useEffect(() => {
    if (!active) return;
    const id = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(id);
  }, [active]);
  return now;
}

function formatClock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  const pad = (n: number) => String(n).padStart(2, '0');
  return h > 0 ? `${h}:${pad(m)}:${pad(sec)}` : `${pad(m)}:${pad(sec)}`;
}

function formatElapsed(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = s % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${sec}s`;
  return `${sec}s`;
}

function timeAgo(iso?: string): string {
  if (!iso) return '';
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return '';
  const diff = Math.max(0, Date.now() - then);
  const s = Math.floor(diff / 1000);
  if (s < 60) return `${s}s ago`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  return `${Math.floor(h / 24)}d ago`;
}

function durationBetween(from?: string, to?: string): string | null {
  if (!from || !to) return null;
  const a = new Date(from).getTime();
  const b = new Date(to).getTime();
  if (Number.isNaN(a) || Number.isNaN(b)) return null;
  return formatElapsed(b - a);
}

function TaskCard({ task, runStartedAt, now, onStartTask, onEditTask, onDeleteTask }: {
  task: AgentTask;
  runStartedAt: number | null;
  now: number;
  onStartTask: (task: AgentTask) => void;
  onEditTask: (task: AgentTask) => void;
  onDeleteTask: (task: AgentTask) => void;
}) {
  const meta = columnConfig[task.status] || columnConfig.ready;
  const running = task.status === 'running';
  const startedAt = runStartedAt ?? (task.lastRunAt ? new Date(task.lastRunAt).getTime() : now);

  return (
    <div
      draggable
      onDragStart={(e: DragEvent) => {
        e.dataTransfer.setData('text/plain', task.id);
        e.dataTransfer.effectAllowed = 'move';
      }}
      className="group cursor-grab active:cursor-grabbing rounded-[12px] bg-[#191a1f] border border-white/[0.08] shadow-[0_10px_24px_rgba(0,0,0,0.18)] hover:border-white/[0.16] hover:shadow-[0_14px_30px_rgba(0,0,0,0.26)] transition-all duration-200 select-none overflow-hidden"
    >
      <div className="h-[3px] w-full" style={{ backgroundColor: meta.color }} />
      <div className="p-3">
        <div className="flex items-start justify-between gap-2">
          <p className="text-[12px] font-medium text-zinc-200 leading-snug line-clamp-2 break-words">{task.prompt || 'Attachment task'}</p>
          <span className={`flex-none grid place-items-center w-6 h-6 rounded-md ${meta.bgClass}`}>
            <span className={`h-2 w-2 rounded-full ${meta.dotClass} ${running ? 'animate-pulse' : ''}`} />
          </span>
        </div>

        <div className="mt-2.5 space-y-1">
          {running ? (
            <div className="flex items-center gap-1.5 text-[10px] text-sky-300 font-mono">
              <Loader2 size={10} className="animate-spin" />
              <span>{formatClock(now - startedAt)}</span>
              <span className="text-zinc-500">elapsed</span>
            </div>
          ) : task.status === 'scheduled' && task.schedule?.nextRunAt ? (
            <div className="flex items-center gap-1.5 text-[10px] text-purple-300">
              <CalendarClock size={10} />
              <span className="truncate">Runs {new Date(task.schedule.nextRunAt).toLocaleString()}</span>
              {task.schedule.recurrence !== 'none' && (
                <span className="px-1.5 py-px rounded bg-purple-400/15 border border-purple-400/25 text-[8px] font-semibold uppercase tracking-wide">
                  {task.schedule.recurrence}
                </span>
              )}
            </div>
          ) : task.status === 'completed' || task.status === 'failed' ? (
            <div className="flex items-center gap-1.5 text-[10px] text-emerald-300">
              <Timer size={10} />
              <span>{durationBetween(task.lastRunAt, task.completedAt) ?? '—'} taken</span>
            </div>
          ) : (
            <div className="flex items-center gap-1.5 text-[10px] text-zinc-500">
              <Clock3 size={10} />
              <span>Created {timeAgo(task.createdAt)}</span>
            </div>
          )}
          <div className="flex items-center gap-1.5 text-[10px] text-zinc-500">
            <Clock3 size={10} />
            <span>{running ? `Started ${timeAgo(task.lastRunAt)}` : `Updated ${timeAgo(task.updatedAt)}`}</span>
          </div>
          {task.attachments.length > 0 && (
            <div className="flex items-center gap-1.5 text-[10px] text-zinc-500">
              <Paperclip size={10} />
              <span>{task.attachments.length} attached</span>
            </div>
          )}
        </div>

        <div className="mt-2.5 pt-2 border-t border-white/[0.06] flex items-center justify-end gap-1.5 opacity-0 group-hover:opacity-100 transition-opacity">
          {!running && (
            <button
              type="button"
              data-interactive="true"
              onClick={() => onStartTask(task)}
              className="grid place-items-center w-6 h-6 rounded-md bg-white text-[#111214] hover:bg-zinc-300 transition-colors cursor-pointer"
              aria-label="Run task"
              title="Run task"
            >
              <Play size={11} />
            </button>
          )}
          <button
            type="button"
            data-interactive="true"
            onClick={() => onEditTask(task)}
            className="grid place-items-center w-6 h-6 rounded-md text-zinc-400 hover:text-white hover:bg-white/[0.09] transition-colors cursor-pointer"
            aria-label="Edit task"
            title="Edit task"
          >
            <Pencil size={11} />
          </button>
          <button
            type="button"
            data-interactive="true"
            onClick={() => onDeleteTask(task)}
            className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-red-300 hover:bg-red-500/10 transition-colors cursor-pointer"
            aria-label="Delete task"
            title="Delete task"
          >
            <Trash2 size={11} />
          </button>
        </div>
      </div>
    </div>
  );
}

export default function CanvasWorkspace({
  tasks, agents, isRunning, runStartedAt, selectedAgent, onSelectAgent,
  onStartTask, onEditTask, onDeleteTask, onStatusChange,
  onOpenTaskModal, taskModalOpen, editingTask, onCloseTaskModal, onTaskCreated, onTaskUpdated,
  onStartNow, workspaceId, chatId,
}: Props) {
  const [dragOverCol, setDragOverCol] = useState<string | null>(null);
  const [draggingId, setDraggingId] = useState<string | null>(null);
  const now = useNow(isRunning);

  const handleDrop = (status: string) => (e: DragEvent) => {
    e.preventDefault();
    setDragOverCol(null);
    const id = e.dataTransfer.getData('text/plain') || draggingId || '';
    const task = tasks.find((t) => t.id === id);
    if (task && task.status !== status) onStatusChange(task, status as AgentTask['status']);
    setDraggingId(null);
  };

  return (
    <div className="relative flex-1 h-full min-w-0 overflow-hidden bg-[#101114]">
      <div className="board-scroll absolute inset-0 overflow-x-auto overflow-y-hidden px-4 pb-[76px]">
        <div className="flex h-full min-w-max items-stretch gap-3 pt-3">
          {COLUMN_ORDER.map((status) => {
            const meta = columnConfig[status];
            const columnTasks = tasks.filter((t) => t.status === status);
            const isDropTarget = status !== 'running';
            const isOver = dragOverCol === status;

            return (
              <section
                key={status}
                className={`flex flex-col w-[264px] flex-none rounded-2xl border border-white/[0.06] bg-white/[0.02] backdrop-blur-sm transition-colors duration-150 ${
                  isOver ? 'border-white/[0.18] bg-white/[0.05]' : ''
                }`}
                onDragOver={(e: DragEvent) => {
                  if (!isDropTarget) return;
                  e.preventDefault();
                  e.dataTransfer.dropEffect = 'move';
                  setDragOverCol(status);
                }}
                onDragLeave={() => { if (dragOverCol === status) setDragOverCol(null); }}
                onDrop={isDropTarget ? handleDrop(status) : undefined}
              >
                <header className="flex items-center gap-2 px-3.5 py-2.5 flex-shrink-0">
                  <span className={`h-2 w-2 rounded-full ${meta.dotClass} ${status === 'running' ? 'animate-pulse' : ''}`} />
                  <h2 className={`text-[11px] font-bold uppercase tracking-[0.12em] ${meta.textClass}`}>{meta.title}</h2>
                  <span className="ml-auto grid place-items-center min-w-[20px] h-[20px] px-1 rounded-full bg-white/[0.07] text-[10px] font-semibold text-zinc-400 tabular-nums">
                    {columnTasks.length}
                  </span>
                  <button
                    type="button"
                    data-interactive="true"
                    onClick={onOpenTaskModal}
                    className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.09] transition-colors cursor-pointer"
                    aria-label={`Add task to ${meta.title}`}
                    title="Add task"
                  >
                    <Plus size={13} />
                  </button>
                </header>
                <div className="board-scroll flex-1 min-h-0 overflow-y-auto px-2.5 pb-2.5 space-y-2">
                  {columnTasks.map((task) => (
                    <TaskCard
                      key={task.id}
                      task={task}
                      runStartedAt={runStartedAt}
                      now={now}
                      onStartTask={onStartTask}
                      onEditTask={onEditTask}
                      onDeleteTask={onDeleteTask}
                    />
                  ))}
                  {columnTasks.length === 0 && (
                    <div className="grid place-items-center h-24 rounded-xl border border-dashed border-white/[0.07] text-[10px] text-zinc-600">
                      {status === 'running' ? 'Nothing running right now' : 'Drop tasks here'}
                    </div>
                  )}
                </div>
              </section>
            );
          })}
        </div>
      </div>

      {/* Agent strip */}
      <div className="absolute bottom-0 left-0 right-0 z-20 px-4 pb-3">
        <div className="flex items-center gap-2 overflow-x-auto rounded-2xl border border-white/[0.08] bg-[#16171c]/95 backdrop-blur-xl shadow-[0_-8px_30px_rgba(0,0,0,0.25)] px-3 py-2.5">
          <span className="flex-none text-[9px] font-bold uppercase tracking-[0.16em] text-zinc-500 mr-1">Agents</span>
          {agents.map((agent) => {
            const isSelected = selectedAgent === agent.id;
            const isActive = agent.isActive;
            const agentElapsed = isActive && runStartedAt ? now - runStartedAt : null;
            return (
              <button
                key={agent.id}
                type="button"
                data-interactive="true"
                onClick={() => onSelectAgent(isSelected ? null : agent.id)}
                className={`flex items-center gap-2.5 flex-none min-w-0 rounded-xl border px-2.5 py-2 transition-all duration-150 cursor-pointer text-left ${
                  isSelected
                    ? 'border-white/[0.22] bg-white/[0.07]'
                    : isActive
                      ? 'border-sky-400/40 bg-sky-400/[0.06]'
                      : 'border-white/[0.06] bg-white/[0.02] hover:bg-white/[0.05]'
                }`}
                title={`${agent.label} — ${agent.task || (isActive ? 'working' : 'idle')}`}
              >
                <span className={`flex-none h-2 w-2 rounded-full ${agentStatusDot[agent.status] || 'bg-zinc-500'} ${isActive ? 'animate-pulse' : ''}`} />
                <span className="min-w-0">
                  <span className={`block text-[11px] font-semibold truncate ${isActive ? 'text-sky-200' : 'text-zinc-200'}`}>{agent.label}</span>
                  <span className="block max-w-[180px] truncate text-[9px] text-zinc-500">
                    {agent.task || (isActive ? 'working…' : 'idle')}
                  </span>
                </span>
                {agentElapsed !== null ? (
                  <span className="flex-none text-[10px] font-mono text-sky-300 tabular-nums">{formatClock(agentElapsed)}</span>
                ) : (
                  <span className="flex-none text-[9px] font-mono text-zinc-600 tabular-nums">{agent.steps} steps</span>
                )}
              </button>
            );
          })}
          {agents.length === 0 && <span className="text-[11px] text-zinc-600">No agents registered</span>}
        </div>
      </div>

      {taskModalOpen && <AddTaskModal workspaceId={workspaceId} chatId={chatId} isRunning={isRunning} task={editingTask} onClose={onCloseTaskModal} onCreated={onTaskCreated} onUpdated={onTaskUpdated} onStartNow={onStartNow} />}
    </div>
  );
}