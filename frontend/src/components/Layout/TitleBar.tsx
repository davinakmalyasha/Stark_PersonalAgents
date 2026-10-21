import { useEffect, useState } from 'react';
import { Smartphone, X } from 'lucide-react';
import PairingModal from './PairingModal';

type AppRegionStyle = React.CSSProperties & { WebkitAppRegion?: 'drag' | 'no-drag' };

interface CwTab {
  workspaceId: string;
  chatId: string;
  title: string;
  workspaceName: string;
}

interface TitleBarProps {
  tabs?: CwTab[];
  activeKey?: string | null;
  onActivateTab?: (wsId: string, chatId: string) => void;
  onCloseTab?: (wsId: string, chatId: string) => void;
  onDropTab?: (data: string) => void;
}

function wsColor(name: string): string {
  const colors = ['#3b82f6', '#8b5cf6', '#06b6d4', '#10b981', '#f59e0b', '#ef4444', '#ec4899', '#6366f1'];
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = (hash * 31 + name.charCodeAt(i)) >>> 0;
  return colors[hash % colors.length];
}

export default function TitleBar({ tabs = [], activeKey, onActivateTab, onCloseTab, onDropTab }: TitleBarProps) {
  const [pairingOpen, setPairingOpen] = useState(false);
  const [dragOver, setDragOver] = useState(false);
  const [liveChatIds, setLiveChatIds] = useState<Set<string>>(new Set());
  useEffect(() => {
    let cancelled = false;
    const poll = async () => {
      if (document.hidden) return;
      try {
        const res = await fetch('/api/runs/active');
        const data = await res.json();
        const ids = new Set<string>();
        for (const r of (Array.isArray((data as any)?.runs) ? (data as any).runs : data?.running ? [data] : [])) {
          if (r.chat_id) ids.add(String(r.chat_id));
        }
        if (!cancelled) setLiveChatIds(ids);
      } catch {}
    };
    poll();
    const id = window.setInterval(poll, 2500);
    const onVis = () => { if (!document.hidden) poll(); };
    document.addEventListener('visibilitychange', onVis);
    return () => { cancelled = true; window.clearInterval(id); document.removeEventListener('visibilitychange', onVis); };
  }, []);

  return (
    <header
      className={`relative flex items-center select-none flex-shrink-0 h-[38px] bg-[#0c0c0e] border-b border-zinc-800/60 px-2 z-50 gap-2 ${dragOver ? 'bg-white/[0.04]' : ''}`}
      style={{ WebkitAppRegion: 'drag' } as AppRegionStyle}
      onDragOver={(e) => {
        if (!onDropTab) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = 'move';
        if (!dragOver) setDragOver(true);
      }}
      onDragLeave={() => setDragOver(false)}
      onDrop={(e) => {
        if (!onDropTab) return;
        e.preventDefault();
        setDragOver(false);
        const data = e.dataTransfer.getData('text/plain');
        if (data) onDropTab(data);
      }}
    >
      {/* Tabs strip — opencode-style, draggable from sidebar */}
      <div
        className={`flex items-center gap-1 flex-1 min-w-0 overflow-x-auto no-scrollbar ${dragOver ? 'ring-1 ring-white/10 rounded-md' : ''}`}
        style={{ WebkitAppRegion: 'no-drag' } as AppRegionStyle}
      >
        {tabs.length === 0 && (
          <span className="text-[11px] text-zinc-600 px-1 select-none">No tabs — drag a chat from the sidebar here</span>
        )}
        {tabs.map((t) => {
          const key = `${t.workspaceId}:${t.chatId}`;
          const isActive = key === activeKey;
          const isLive = liveChatIds.has(t.chatId);
          return (
            <div
              key={key}
              onClick={() => onActivateTab?.(t.workspaceId, t.chatId)}
              title={`${t.title} — ${t.workspaceName}${isLive ? ' · Stark working…' : ''}`}
              className={`group flex items-center gap-1.5 pl-2 pr-1 py-1 rounded-md cursor-pointer select-none shrink-0 max-w-[180px] border text-[11px] transition-colors ${
                isActive
                  ? 'bg-white/[0.09] border-white/15 text-white'
                  : 'bg-white/[0.03] border-transparent text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.06] hover:border-white/10'
              } ${isLive ? 'ring-1 ring-emerald-500/30' : ''}`}
            >
              <span
                className="grid place-items-center w-5 h-5 rounded text-[10px] font-bold text-white shrink-0 relative"
                style={{ background: wsColor(t.workspaceName) }}
              >
                {(t.workspaceName?.[0] || 'C').toUpperCase()}
                {isLive && <span className="absolute -top-1 -right-1 w-2 h-2 rounded-full bg-emerald-400 animate-pulse ring-2 ring-[#0c0c0e]" />}
              </span>
              <span className="truncate">{t.title || 'Untitled'}</span>
              <button
                onClick={(e) => { e.stopPropagation(); onCloseTab?.(t.workspaceId, t.chatId); }}
                title="Close tab"
                className="grid place-items-center w-5 h-5 rounded text-zinc-500 hover:text-white hover:bg-white/10 shrink-0"
              >
                <X size={11} />
              </button>
            </div>
          );
        })}
        {dragOver && tabs.length > 0 && (
          <span className="text-[11px] text-zinc-500 px-2 shrink-0">Drop to open</span>
        )}
      </div>

      <button
        onClick={() => setPairingOpen(true)}
        title="Pair your phone to control this app remotely"
        className="flex items-center gap-1.5 rounded-md px-2 py-1 text-[11px] text-zinc-500 hover:bg-zinc-800/70 hover:text-zinc-300 transition-colors shrink-0"
        style={{ WebkitAppRegion: 'no-drag' } as AppRegionStyle}
      >
        <Smartphone size={13} />
        Phone
      </button>
      <PairingModal open={pairingOpen} onClose={() => setPairingOpen(false)} />
    </header>
  );
}
