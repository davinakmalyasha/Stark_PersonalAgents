import { CalendarClock, FolderKanban, PanelLeftClose, Plus, X } from 'lucide-react';
import type { GroupProjectRow, ScheduledTask } from '../types';
import { displayActor } from './group/groupProjectUi';

interface Props {
  projects: GroupProjectRow[];
  projectsScoped: boolean;
  schedules: ScheduledTask[];
  onOpenProject: (projectId: string) => void;
  onNewProject: () => void;
  onNewTask: () => void;
  onCancelSchedule: (scheduleId: string) => void;
  onOpenScheduledChat: (workspaceId: string, chatId: string) => void;
  onCollapse?: () => void;
}

function dot(status: string, awaiting: number): string {
  if (awaiting > 0) return 'bg-amber-400';
  switch (status) {
    case 'running': return 'bg-emerald-400';
    case 'done': return 'bg-zinc-500';
    case 'paused': return 'bg-sky-400';
    case 'stalled': return 'bg-orange-400';
    case 'failed': return 'bg-red-400';
    default: return 'bg-zinc-600';
  }
}

export default function HouseProjectsPanel({
  projects, projectsScoped, schedules, onOpenProject, onNewProject, onNewTask, onCancelSchedule, onOpenScheduledChat, onCollapse,
}: Props) {
  return (
    <div className="flex flex-col min-h-0 h-full">
      <div className="px-3.5 pt-4 pb-2 flex items-center gap-1.5 shrink-0">
        <span className="truncate text-[15px] font-semibold tracking-tight text-zinc-100">Projects</span>
        <span className="flex-1" />
        <button
          onClick={onNewProject}
          title="New project"
          aria-label="New project"
          className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
        >
          <Plus size={13} />
        </button>
        {onCollapse && (
          <button
            onClick={onCollapse}
            title="Collapse sidebar"
            aria-label="Collapse sidebar"
            className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
          >
            <PanelLeftClose size={13} />
          </button>
        )}
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto house-thread px-2.5 pb-3.5">
        {projectsScoped && (
          <p className="px-1 pb-1.5 text-[10.5px] text-zinc-600">Filtered to this coding workspace.</p>
        )}
        {projects.length === 0 && (
          <p className="px-1 py-3 text-[11.5px] text-zinc-600">No group projects yet.</p>
        )}
        {projects.map((p) => (
          <button
            key={p.id}
            onClick={() => onOpenProject(p.id)}
            className="w-full flex items-center gap-2.5 rounded-lg px-2.5 py-2 text-left hover:bg-white/[0.05] transition-colors cursor-pointer"
          >
            <span className={`w-2 h-2 rounded-full shrink-0 ${dot(p.status, p.awaiting_approval)}`} />
            <span className="flex-1 min-w-0">
              <span className="block truncate text-[12.5px] text-zinc-200">{p.title}</span>
              <span className="block truncate text-[10.5px] text-zinc-600">
                {p.awaiting_approval > 0
                  ? `${p.awaiting_approval} waiting on you`
                  : p.active_item
                    ? `${displayActor(p.active_member || 'member')} · ${p.active_item.title}`
                    : p.status}
              </span>
            </span>
          </button>
        ))}

        <div className="px-1 pt-3 pb-1 flex items-center gap-1.5">
          <CalendarClock size={12} className="text-zinc-600" />
          <span className="text-[10px] font-semibold uppercase tracking-widest text-zinc-500">Scheduled</span>
          <span className="flex-1" />
          <button
            onClick={onNewTask}
            title="New scheduled task"
            aria-label="New scheduled task"
            className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer"
          >
            <Plus size={13} />
          </button>
        </div>
        {schedules.length === 0 && (
          <p className="px-1 py-2 text-[11.5px] text-zinc-600">Nothing scheduled.</p>
        )}
        {schedules.map((s) => (
          <div key={s.id} className="group flex items-center gap-2 rounded-lg px-2.5 py-2 hover:bg-white/[0.04]">
            <button
              onClick={() => onOpenScheduledChat(s.workspaceId, s.chatId)}
              className="min-w-0 flex-1 text-left cursor-pointer"
            >
              <span className="block truncate text-[12px] text-zinc-200">{s.name}</span>
              <span className="block text-[10px] text-zinc-600">
                {new Date(s.nextRunAt).toLocaleString()} · {s.recurrence === 'none' ? 'once' : s.recurrence}
              </span>
            </button>
            <button
              onClick={() => onCancelSchedule(s.id)}
              title={`Cancel ${s.name}`}
              aria-label={`Cancel ${s.name}`}
              className="grid h-7 w-7 shrink-0 place-items-center rounded-md text-zinc-600 opacity-0 group-hover:opacity-100 hover:bg-white/[0.08] hover:text-zinc-200"
            >
              <X size={13} />
            </button>
          </div>
        ))}
        {projects.length === 0 && (
          <div className="px-1 pt-2">
            <FolderKanban size={22} className="text-zinc-700" />
          </div>
        )}
      </div>
    </div>
  );
}
