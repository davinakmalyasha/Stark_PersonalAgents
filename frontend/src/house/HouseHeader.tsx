import { useCallback, useEffect, useRef, useState } from 'react';
import { Menu, FolderKanban, CircleAlert, Terminal, Plus, CalendarClock, X } from 'lucide-react';
import type { SessionTab, GroupProjectRow, ScheduledTask } from '../types';
import SessionTabs from './SessionTabs';
import WindowControls from '../components/Layout/WindowControls';
import { displayActor, timeAgo } from './group/groupProjectUi';

interface HouseHeaderProps {
  tabs: SessionTab[];
  activeTabId: string | null;
  runningTabId: string | null;
  onActivateTab: (id: string) => void;
  onCloseTab: (id: string) => void;
  onNewSession: () => void;
  onOpenSidebar: () => void;
  /** Durable projects visible from the active workspace. */
  projects: GroupProjectRow[];
  /** True when the list is filtered to a coding workspace — said out loud in
   *  the panel, so a short list never looks like a bug. */
  projectsScoped: boolean;
  onOpenProject: (projectId: string) => void;
  schedules: ScheduledTask[];
  onNewTask: () => void;
  onCancelSchedule: (scheduleId: string) => void;
  onOpenScheduledChat: (workspaceId: string, chatId: string) => void;
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
 * Main workspace top bar: hamburger + projects + session tab strip.
 */
export default function HouseHeader({
  tabs, activeTabId, runningTabId,
  onActivateTab, onCloseTab, onNewSession,
  onOpenSidebar, projects, projectsScoped, onOpenProject, schedules,
  onNewTask, onCancelSchedule, onOpenScheduledChat,
}: HouseHeaderProps) {
  const hasDesktopBridge = typeof window !== 'undefined' && !!window.antigravity;
  const isDesktopShell = hasDesktopBridge || Boolean((window as unknown as { __TAURI__?: unknown }).__TAURI__);
  const [projectsOpen, setProjectsOpen] = useState(false);
  const [taskTab, setTaskTab] = useState<'projects' | 'scheduled'>('projects');
  const projectsRef = useRef<HTMLDivElement | null>(null);
  const handleNewTask = useCallback(() => {
    setProjectsOpen(false);
    onNewTask();
  }, [onNewTask]);

  useDismiss(projectsOpen, useCallback(() => setProjectsOpen(false), []), projectsRef);

  const waiting = projects.reduce((n, p) => n + (p.awaiting_approval || 0), 0);

  return (
    <header
      className={`relative z-20 flex items-center gap-2 px-3 sm:px-4 ${isDesktopShell ? 'pr-36' : ''} h-[52px] border-b house-hairline shrink-0`}
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

      {/* ── Durable and scheduled tasks ── */}
      <div ref={projectsRef} className="relative shrink-0" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <div className="flex items-center gap-0.5">
          <button
            onClick={() => setProjectsOpen((v) => !v)}
            title="Tasks"
            aria-label="Tasks"
            aria-expanded={projectsOpen}
            aria-haspopup="true"
            className={`relative grid place-items-center w-8 h-8 rounded-lg transition-colors cursor-pointer ${
              projectsOpen ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.07]'
            }`}
          >
            <FolderKanban size={15} />
            {(projects.length + schedules.length) > 0 && (
              <span className={`absolute -top-0.5 -right-0.5 grid place-items-center min-w-[14px] h-[14px] px-0.5 rounded-full text-[9px] font-bold tabular-nums ${
                waiting > 0 ? 'bg-amber-400 text-[#0a0a0d]' : 'bg-white text-[#0a0a0d]'
              }`}>
                {projects.length + schedules.length}
              </span>
            )}
          </button>
          <button type="button" onClick={handleNewTask} title="New task" aria-label="Create a task" className="grid h-7 w-7 place-items-center rounded-md text-zinc-500 transition-colors hover:bg-white/[0.07] hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70">
            <Plus size={14} />
          </button>
        </div>

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
            <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500">Tasks</span>
            <button type="button" onClick={handleNewTask} className="flex h-7 items-center gap-1 rounded-md bg-white px-2 text-[10px] font-semibold text-[#0a0a0d] hover:bg-zinc-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70">
              <Plus size={12} /> New task
            </button>
          </div>

          <div role="tablist" aria-label="Task categories" className="flex gap-1 border-b border-white/[0.06] px-2 py-1.5">
            <button type="button" role="tab" aria-selected={taskTab === 'projects'} onClick={() => setTaskTab('projects')} className={`rounded-md px-2.5 py-1 text-[10px] font-medium ${taskTab === 'projects' ? 'bg-white/[0.08] text-zinc-200' : 'text-zinc-500 hover:text-zinc-300'}`}>Projects <span className="ml-1 text-zinc-600">{projects.length}</span></button>
            <button type="button" role="tab" aria-selected={taskTab === 'scheduled'} onClick={() => setTaskTab('scheduled')} className={`rounded-md px-2.5 py-1 text-[10px] font-medium ${taskTab === 'scheduled' ? 'bg-white/[0.08] text-zinc-200' : 'text-zinc-500 hover:text-zinc-300'}`}>Scheduled <span className="ml-1 text-zinc-600">{schedules.length}</span></button>
          </div>

          {taskTab === 'projects' ? <>
          {projectsScoped && (
            <div className="px-3.5 pt-2 text-[10px] text-zinc-600" title="Projects started from this workspace">Projects from this workspace</div>
          )}
          {projects.length === 0 ? (
            <div className="px-4 py-6 text-center">
              <p className="text-[12px] text-zinc-500">
                {projectsScoped ? 'No projects from this workspace.' : 'No projects yet.'}
              </p>
              <p className="text-[11px] text-zinc-600 mt-1">
                {projectsScoped
                  ? 'Projects started here will appear here.'
                  : 'Create a project to coordinate longer work.'}
              </p>
              <button type="button" onClick={handleNewTask} className="mx-auto mt-3 flex h-8 items-center gap-1 rounded-lg bg-white px-3 text-[10px] font-semibold text-[#0a0a0d] hover:bg-zinc-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70"><Plus size={12} /> New task</button>
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
                  <span className="flex min-w-0 flex-1 flex-col gap-1">
                    <span className="flex min-w-0 items-center gap-1.5">
                      <b className="min-w-0 flex-1 truncate text-[11px] font-semibold text-zinc-200">
                        {p.title || p.id}
                      </b>
                      {p.last_activity_ts ? (
                        <span className="shrink-0 text-[8.5px] text-zinc-600">
                          {timeAgo(p.last_activity_ts)}
                        </span>
                      ) : null}
                    </span>
                    <span className="truncate text-[9.5px] leading-4 text-zinc-500">
                      {p.active_item
                        ? `${displayActor(p.active_member || '')} · ${p.active_item.title}`
                        : p.last_message?.body || (p.goal && p.goal !== p.title ? p.goal : 'No messages yet.')}
                    </span>
                    <span className="flex min-w-0 items-center gap-2">
                      <span className="h-1 min-w-0 flex-1 overflow-hidden rounded-full bg-white/[0.06]">
                        <span
                          className="block h-full rounded-full bg-zinc-400"
                          style={{
                            width: `${p.items_total ? Math.round((p.items_done / p.items_total) * 100) : 0}%`,
                          }}
                        />
                      </span>
                      <span className="shrink-0 text-[8.5px] tabular-nums text-zinc-600">
                        {p.milestones_done}/{p.milestones_total} · {p.items_done}/{p.items_total}
                      </span>
                    </span>
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
          </> : schedules.length === 0 ? (
            <div className="px-4 py-6 text-center">
              <CalendarClock size={18} className="mx-auto mb-2 text-zinc-600" />
              <p className="text-[12px] text-zinc-500">No scheduled tasks.</p>
              <p className="mt-1 text-[11px] text-zinc-600">Choose when a prompt should run.</p>
              <button type="button" onClick={handleNewTask} className="mx-auto mt-3 flex h-8 items-center gap-1 rounded-lg bg-white px-3 text-[10px] font-semibold text-[#0a0a0d] hover:bg-zinc-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70"><Plus size={12} /> New task</button>
            </div>
          ) : (
            <div className="flex flex-col gap-1.5 p-2">
              {schedules.map((schedule) => (
                <div key={schedule.id} className="group flex items-center gap-2 rounded-lg border border-white/[0.05] bg-white/[0.02] p-2 hover:bg-white/[0.045]">
                  <button type="button" onClick={() => { setProjectsOpen(false); onOpenScheduledChat(schedule.workspaceId, schedule.chatId); }} className="min-w-0 flex-1 text-left focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70">
                    <span className="block truncate text-[11px] font-medium text-zinc-200">{schedule.name}</span>
                    <span className="mt-1 block text-[9.5px] text-zinc-500">{new Date(schedule.nextRunAt).toLocaleString()} · {schedule.recurrence === 'none' ? 'once' : schedule.recurrence}</span>
                  </button>
                  <button type="button" onClick={() => onCancelSchedule(schedule.id)} title="Cancel scheduled task" aria-label={`Cancel ${schedule.name}`} className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-zinc-600 hover:bg-white/[0.08] hover:text-zinc-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70"><X size={13} /></button>
                </div>
              ))}
            </div>
          )}
        </div>
      </div>

      {/* ── Session tab strip (the top bar IS the workspace switcher) ── */}
      <div className="flex-1 min-w-0 flex items-center" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
        <SessionTabs
          tabs={tabs}
          activeTabId={activeTabId}
          runningTabId={runningTabId}
          onActivateTab={onActivateTab}
          onCloseTab={onCloseTab}
          onNewSession={onNewSession}
        />
      </div>
      <WindowControls className="absolute right-2 top-1/2 -translate-y-1/2 z-30" />
    </header>
  );
}
