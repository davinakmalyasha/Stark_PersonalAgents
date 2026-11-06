import { FolderKanban, Plus } from 'lucide-react';
import type { GroupProjectRow } from '../types';
import { displayActor } from './group/groupProjectUi';

interface Props {
  projects: GroupProjectRow[];
  projectsScoped: boolean;
  onOpenProject: (projectId: string) => void;
  onNewProject: () => void;
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

function initials(value: string): string {
  const parts = (value || 'Project').replace(/[^\p{L}\p{N}\s_-]/gu, ' ').split(/[\s_-]+/).filter(Boolean);
  return ((parts[0]?.[0] || 'P') + (parts[1]?.[0] || '')).toUpperCase();
}

function timeAgo(seconds?: number): string {
  if (!seconds) return '';
  const mins = Math.floor(Math.max(0, Date.now() - seconds * 1000) / 60000);
  if (mins < 1) return 'now';
  if (mins < 60) return `${mins}m`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

/** WhatsApp-style group-project list: every row is one live conversation. */
export default function HouseProjectsPanel({
  projects, projectsScoped, onOpenProject, onNewProject,
}: Props) {
  return (
    <div className="flex flex-col min-h-0 h-full">
      <div className="px-3.5 pt-4 pb-2 flex items-center gap-1.5 shrink-0">
        <span className="truncate text-[15px] font-semibold tracking-tight text-zinc-100">Group chats</span>
        <span className="flex-1" />
        <button
          onClick={onNewProject}
          title="New group project"
          aria-label="New group project"
          className="grid place-items-center w-7 h-7 rounded-lg text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
        >
          <Plus size={14} />
        </button>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto house-thread px-1.5 pb-3.5">
        {projectsScoped && (
          <p className="px-2 pb-1.5 text-[10.5px] text-zinc-600">Scoped to this coding workspace.</p>
        )}

        {projects.length === 0 && (
          <div className="flex flex-col items-center gap-2.5 px-4 py-12 text-center">
            <span className="grid place-items-center w-11 h-11 rounded-2xl border border-white/[0.07] bg-white/[0.025] text-zinc-600">
              <FolderKanban size={20} />
            </span>
            <span className="text-[12px] font-medium text-zinc-300">No group chats yet</span>
            <span className="max-w-[190px] text-[10.5px] leading-relaxed text-zinc-600">
              Start a project and the team chat will appear here.
            </span>
            <button
              type="button"
              onClick={onNewProject}
              className="mt-1 rounded-lg border border-white/[0.09] bg-white/[0.05] px-3 py-1.5 text-[10.5px] font-medium text-zinc-200 hover:bg-white/[0.09] transition-colors cursor-pointer"
            >
              New project
            </button>
          </div>
        )}

        {projects.map((project) => {
          const title = project.title || project.id;
          const waiting = project.awaiting_approval || 0;
          const preview = waiting > 0
            ? `${waiting} work item${waiting === 1 ? '' : 's'} waiting on you`
            : project.active_item
              ? `${displayActor(project.active_member || 'member')} · ${project.active_item.title}`
              : project.last_message?.body || project.goal || 'No activity yet.';
          const time = timeAgo(project.last_activity_ts);
          return (
            <button
              key={project.id}
              type="button"
              onClick={() => onOpenProject(project.id)}
              className="group/project relative w-full flex items-center gap-2.5 rounded-xl px-2 py-2 text-left transition-colors hover:bg-white/[0.045] cursor-pointer"
            >
              <span className="relative grid place-items-center w-9 h-9 flex-none rounded-xl border border-white/[0.075] bg-white/[0.035] text-[10px] font-bold text-zinc-300">
                {initials(title)}
                <span className={`absolute -right-0.5 -bottom-0.5 w-2.5 h-2.5 rounded-full border-2 border-[#0f0f13] ${dot(project.status, waiting)}`} />
              </span>
              <span className="min-w-0 flex-1">
                <span className="flex items-baseline gap-2">
                  <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium text-zinc-200">{title}</span>
                  <span className="flex-none text-[9px] tabular-nums text-zinc-600">{time}</span>
                </span>
                <span className={`mt-0.5 block truncate text-[10.5px] ${waiting > 0 ? 'text-amber-300/85' : 'text-zinc-600'}`}>
                  {preview}
                </span>
              </span>
              {waiting > 0 && (
                <span className="grid place-items-center min-w-[17px] h-[17px] flex-none rounded-full bg-amber-400 px-1 text-[9px] font-bold text-[#0a0a0d] tabular-nums">
                  {waiting}
                </span>
              )}
              <span className="ml-0.5 flex-none text-[8px] font-semibold uppercase tracking-wide text-zinc-700">
                {project.status}
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
