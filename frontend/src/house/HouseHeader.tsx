import { useCallback, useEffect, useRef, useState } from 'react';
import { Menu, Plus, ListTodo, Clock, FolderKanban, CircleAlert, Terminal } from 'lucide-react';
import type { AgentTask, SessionTab, GroupProjectRow } from '../types';
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
  /** Durable projects visible from the active workspace. */
  projects: GroupProjectRow[];
  /** True when the list is filtered to a coding workspace — said out loud in
   *  the panel, so a short list never looks like a bug. */
  projectsScoped: boolean;
  onOpenProject: (projectId: string) => void;
}

/** Close a dropdown on outside-click or Escape. */
function useDismiss(open: boolean, close: () => void,
                    ref: React.RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') close();
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close, ref]);
}

/** The dot state. Waiting outranks running: if a project needs a human, that
 *  is the one thing about it worth surfacing at a glance. */
function projectDotClass(status: string, awaiting: number): string {
  if (awaiting > 0) return 'is-waiting';
  switch (status) {
    case 'running': return 'is-running';
    case 'done': return 'is-done';
    case 'paused': return 'is-paused';
    case 'stalled': return 'is-stalled';
    case 'failed': return 'is-failed';
    default: return '';
  }
}

/**
 * Main workspace top bar: hamburger + saved-tasks + projects + session tab
 * strip (opencode-style). The brand logo/name is gone — the whole bar is
 * working space.
 */
export default function HouseHeader({
  tabs, activeTabId, runningTabId,
  onActivateTab, onCloseTab, onNewSession,
  onOpenSidebar, tasks, onOpenTasks, onTaskStart, onTaskDelete,
  projects, projectsScoped, onOpenProject,
}: HouseHeaderProps) {
  const isElectron = typeof window !== 'undefined' && !!window.antigravity;
  const [tasksOpen, setTasksOpen] = useState(false);
  const [projectsOpen, setProjectsOpen] = useState(false);
  const tasksRef = useRef<HTMLDivElement | null>(null);
  const projectsRef = useRef<HTMLDivElement | null>(null);

  useDismiss(tasksOpen, useCallback(() => setTasksOpen(false), []), tasksRef);
  useDismiss(projectsOpen, useCallback(() => setProjectsOpen(false), []), projectsRef);

  const waiting = projects.reduce((n, p) => n + (p.awaiting_approval || 0), 0);

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
          aria-expanded={tasksOpen}
          aria-haspopup="true"
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

      {/* ── Group projects — durable work that outlives this chat ── */}
      <div ref={projectsRef} className="relative shrink-0" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <button
          onClick={() => setProjectsOpen((v) => !v)}
          title="Projects"
          aria-label="Durable projects"
          aria-expanded={projectsOpen}
          aria-haspopup="true"
          className={`relative grid place-items-center w-8 h-8 rounded-lg transition-colors cursor-pointer ${
            projectsOpen ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.07]'
          }`}
        >
          <FolderKanban size={15} />
          {projects.length > 0 && (
            <span className={`absolute -top-0.5 -right-0.5 grid place-items-center min-w-[14px] h-[14px] px-0.5 rounded-full text-[9px] font-bold tabular-nums ${
              waiting > 0 ? 'bg-amber-400 text-[#0a0a0d]' : 'bg-white text-[#0a0a0d]'
            }`}>
              {projects.length}
            </span>
          )}
        </button>

        {/* One atomic announcement rather than a live region per row: a bare
            number is meaningless read aloud, and competing regions are worse. */}
        <span className="sr-only" role="status" aria-atomic="true">
          {projects.length} project{projects.length === 1 ? '' : 's'}
          {waiting > 0 ? `, ${waiting} waiting on you` : ''}
        </span>

        <div
          className={`absolute left-0 top-[calc(100%+8px)] z-50 w-[330px] max-h-[min(420px,calc(100vh-140px))] overflow-y-auto house-thread rounded-xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl transition-all duration-200 ease-out origin-top-left ${
            projectsOpen
              ? 'opacity-100 translate-y-0 scale-100 pointer-events-auto'
              : 'opacity-0 -translate-y-2 scale-[0.97] pointer-events-none'
          }`}
        >
          <div className="flex items-center justify-between gap-2 px-3.5 py-2.5 border-b house-hairline sticky top-0 bg-[#141419]/95 backdrop-blur-xl">
            <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500">Projects</span>
            {projectsScoped && (
              <span className="text-[10px] text-zinc-600 truncate" title="Projects started from this workspace">
                this workspace
              </span>
            )}
          </div>

          {projects.length === 0 ? (
            <div className="px-4 py-6 text-center">
              <p className="text-[12px] text-zinc-500">
                {projectsScoped ? 'No projects from this workspace.' : 'No projects yet.'}
              </p>
              <p className="text-[11px] text-zinc-600 mt-1">
                {projectsScoped
                  ? 'Projects started here will appear here.'
                  : 'Ask Stark to run something long-running.'}
              </p>
            </div>
          ) : (
            <div className="flex flex-col gap-2 p-2.5">
              {projects.map((p) => (
                <button
                  key={p.id}
                  type="button"
                  onClick={() => { setProjectsOpen(false); onOpenProject(p.id); }}
                  className="canvas-task-chip gp-row"
                  title={p.title || p.id}
                >
                  <span
                    className={`canvas-task-dot ${projectDotClass(p.status, p.awaiting_approval)}`}
                    aria-hidden="true"
                  />
                  <span className="canvas-task-chip-copy">
                    <b>{p.title || p.id}</b>
                    <small>
                      milestones {p.milestones_done}/{p.milestones_total}
                      {' · '}items {p.items_done}/{p.items_total}
                      {typeof p.spent_usd === 'number' && p.spent_usd > 0
                        ? ` · $${p.spent_usd.toFixed(2)}`
                        : ''}
                    </small>
                  </span>
                  {p.shell_enabled && (
                    <span
                      className="gp-shell-badge"
                      title="Members of this project can run shell commands, so the rest of its envelope is advisory"
                    >
                      <Terminal size={9} aria-hidden="true" />
                      shell
                    </span>
                  )}
                  {p.awaiting_approval > 0 && (
                    <span className="gp-waiting-badge">
                      <CircleAlert size={9} aria-hidden="true" />
                      {p.awaiting_approval}
                    </span>
                  )}
                </button>
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
