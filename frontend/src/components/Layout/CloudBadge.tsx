import { useEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { Cloud, CloudOff } from 'lucide-react';
import { API_BASE, apiUrl, sendToCloud } from '../../services/api';

interface RunInfo {
  running: boolean;
  chat_id?: string;
  workspace_path?: string;
  started_at?: string;
  origin?: string;
  cap?: number;
}

interface Props {
  chatId: string;
  workspaceId?: string;
  live: boolean;
  title?: string;
}

/**
 * Per-chat cloud badge: sits alongside rename/delete/pin in sidebar rows.
 * Glow = this chat has a live run (from /api/runs/active). Click opens the
 * details drawer (same divs as canvas cards: status, origin, started_at).
 */
export default function CloudBadge({ chatId, workspaceId, live, title }: Props) {
  const [open, setOpen] = useState(false);
  const [info, setInfo] = useState<RunInfo | null>(null);
  const [origin, setOrigin] = useState<string>('local');
  const [loading, setLoading] = useState(false);
  const [sending, setSending] = useState(false);
  const [sendNote, setSendNote] = useState<string | null>(null);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const [pos, setPos] = useState<{ top: number; left: number }>({ top: 0, left: 0 });

  const openDetails = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (btnRef.current) {
      const r = btnRef.current.getBoundingClientRect();
      setPos({ top: r.bottom + 6, left: Math.min(r.left, window.innerWidth - 300) });
    }
    setOpen((v) => !v);
    if (open) return;
    setLoading(true);
    try {
      const [runRes, healthRes] = await Promise.all([
        fetch(`${API_BASE}/runs/active?chat_id=${encodeURIComponent(chatId)}`).then((r) => r.json().catch(() => null)),
        fetch(apiUrl('/api/health')).then((r) => r.json().catch(() => null)),
      ]);
      if (runRes) {
        const running =
          !!runRes.running ||
          (Array.isArray(runRes.runs) && runRes.runs.some((x: { chat_id?: string }) => x?.chat_id === chatId));
        const entry = Array.isArray(runRes.runs)
          ? runRes.runs.find((x: { chat_id?: string }) => x?.chat_id === chatId)
          : null;
        setInfo({
          running,
          chat_id: chatId,
          workspace_path: entry?.workspace_path ?? runRes.workspace_path ?? '',
          started_at: entry?.started_at ?? runRes.started_at ?? '',
          origin: runRes.origin ?? healthRes?.origin ?? 'local',
          cap: runRes.cap ?? healthRes?.cap,
        });
      }
      if (healthRes?.origin) setOrigin(String(healthRes.origin));
    } catch {
      /* offline: keep last state */
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open ]);

  return (
    <>
      <button
        ref={btnRef}
        onClick={openDetails}
        className={`p-1.5 rounded-md flex-shrink-0 cursor-pointer transition-all ${
          live
            ? 'text-sky-300 hover:text-sky-100 hover:bg-sky-500/20 drop-shadow-[0_0_6px_rgba(125,211,252,0.7)]'
            : 'text-zinc-500 hover:text-zinc-200 hover:bg-zinc-700'
        }`}
        title={title || (live ? 'Running — click for run details' : 'Cloud run details')}
      >
        {live ? <Cloud size={12} className="animate-pulse" /> : <CloudOff size={12} />}
      </button>
      {open && createPortal(
        <div className="fixed inset-0 z-[200]" onClick={() => setOpen(false)}>
          <div
            className="absolute w-[280px] rounded-xl border border-zinc-800 bg-[#141417] shadow-2xl p-3"
            style={{ top: pos.top, left: pos.left }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center gap-2 mb-2">
              {live || info?.running
                ? <Cloud size={14} className="text-sky-300 animate-pulse" />
                : <CloudOff size={14} className="text-zinc-500" />}
              <span className="text-[12px] font-bold text-zinc-100">
                {(live || info?.running) ? 'Running' : 'Idle'}
              </span>
              <span className="ml-auto text-[10px] text-zinc-500">origin: {info?.origin ?? origin}</span>
            </div>
            {loading ? (
              <div className="text-[11px] text-zinc-500">Loading run details…</div>
            ) : (
              <div className="space-y-1.5 text-[11px]">
                <div className="flex justify-between gap-2">
                  <span className="text-zinc-500">chat</span>
                  <span className="truncate text-zinc-300 font-mono">{chatId.slice(0, 8)}…</span>
                </div>
                {workspaceId && (
                  <div className="flex justify-between gap-2">
                    <span className="text-zinc-500">workspace</span>
                    <span className="truncate text-zinc-300 font-mono">{workspaceId.slice(0, 12)}…</span>
                  </div>
                )}
                {(info?.started_at) && (
                  <div className="flex justify-between gap-2">
                    <span className="text-zinc-500">started</span>
                    <span className="text-zinc-300">{new Date(info.started_at).toLocaleTimeString()}</span>
                  </div>
                )}
                {typeof info?.cap === 'number' && (
                  <div className="flex justify-between gap-2">
                    <span className="text-zinc-500">cap</span>
                    <span className="text-zinc-300">{info.cap} concurrent</span>
                  </div>
                )}
                <div className="pt-1 text-[10px] text-zinc-600">
                  Cloud send moves this chat to the VPS on a cloud/branch. Preference is saved per chat.
                </div>
                {sendNote && <div className="text-[11px] text-zinc-300">{sendNote}</div>}
                <div className="flex items-center gap-2 pt-1">
                  <button
                    onClick={async (e) => {
                      e.stopPropagation();
                      if (!workspaceId || sending) return;
                      setSending(true);
                      setSendNote(null);
                      try {
                        const handoff = await sendToCloud({ workspace_id: workspaceId, chat_id: chatId });
                        setSendNote(
                          handoff.status === 'sent'
                            ? `Sent on ${handoff.branch}. Continue it on the VPS.`
                            : `Staged on ${handoff.branch}. VPS not configured — carry ${handoff.package} over.`,
                        );
                      } catch (error) {
                        setSendNote(error instanceof Error ? error.message : 'Send failed.');
                      } finally {
                        setSending(false);
                      }
                    }}
                    disabled={!workspaceId || sending}
                    className="px-2 py-1.5 rounded-lg bg-sky-500/20 border border-sky-500/30 text-sky-200 text-[11px] font-semibold hover:bg-sky-500/30 transition-colors disabled:opacity-40 cursor-pointer"
                  >
                    {sending ? 'Sending…' : 'Send to Cloud'}
                  </button>
                  <button
                    onClick={() => { window.location.hash = '#/cloud'; setOpen(false); }}
                    className="px-2 py-1.5 rounded-lg border border-zinc-700 text-zinc-300 text-[11px] font-medium hover:bg-white/[0.05] transition-colors cursor-pointer"
                  >
                    View runs
                  </button>
                </div>
              </div>
            )}
          </div>
        </div>,
        document.body,
      )}
    </>
  );
}
