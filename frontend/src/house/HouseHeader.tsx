import { useEffect, useRef, useState } from 'react';
import { Menu, Plus, ListTodo, Clock } from 'lucide-react';
import type { AgentTask, SessionTab } from '../types';
import { formatRecurrence } from '../utils/recurrence';
import TaskTabs from './TaskTabs';

interface HouseHeaderProps {
  tabs: SessionTab[];
  activeTabId: string | null;
  runningTabId: string | null;
  onActivateTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onNewSession: () => void;
  onOpenSidebar: () => void;
  tasks: AgentTask[];
  onOpenTasks: () => void;
  onTaskStart: (taskId: string) => void;
  onTaskDelete: (taskId: string) => void;
}

/**
 * Main workspace top bar: hamburger + saved-tasks + session tab strip
 * (opencode-style). The brand logo/name is gone — the whole bar is
 * working space.
 */
export default function HouseHeader({
  tabs, activeTabId, runningTabId,
  onActivateTab, onCloseTab, onNewSession,
  onOpenSidebar, tasks, onOpenTasks, onTaskStart, onTaskDelete,
}: HouseHeaderProps) {
  const isElectron = typeof window !== 'undefined' && !!window.antigravity;
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
      className={`relative z-20 flex items-center gap-2 px-3 sm:px-4 ${isElectron ? 'pr-36' : ''} h-[52px] border-b house-hairline shrink-0`}
      style={{ WebkitAppRegion: 'drag' } as React.CSSProperties}
    >
      <button
        onClick={onOpenSidebar}
        title="Chat history"
        aria-label="Open chat history"
        className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer shrink-0"
        style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}
      >
        <Menu size={16} />
      </button>

      {/* ── Scheduled / saved tasks — left cluster, clear of window controls ── */}
      <div ref={tasksRef} className="relative shrink-0" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <button
          onClick={() => setTasksOpen((v) => !v)}
          title="Saved & scheduled tasks"
          aria-label="Saved and scheduled tasks"
          className={`relative grid place-items-center w-8 h-8 rounded-lg transition-colors cursor-pointer ${
            tasksOpen ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.07]'
          }`}
        >
          <ListTodo size={15} />
          {tasks.length > 0 && (
            <span className="absolute -top-0.5 -right-0.5 grid place-items-center min-w-[14px] h-[14px] px-0.5 rounded-full bg-white text-[#0a0a0d] text-[9px] font-bold tabular-nums">
              {tasks.length}
            </span>
          )}
        </button>

        {/* Dropdown — smooth open/close transition */}
        <div
          className={`absolute left-0 top-[calc(100%+8px)] z-50 w-[330px] max-h-[min(360px,calc(100vh-140px))] overflow-y-auto house-thread rounded-xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl transition-all duration-200 ease-out origin-top-left ${
            tasksOpen
              ? 'opacity-100 translate-y-0 scale-100 pointer-events-auto'
              : 'opacity-0 -translate-y-2 scale-[0.97] pointer-events-none'
          }`}
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
                      {t.schedule?.recurrence && t.schedule.recurrence !== 'none' ? ` · ${formatRecurrence(t.schedule.recurrence)}` : ''}
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

      {/* ── Session tab strip (the top bar IS the workspace switcher) ── */}
      <div className="flex-1 min-w-0 flex items-center" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <TaskTabs
          tabs={tabs}
          activeTabId={activeTabId}
          runningTabId={runningTabId}
          onActivateTab={onActivateTab}
          onCloseTab={onCloseTab}
          onNewSession={onNewSession}
        />
      </div>
    </header>
  );
}
