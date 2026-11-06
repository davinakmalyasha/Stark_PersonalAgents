import { FolderKanban, Home, Settings, SquarePen, Waypoints } from 'lucide-react';
import type { SideView } from './HouseRail';

interface Props {
  view: SideView;
  onView: (view: SideView) => void;
  onNewChat: () => void;
  onOpenSettings: () => void;
  onOpenMap: () => void;
  mapOpen: boolean;
  waiting: number;
}

/**
 * Phone/tablet navigation. The vertical rail is hidden below `md`; this bar
 * gives the same destinations without stealing chat width.
 */
export default function HouseMobileNav({ view, onView, onNewChat, onOpenSettings, onOpenMap, mapOpen, waiting }: Props) {
  const item = (active: boolean) =>
    `house-mobile-nav-btn relative flex flex-1 flex-col items-center justify-center gap-0.5 py-1.5 rounded-lg cursor-pointer ${
      active ? 'text-white' : 'text-zinc-500 active:text-zinc-200'
    }`;
  return (
    <nav
      aria-label="Workspace"
      className="md:hidden shrink-0 flex items-stretch gap-1 px-2 pt-1 border-t house-hairline bg-[#0c0c0f]"
      style={{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 4px)' }}
    >
      <button onClick={() => onView('chats')} className={item(view === 'chats')} aria-label="Chats" aria-pressed={view === 'chats'}>
        <Home size={18} />
        <span className="text-[9px]">Chats</span>
      </button>
      <button onClick={onOpenMap} className={item(mapOpen)} aria-label="Agent map" aria-pressed={mapOpen}>
        <Waypoints size={18} />
        <span className="text-[9px]">Map</span>
      </button>
      <button onClick={onNewChat} className={item(false)} aria-label="New chat">
        <SquarePen size={18} />
        <span className="text-[9px]">New</span>
      </button>
      <button onClick={() => onView('projects')} className={item(view === 'projects')} aria-label="Projects" aria-pressed={view === 'projects'}>
        <span className="relative">
          <FolderKanban size={18} />
          {waiting > 0 && (
            <span className="absolute -top-1 -right-1 grid place-items-center min-w-[13px] h-[13px] px-0.5 rounded-full bg-amber-400 text-[#0a0a0d] text-[8px] font-bold tabular-nums">
              {waiting}
            </span>
          )}
        </span>
        <span className="text-[9px]">Projects</span>
      </button>
      <button onClick={onOpenSettings} className={item(false)} aria-label="Settings">
        <Settings size={18} />
        <span className="text-[9px]">Settings</span>
      </button>
    </nav>
  );
}
