import { useCallback, useEffect, useState } from 'react';
import { ArrowLeft, Cloud, CloudOff, Loader2, RefreshCw, Send } from 'lucide-react';
import WindowControls from '../components/Layout/WindowControls';
import {
  fetchChats,
  fetchCloudHandoffs,
  fetchCloudStatus,
  fetchWorkspaces,
  sendToCloud,
  type ChatSummary,
  type CloudHandoff,
  type CloudStatus,
  type WorkspaceInfo,
} from '../services/api';

function formatTime(value?: string | null): string {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(date);
}

function statusColor(status: string): string {
  switch (status) {
    case 'received':
    case 'sent':
      return 'text-emerald-300 border-emerald-500/30 bg-emerald-500/10';
    case 'staged':
      return 'text-sky-300 border-sky-500/30 bg-sky-500/10';
    case 'failed':
      return 'text-red-300 border-red-500/30 bg-red-500/10';
    default:
      return 'text-zinc-300 border-zinc-700 bg-zinc-800/60';
  }
}

export default function CloudRuns() {
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [handoffs, setHandoffs] = useState<CloudHandoff[]>([]);
  const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [chatsByWs, setChatsByWs] = useState<Record<string, ChatSummary[]>>({});
  const [workspaceId, setWorkspaceId] = useState('');
  const [chatId, setChatId] = useState('');
  const [task, setTask] = useState('');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextHandoffs, wsList] = await Promise.all([
        fetchCloudStatus(),
        fetchCloudHandoffs(),
        fetchWorkspaces(),
      ]);
      setStatus(nextStatus);
      setHandoffs(nextHandoffs);
      setWorkspaces(wsList);
      if (!workspaceId && wsList.length > 0) setWorkspaceId(wsList[0].id);
      const pairs = await Promise.all(
        wsList.map(async (w) => [w.id, await fetchChats(w.id).catch(() => [])] as const),
      );
      const byWs: Record<string, ChatSummary[]> = {};
      for (const [id, chats] of pairs) byWs[id] = chats;
      setChatsByWs(byWs);
    } catch {
      /* backend unreachable: keep last state */
    }
  }, [workspaceId]);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => void refresh(), 10_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const chats = chatsByWs[workspaceId] ?? [];

  const handleSend = async () => {
    if (!workspaceId || busy) return;
    setBusy(true);
    setNotice(null);
    try {
      const handoff = await sendToCloud({
        workspace_id: workspaceId,
        chat_id: chatId || undefined,
        task: task.trim() || undefined,
      });
      setNotice(
        handoff.status === 'sent'
          ? `Sent to VPS on ${handoff.branch}. Continue it there — same chat id.`
          : `Staged as ${handoff.branch}. VPS not configured: import ${handoff.package} on the VPS via POST /api/cloud/receive.`,
      );
      setTask('');
      await refresh();
    } catch (error) {
      setNotice(error instanceof Error ? error.message : 'Send failed.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col h-screen w-screen bg-[#0c0c0e] text-zinc-100 overflow-hidden font-sans">
      <header className="flex h-[52px] shrink-0 items-center justify-between border-b border-white/10 px-4 sm:px-6">
        <div className="flex items-center gap-3">
          <button
            onClick={() => { window.location.hash = ''; }}
            title="Back to home"
            className="p-2 rounded-lg text-zinc-400 hover:text-zinc-100 hover:bg-white/[0.06] transition-colors cursor-pointer"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="flex items-center gap-2">
            {status && status.runs.length > 0
              ? <Cloud size={16} className="text-sky-300 animate-pulse" />
              : <CloudOff size={16} className="text-zinc-500" />}
            <h1 className="text-[14px] font-bold">Cloud runs</h1>
          </div>
        </div>
        <div className="flex items-center gap-2">
          <span className="text-[11px] text-zinc-500">
            {status ? `${status.origin} · cap ${status.cap}` : 'connecting…'}
          </span>
          <button
            onClick={() => void refresh()}
            title="Refresh"
            className="p-2 rounded-lg text-zinc-400 hover:text-zinc-100 hover:bg-white/[0.06] transition-colors cursor-pointer"
          >
            <RefreshCw size={14} />
          </button>
          <WindowControls />
        </div>
      </header>

      <div className="flex-1 min-h-0 overflow-y-auto px-4 sm:px-6 py-4 space-y-4">
        {/* Status */}
        <section className="rounded-xl border border-white/[0.08] bg-[#141417] p-4">
          <div className="text-[12px] font-bold mb-2">This machine</div>
          {status ? (
            <div className="grid grid-cols-2 sm:grid-cols-4 gap-3 text-[12px]">
              <div>
                <div className="text-zinc-500 text-[10px] uppercase tracking-widest">Origin</div>
                <div className="font-mono">{status.origin}</div>
              </div>
              <div>
                <div className="text-zinc-500 text-[10px] uppercase tracking-widest">Running</div>
                <div className="font-mono">{status.runs.length} / {status.cap}</div>
              </div>
              <div>
                <div className="text-zinc-500 text-[10px] uppercase tracking-widest">VPS target</div>
                <div className="font-mono">{status.configured ? (status.cloud_host ?? 'set') : 'not set'}</div>
              </div>
              <div>
                <div className="text-zinc-500 text-[10px] uppercase tracking-widest">Handoffs</div>
                <div className="font-mono">{handoffs.length}</div>
              </div>
            </div>
          ) : (
            <div className="text-[12px] text-zinc-500">Loading…</div>
          )}
          {!status?.configured && (
            <div className="mt-2 text-[11px] text-zinc-500">
              Set <span className="font-mono">STARK_CLOUD_URL</span> + <span className="font-mono">STARK_CLOUD_TOKEN</span> (a VPS pairing-device token) to push directly; otherwise handoffs stage a package you carry over.
            </div>
          )}
        </section>

        {/* Send */}
        <section className="rounded-xl border border-white/[0.08] bg-[#141417] p-4">
          <div className="text-[12px] font-bold mb-3">Send to Cloud</div>
          <div className="grid sm:grid-cols-2 gap-3">
            <label className="block text-[11px] text-zinc-400">
              Workspace
              <select
                value={workspaceId}
                onChange={(e) => { setWorkspaceId(e.target.value); setChatId(''); }}
                className="mt-1 w-full rounded-lg bg-zinc-900 border border-zinc-700/60 px-2 py-2 text-[12px] text-zinc-100"
              >
                {workspaces.map((w) => (
                  <option key={w.id} value={w.id}>{w.name}</option>
                ))}
              </select>
            </label>
            <label className="block text-[11px] text-zinc-400">
              Chat (optional — whole workspace when empty)
              <select
                value={chatId}
                onChange={(e) => setChatId(e.target.value)}
                className="mt-1 w-full rounded-lg bg-zinc-900 border border-zinc-700/60 px-2 py-2 text-[12px] text-zinc-100"
              >
                <option value="">Whole workspace</option>
                {chats.map((c) => (
                  <option key={c.id} value={c.id}>{c.title || 'Untitled'}</option>
                ))}
              </select>
            </label>
          </div>
          <label className="block mt-3 text-[11px] text-zinc-400">
            Task for the cloud agent (optional)
            <input
              value={task}
              onChange={(e) => setTask(e.target.value)}
              placeholder="e.g. finish the failing tests on cloud/fix-login-…"
              className="mt-1 w-full rounded-lg bg-zinc-900 border border-zinc-700/60 px-2 py-2 text-[12px] text-zinc-100 placeholder:text-zinc-600"
            />
          </label>
          <div className="mt-3 flex items-center gap-2">
            <button
              onClick={() => void handleSend()}
              disabled={!workspaceId || busy}
              className="flex items-center gap-1.5 px-3 py-2 rounded-lg bg-sky-500/20 border border-sky-500/30 text-sky-200 text-[12px] font-semibold hover:bg-sky-500/30 transition-colors disabled:opacity-40 cursor-pointer"
            >
              {busy ? <Loader2 size={14} className="animate-spin" /> : <Send size={14} />}
              {busy ? 'Sending…' : 'Send to Cloud'}
            </button>
            {notice && <span className="text-[11px] text-zinc-400">{notice}</span>}
          </div>
        </section>

        {/* Handoffs */}
        <section className="rounded-xl border border-white/[0.08] bg-[#141417] p-4">
          <div className="text-[12px] font-bold mb-3">Handoffs</div>
          {handoffs.length === 0 ? (
            <div className="text-[12px] text-zinc-500">No handoffs yet. Send a chat above and it lands here with its branch.</div>
          ) : (
            <div className="space-y-2">
              {handoffs.map((h) => (
                <div key={h.id} className="rounded-lg border border-white/[0.06] bg-white/[0.02] p-3">
                  <div className="flex items-center gap-2">
                    <span className={`px-1.5 py-0.5 rounded border text-[10px] font-semibold ${statusColor(h.status)}`}>
                      {h.status}
                    </span>
                    <span className="font-mono text-[11px] text-zinc-200">{h.branch}</span>
                    <span className="ml-auto text-[10px] text-zinc-600">{formatTime(h.updated_at)}</span>
                  </div>
                  <div className="mt-1.5 grid sm:grid-cols-3 gap-1 text-[11px] text-zinc-500">
                    <span className="truncate">chat: <span className="font-mono text-zinc-400">{h.chat_id?.slice(0, 12) ?? '—'}</span></span>
                    <span className="truncate">ws: <span className="font-mono text-zinc-400">{h.workspace_id.slice(0, 12)}</span></span>
                    <span className="truncate">from: <span className="font-mono text-zinc-400">{h.origin}</span></span>
                  </div>
                  {h.task && <div className="mt-1 text-[11px] text-zinc-400">{h.task}</div>}
                  {h.error && <div className="mt-1 text-[11px] text-red-300">{h.error}</div>}
                </div>
              ))}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
