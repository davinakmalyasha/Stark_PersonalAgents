import { Cloud, FolderKanban, Home, Settings, SquarePen } from 'lucide-react';
import type { CSSProperties } from 'react';

export type SideView = 'chats' | 'projects';

interface Props {
  view: SideView;
  /** Rail icon click: switch the side panel view (clicking active collapses). */
  onView: (view: SideView) => void;
  onNewChat: () => void;
  onOpenSettings: () => void;
  onOpenCloud: () => void;
  waiting: number;
}

export default function HouseRail({ view, onView, onNewChat, onOpenSettings, onOpenCloud, waiting }: Props) {
  const btn = (active: boolean) =>
    `house-rail-btn grid place-items-center w-9 h-9 rounded-xl cursor-pointer shrink-0 ${
      active ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.07]'
    }`;
  return (
    <nav
      aria-label="Workspace"
      className="flex flex-col items-center gap-1 py-3 px-1.5 shrink-0 border-r house-hairline bg-[#0c0c0f] z-20"
      style={{ WebkitAppRegion: 'drag' } as CSSProperties}
    >
      <div style={{ WebkitAppRegion: 'no-drag' } as CSSProperties} className="flex flex-col items-center gap-1">
        <button onClick={onNewChat} title="New chat" aria-label="New chat" className={btn(false)}>
          <SquarePen size={17} />
        </button>
      </div>
      <div className="w-6 h-px bg-white/[0.07] my-1.5 shrink-0" />
      <div style={{ WebkitAppRegion: 'no-drag' } as CSSProperties} className="flex flex-col items-center gap-1">
        <button
          onClick={() => onView('chats')}
          title="Chats"
          aria-label="Chats"
          aria-pressed={view === 'chats'}
          className={btn(view === 'chats')}
        >
          <Home size={17} />
        </button>
        <button
          onClick={() => onView('projects')}
          title="Group projects"
          aria-label="Group projects"
          aria-pressed={view === 'projects'}
          className={`relative ${btn(view === 'projects')}`}
        >
          <FolderKanban size={17} />
          {waiting > 0 && (
            <span className="absolute -top-0.5 -right-0.5 grid place-items-center min-w-[14px] h-[14px] px-0.5 rounded-full bg-amber-400 text-[#0a0a0d] text-[9px] font-bold tabular-nums">
              {waiting}
            </span>
          )}
        </button>
        <button
          onClick={onOpenCloud}
          title="Cloud runs"
          aria-label="Cloud runs"
          className={btn(false)}
        >
          <Cloud size={17} />
        </button>
      </div>
      <span className="flex-1" />
      <div style={{ WebkitAppRegion: 'no-drag' } as CSSProperties} className="flex flex-col items-center gap-1">
        <button onClick={onOpenSettings} title="Settings" aria-label="Open settings" className={btn(false)}>
          <Settings size={17} />
        </button>
      </div>
    </nav>
  );
}
