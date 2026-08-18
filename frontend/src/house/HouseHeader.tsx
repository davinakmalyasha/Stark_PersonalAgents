import { useEffect, useRef, useState } from 'react';
import { Menu, Zap, Settings, Plus, ListTodo, Clock } from 'lucide-react';
import { getAssistantName } from './assistant';
import type { AgentTask } from '../types';

interface HouseHeaderProps {
  timeStr: string;
  isRunning: boolean;
  tasks: AgentTask[];
  onOpenSidebar: () => void;
  onNewChat: () => void;
  onOpenTasks: () => void;
  onTaskStart: (taskId: string) => void;
  onTaskDelete: (taskId: string) => void;
}

export default function HouseHeader({
  timeStr, isRunning, tasks,
  onOpenSidebar, onNewChat, onOpenTasks, onTaskStart, onTaskDelete,
}: HouseHeaderProps) {
  const isElectron = typeof window !== 'undefined' && !!window.antigravity;
  const assistantName = getAssistantName();
  const [tasksOpen, setTasksOpen] = useState(false);
  const tasksRef = useRef<HTMLDivElement | null>(null);

  // Close the task dropdown on outside click / Escape
  useEffect(() => {
    if (!tasksOpen) return;
    const onDown = (e: MouseEvent) => {
      if (tasksRef.current && !tasksRef.current.contains(e.target as Node)) setTasksOpen(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setTasksOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [tasksOpen]);

  return (
    <header
      className={`relative z-20 flex items-center justify-between px-4 sm:px-6 ${isElectron ? 'pr-36' : ''} h-[52px] border-b house-hairline shrink-0`}
      style={{ WebkitAppRegion: 'drag' } as React.CSSProperties}
    >
      <div className="flex items-center gap-2.5" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <button
          onClick={onOpenSidebar}
          title="Chat history"
          aria-label="Open chat history"
          className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
        >
          <Menu size={16} />
        </button>
        <div className="flex items-center gap-2.5 mr-1">
          <div className="grid place-items-center w-7 h-7 rounded-lg bg-white text-[#0a0a0d]">
            <Zap size={14} />
          </div>
          <span className="house-font-display text-[15px] font-bold tracking-wide text-white">
            {assistantName.toUpperCase()}
          </span>
        </div>
        <button
          onClick={onNewChat}
          disabled={isRunning}
          title="New chat"
          className="hidden sm:flex items-center gap-1.5 h-8 px-3 rounded-lg border border-white/10 bg-white/[0.04] text-[12px] text-zinc-300 hover:bg-white/[0.09] hover:border-white/20 disabled:opacity-40 disabled:cursor-not-allowed transition-all cursor-pointer"
        >
          <Plus size={13} />
          New chat
        </button>

        {/* ── Tasks: button + dropdown panel ── */}
        <div ref={tasksRef} className="relative">
          <button
            onClick={() => setTasksOpen((v) => !v)}
            title="Task control — saved, scheduled and triggered tasks"
            className={`hidden sm:flex items-center gap-1.5 h-8 px-3 rounded-lg border text-[12px] transition-all cursor-pointer ${
              tasksOpen
                ? 'border-white/25 bg-white/[0.09] text-white'
                : 'border-white/10 bg-white/[0.04] text-zinc-300 hover:bg-white/[0.09] hover:border-white/20'
            }`}
          >
            <ListTodo size={13} />
            Tasks
            {tasks.length > 0 && (
              <span className="grid place-items-center min-w-[18px] h-[18px] px-1 rounded-full bg-white text-[#0a0a0d] text-[10px] font-semibold tabular-nums">
                {tasks.length}
              </span>
            )}
          </button>

          {/* Dropdown — smooth open/close transition */}
          <div
            className={`absolute right-0 top-[calc(100%+8px)] z-50 w-[330px] max-h-[min(360px,calc(100vh-140px))] overflow-y-auto house-thread rounded-xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl transition-all duration-200 ease-out origin-top-right ${
              tasksOpen
                ? 'opacity-100 translate-y-0 scale-100 pointer-events-auto'
                : 'opacity-0 -translate-y-2 scale-[0.97] pointer-events-none'
            }`}
            style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
          >
            <div className="flex items-center justify-between px-3.5 py-2.5 border-b house-hairline sticky top-0 bg-[#141419]/95 backdrop-blur-xl">
              <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500">Saved tasks</span>
              <button
                onClick={() => { setTasksOpen(false); onOpenTasks(); }}
                className="flex items-center gap-1.5 h-7 px-2.5 rounded-lg bg-white text-[#0a0a0d] text-[11px] font-semibold hover:bg-zinc-200 transition-colors cursor-pointer"
              >
                <Plus size={12} />
                Add task
              </button>
            </div>

            {tasks.length === 0 ? (
              <div className="px-4 py-6 text-center">
                <p className="text-[12px] text-zinc-500">No saved tasks yet.</p>
                <p className="text-[11px] text-zinc-600 mt-1">Schedule something and Stark will handle it.</p>
              </div>
            ) : (
              <div className="flex flex-col gap-2 p-2.5">
                {tasks.map((t) => (
                  <div key={t.id} className="canvas-task-chip !w-full">
                    <span className={`canvas-task-dot ${
                      t.status === 'scheduled' ? 'is-scheduled'
                        : t.status === 'running' ? 'is-running'
                          : t.status === 'failed' ? 'is-failed' : ''
                    }`} />
                    <div className="canvas-task-chip-copy">
                      <b>{t.prompt}</b>
                      <small className="flex items-center gap-1">
                        {t.schedule?.nextRunAt ? (
                          <>
                            <Clock size={9} />
                            {new Date(t.schedule.nextRunAt).toLocaleString()}
                          </>
                        ) : (
                          t.status
                        )}
                        {t.schedule?.recurrence && t.schedule.recurrence !== 'none' ? ` · ${t.schedule.recurrence}` : ''}
                      </small>
                    </div>
                    {t.status === 'ready' && (
                      <button onClick={() => onTaskStart(t.id)} title="Run now">Run</button>
                    )}
                    <button
                      className="canvas-task-delete"
                      onClick={() => onTaskDelete(t.id)}
                      title="Delete task"
                    >
                      ×
                    </button>
                  </div>
                ))}
              </div>
            )}
          </div>
        </div>
      </div>

      <div
        className="flex items-center gap-4 text-[11px] text-zinc-500"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
      >
        <span className="text-zinc-400 tabular-nums font-medium">{timeStr}</span>
        <button className="hover:text-zinc-300 transition-colors cursor-pointer" title="Settings">
          <Settings size={14} className="text-zinc-600 hover:text-zinc-300" />
        </button>
      </div>
    </header>
  );
}
