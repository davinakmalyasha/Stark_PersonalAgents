import { useMemo, useState } from 'react';
import type { ReactNode } from 'react';
import { Check, FolderKanban, MessageSquare, Palette, Plus, Search, X } from 'lucide-react';
import type { ChatSummary, GroupProjectRow, WorkspaceInfo } from '../types';
import type { CanvasAddonId, CanvasDraft } from './canvasTypes';
import { draftId } from './canvasTypes';
import { cleanChatTitle } from '../utils/chatTitle';

export interface AddChatEntry {
  workspace: WorkspaceInfo;
  chat: ChatSummary;
}

export interface AddProjectEntry {
  workspace: WorkspaceInfo;
  project: GroupProjectRow;
}

export interface AddAddonEntry {
  workspace: WorkspaceInfo;
  addonId: CanvasAddonId;
  chatId?: string;
}

interface Props {
  chats: AddChatEntry[];
  projects: AddProjectEntry[];
  addons: AddAddonEntry[];
  added: Set<string>;
  onAdd: (draft: CanvasDraft) => void;
  onClose: () => void;
}

const ADDON_LABEL: Record<CanvasAddonId, string> = {
  design: 'OpenDesign',
};

function Row({
  draft, title, subtitle, icon, added, onAdd,
}: {
  draft: CanvasDraft;
  title: string;
  subtitle: string;
  icon: ReactNode;
  added: boolean;
  onAdd: (draft: CanvasDraft) => void;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      draggable={!added}
      onDragStart={(e) => {
        e.dataTransfer.setData('application/x-stark-canvas-item', JSON.stringify(draft));
        e.dataTransfer.effectAllowed = 'copy';
      }}
      onClick={() => { if (!added) onAdd(draft); }}
      onKeyDown={(e) => { if (!added && e.key === 'Enter') onAdd(draft); }}
      className={`flex items-center gap-2 rounded-md px-2 py-1.5 text-left transition-colors ${
        added ? 'cursor-default opacity-45' : 'cursor-pointer hover:bg-white/[0.05]'
      }`}
      title={added ? 'Already on the canvas' : `${title} — click or drag to canvas`}
    >
      <span className="flex-none text-zinc-600">{icon}</span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[11.5px] text-zinc-300">{title}</span>
        <span className="block truncate text-[9.5px] text-zinc-600">{subtitle}</span>
      </span>
      {added ? <Check size={11} className="flex-none text-emerald-400" /> : <Plus size={11} className="flex-none text-zinc-600" />}
    </div>
  );
}

