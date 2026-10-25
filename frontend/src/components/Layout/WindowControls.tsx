import { useEffect, useState } from 'react';
import { Minus, Square, X } from 'lucide-react';

type AppRegionStyle = React.CSSProperties & { WebkitAppRegion?: 'drag' | 'no-drag' };

type TauriWindowHandle = {
  minimize?: () => void;
  toggleMaximize?: () => void;
  close?: () => void;
};

type TauriGlobal = {
  __TAURI__?: {
    window?: {
      getCurrentWindow?: () => TauriWindowHandle;
    };
  };
};

const isTauri = () => Boolean((window as unknown as TauriGlobal).__TAURI__);

const currentWindow = () => (window as unknown as TauriGlobal).__TAURI__?.window?.getCurrentWindow?.();

function run(action: 'minimize' | 'toggleMaximize' | 'close') {
  const win = currentWindow();
  if (!win) return;
  if (action === 'minimize') win.minimize?.();
  if (action === 'toggleMaximize') win.toggleMaximize?.();
  if (action === 'close') win.close?.();
}

export default function WindowControls({ className = '' }: { className?: string }) {
  const [ready, setReady] = useState(isTauri);
  useEffect(() => {
    if (ready) return;
    const timer = window.setInterval(() => {
      if (isTauri()) {
        setReady(true);
        window.clearInterval(timer);
      }
    }, 100);
    return () => window.clearInterval(timer);
  }, [ready]);
  if (!ready) return null;
  return (
    <div
      data-tauri-window-control
      className={`flex items-center gap-0.5 shrink-0 ${className}`}
      style={{ WebkitAppRegion: 'no-drag' } as AppRegionStyle}
    >
      <button
        onClick={() => run('minimize')}
        title="Minimize"
        aria-label="Minimize"
        className="grid place-items-center w-8 h-7 rounded text-zinc-500 hover:text-white hover:bg-white/10 transition-colors"
      >
        <Minus size={13} />
      </button>
      <button
        onClick={() => run('toggleMaximize')}
        title="Maximize"
        aria-label="Maximize"
        className="grid place-items-center w-8 h-7 rounded text-zinc-500 hover:text-white hover:bg-white/10 transition-colors"
      >
        <Square size={11} />
      </button>
      <button
        onClick={() => run('close')}
        title="Close"
        aria-label="Close"
        className="grid place-items-center w-8 h-7 rounded text-zinc-500 hover:text-white hover:bg-red-500/80 transition-colors"
      >
        <X size={13} />
      </button>
    </div>
  );
}
