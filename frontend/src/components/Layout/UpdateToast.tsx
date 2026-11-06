import { useEffect, useState } from 'react';
import { ArrowUpCircle, CheckCircle2, RefreshCw, TriangleAlert } from 'lucide-react';
import { subscribeUpdate, type UpdateState } from '../../utils/updateStore';

const HIDE_AFTER_MS = 6000;

/**
 * One global update indicator. Replaces the old behaviour where update status
 * only appeared inside whichever chat happened to set the notification.
 */
export default function UpdateToast() {
  const [state, setState] = useState<UpdateState | null>(null);
  const [hidden, setHidden] = useState(false);

  useEffect(() => subscribeUpdate((next) => {
    setState(next);
    setHidden(false);
  }), []);

  useEffect(() => {
    if (!state) return;
    if (state.phase === 'ready' || state.phase === 'downloading') return;
    const timer = window.setTimeout(() => setHidden(true), HIDE_AFTER_MS);
    return () => window.clearTimeout(timer);
  }, [state]);

  if (!state || state.phase === 'idle' || state.phase === 'unsupported') return null;
  if (hidden && state.phase !== 'ready' && state.phase !== 'downloading') return null;

  const icon =
    state.phase === 'downloading' ? <RefreshCw size={13} className="animate-spin" />
      : state.phase === 'ready' ? <ArrowUpCircle size={13} />
        : state.phase === 'uptodate' ? <CheckCircle2 size={13} />
          : <TriangleAlert size={13} />;

  const tone =
    state.phase === 'error' ? 'text-amber-300'
      : state.phase === 'uptodate' ? 'text-zinc-400'
        : 'text-zinc-200';

  return (
    <div
      role="status"
      aria-live="polite"
      className="fixed z-[120] right-3 bottom-3 md:right-5 md:bottom-5 max-w-[calc(100vw-24px)] md:max-w-[320px] flex items-center gap-2 rounded-xl border border-white/10 bg-[#15151a]/95 backdrop-blur px-3 py-2 shadow-2xl"
      style={{ marginBottom: 'env(safe-area-inset-bottom)' }}
    >
      <span className={tone}>{icon}</span>
      <span className={`text-[11.5px] leading-snug ${tone}`}>{state.message}</span>
    </div>
  );
}