export default function CanvasAddMenu({ chats, projects, addons, added, onAdd, onClose }: Props) {
  const [query, setQuery] = useState('');
  const [tab, setTab] = useState<'chats' | 'projects' | 'addons'>('chats');
  const q = query.trim().toLowerCase();

  const filteredChats = useMemo(
    () => chats.filter(({ chat, workspace }) =>
      !q || `${chat.title} ${workspace.name}`.toLowerCase().includes(q)),
    [chats, q],
  );
  const filteredProjects = useMemo(
    () => projects.filter(({ project, workspace }) =>
      !q || `${project.title} ${project.id} ${workspace.name}`.toLowerCase().includes(q)),
    [projects, q],
  );
  const filteredAddons = useMemo(
    () => addons.filter(({ workspace }) =>
      !q || `${workspace.name} design`.toLowerCase().includes(q)),
    [addons, q],
  );

  return (
    <div className="absolute left-3 top-12 z-30 w-[360px] max-h-[min(480px,calc(100vh-120px))] overflow-hidden rounded-xl border border-white/[0.09] bg-[#121317]/98 shadow-2xl backdrop-blur-xl">
      <div className="flex items-center gap-2 border-b border-white/[0.06] px-2.5 py-2">
        <Search size={11} className="flex-none text-zinc-600" />
        <input
          autoFocus
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Add chat, project, or addon…"
          className="min-w-0 flex-1 bg-transparent text-[11.5px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none"
        />
        <button type="button" onClick={onClose} className="grid h-5 w-5 place-items-center rounded text-zinc-600 hover:bg-white/[0.06] hover:text-white" aria-label="Close add menu">
          <X size={11} />
        </button>
      </div>
      <div className="flex items-center gap-1 border-b border-white/[0.06] px-2 py-1.5">
        {([
          ['chats', 'Chats', filteredChats.length],
          ['projects', 'Projects', filteredProjects.length],
          ['addons', 'Addons', filteredAddons.length],
        ] as const).map(([key, label, count]) => (
          <button
            key={key}
            type="button"
            onClick={() => setTab(key)}
            className={`rounded-md px-2 py-1 text-[10px] font-medium transition-colors ${
              tab === key ? 'bg-white/[0.09] text-white' : 'text-zinc-500 hover:bg-white/[0.04] hover:text-zinc-300'
            }`}
          >
            {label} <span className="text-[9px] text-zinc-600">{count}</span>
          </button>
        ))}
      </div>
      <div className="max-h-[380px] overflow-y-auto p-1.5">
        {tab === 'chats' && filteredChats.length > 0 && (
          <>
            <div className="px-2 pb-1 pt-2 text-[8.5px] font-bold uppercase tracking-widest text-zinc-600">Chats</div>
            {filteredChats.map(({ workspace, chat }) => {
              const draft: CanvasDraft = { kind: 'chat', workspaceId: workspace.id, chatId: chat.id };
              return (
                <Row
                  key={draftId(draft)}
                  draft={draft}
                  title={cleanChatTitle(chat.title)}
                  subtitle={workspace.isGeneral ? 'Main workspace' : workspace.name}
                  icon={<MessageSquare size={11} />}
                  added={added.has(draftId(draft))}
                  onAdd={onAdd}
                />
              );
            })}
          </>
        )}

        {tab === 'projects' && filteredProjects.length > 0 && (
          <>
            <div className="px-2 pb-1 pt-2 text-[8.5px] font-bold uppercase tracking-widest text-zinc-600">Projects</div>
            {filteredProjects.map(({ workspace, project }) => {
              const draft: CanvasDraft = { kind: 'project', workspaceId: workspace.id, projectId: project.id };
              return (
                <Row
                  key={draftId(draft)}
                  draft={draft}
                  title={project.title || project.id}
                  subtitle={`${workspace.isGeneral ? 'Main workspace' : workspace.name} · ${project.items_done}/${project.items_total} items`}
                  icon={<FolderKanban size={11} />}
                  added={added.has(draftId(draft))}
                  onAdd={onAdd}
                />
              );
            })}
          </>
        )}

        {tab === 'addons' && filteredAddons.length > 0 && (
          <>
            <div className="px-2 pb-1 pt-2 text-[8.5px] font-bold uppercase tracking-widest text-zinc-600">Addons</div>
            {filteredAddons.map(({ workspace, addonId, chatId }) => {
              const draft: CanvasDraft = { kind: 'addon', workspaceId: workspace.id, addonId, chatId };
              return (
                <Row
                  key={draftId(draft)}
                  draft={draft}
                  title={ADDON_LABEL[addonId]}
                  subtitle={workspace.isGeneral ? 'Main workspace' : workspace.name}
                  icon={<Palette size={11} />}
                  added={added.has(draftId(draft))}
                  onAdd={onAdd}
                />
              );
            })}
          </>
        )}

        {((tab === 'chats' && filteredChats.length === 0) || (tab === 'projects' && filteredProjects.length === 0) || (tab === 'addons' && filteredAddons.length === 0)) && (
          <div className="px-3 py-6 text-center text-[11px] text-zinc-600">
            {query.trim() ? `Nothing matches “${query.trim()}”.` : `No ${tab} available.`}
          </div>
        )}
      </div>
    </div>
  );
}
