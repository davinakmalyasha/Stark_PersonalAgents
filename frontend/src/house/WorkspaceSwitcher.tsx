import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Search, User, Code2, Home, X } from 'lucide-react';

interface WorkspaceItem {
  id: string;
  label: string;
  icon: React.ReactNode;
  onSelect: () => void;
}

interface Props {
  open: boolean;
  onClose: () => void;
  anchorRef: React.RefObject<HTMLElement | null>;
  onLaunchPersonal: () => void;
  onLaunchCoding: () => void;
  onLaunchHouse: () => void;
}

export default function WorkspaceSwitcher({
  open,
  onClose,
  anchorRef,
  onLaunchPersonal,
  onLaunchCoding,
  onLaunchHouse,
}: Props) {
  const [filter, setFilter] = useState('');
  const panelRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const items: WorkspaceItem[] = [
    { id: 'personal', label: 'Personal', icon: <User size={16} />, onSelect: onLaunchPersonal },
    { id: 'coding', label: 'Coding', icon: <Code2 size={16} />, onSelect: onLaunchCoding },
    { id: 'house', label: 'House', icon: <Home size={16} />, onSelect: onLaunchHouse },
  ];

  const q = filter.trim().toLowerCase();
  const filtered = q
    ? items.filter((it) => it.label.toLowerCase().includes(q))
    : items;

  useEffect(() => {
    if (!open) return;
    // Focus search on open
    requestAnimationFrame(() => inputRef.current?.focus());
    const onPointerDown = (e: PointerEvent) => {
      const target = e.target as Node;
      if (panelRef.current?.contains(target)) return;
      if (anchorRef.current?.contains(target)) return;
      onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, onClose, anchorRef]);

  // Position to the right of the sidebar (292px wide + 8px gap)
  const [style, setStyle] = useState<React.CSSProperties>({});
  useEffect(() => {
    if (!open) return;
    const update = () => {
      const anchor = anchorRef.current;
      if (!anchor) return;
      const rect = anchor.getBoundingClientRect();
      // Align panel's bottom with trigger's bottom, anchored just outside sidebar
      const left = rect.right + 10;
      const maxLeft = window.innerWidth - 340; // keep inside viewport
      setStyle({
        position: 'fixed',
        left: `${Math.min(left, maxLeft)}px`,
        bottom: `${Math.max(12, window.innerHeight - rect.bottom)}px`,
        zIndex: 60,
      });
    };
    update();
    window.addEventListener('resize', update);
    return () => window.removeEventListener('resize', update);
  }, [open, anchorRef]);

  if (!open) return null;

  const panel = (
    <div
      ref={panelRef}
      role="dialog"
      aria-label="Workspaces"
      aria-modal="true"
      style={style}
      className="w-[320px] rounded-xl border house-hairline bg-[#141419] shadow-2xl overflow-hidden flex flex-col max-h-[min(360px,calc(100vh-24px))]"
    >
      {/* Search */}
      <div className="flex items-center gap-2 px-3 py-2.5 border-b house-hairline shrink-0">
        <div className="flex items-center gap-2 flex-1 min-w-0 px-2.5 py-1.5 rounded-lg bg-white/[0.04] border border-white/[0.06] focus-within:border-white/[0.12] focus-within:bg-white/[0.06] transition-colors">
          <Search size={12} className="text-zinc-500 shrink-0" />
          <input
            ref={inputRef}
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Search workspaces…"
            className="flex-1 min-w-0 bg-transparent text-[12px] text-zinc-200 placeholder:text-zinc-500 focus:outline-none"
          />
          {filter && (
            <button
              onClick={() => setFilter('')}
              className="text-zinc-500 hover:text-zinc-300 text-[12px] leading-none cursor-pointer"
              aria-label="Clear search"
            >
              <X size={12} />
            </button>
          )}
        </div>
      </div>

      {/* Icon grid */}
      <div className="flex-1 min-h-0 overflow-y-auto house-thread p-3">
        {filtered.length === 0 ? (
          <div className="px-2 py-6 text-center text-[12px] text-zinc-500">
            No workspaces match “{filter.trim()}”
          </div>
        ) : (
          <div className="grid grid-cols-4 gap-2">
            {filtered.map((it) => (
              <button
                key={it.id}
                onClick={() => {
                  onClose();
                  it.onSelect();
                }}
                className="group flex flex-col items-center gap-1.5 py-2.5 rounded-xl hover:bg-white/[0.04] border border-transparent hover:border-white/[0.06] transition-colors cursor-pointer"
                title={it.label}
              >
                <span className="grid place-items-center w-11 h-11 rounded-xl bg-white/[0.04] border border-white/[0.06] text-zinc-300 group-hover:bg-white/[0.07] group-hover:border-white/[0.10] group-hover:text-white transition-colors">
                  {it.icon}
                </span>
                <span className="text-[10px] leading-tight text-zinc-500 group-hover:text-zinc-300 opacity-0 group-hover:opacity-100 -mt-0.5 max-w-[72px] truncate text-center transition-opacity">
                  {it.label}
                </span>
              </button>
            ))}
          </div>
        )}
      </div>

      <div className="px-3 py-2 border-t house-hairline text-[10px] text-zinc-500 text-center shrink-0">
        {filtered.length} workspace{filtered.length === 1 ? '' : 's'}
      </div>
    </div>
  );

  return createPortal(panel, document.body);
}
