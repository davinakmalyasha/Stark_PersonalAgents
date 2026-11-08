import { useCallback, useEffect, useState } from 'react';
import { Cloud, CloudOff, Loader2, RefreshCw, Send } from 'lucide-react';
import {
  fetchChats,
  fetchCloudHandoffs,
  fetchCloudStatus,
  fetchWorkspaces,
  sendToCloud,
  type CloudHandoff,
  type CloudStatus,
} from '../services/api';
import type { ChatSummary, WorkspaceInfo } from '../types';

/**
 * Compact cloud panel for the Home side rail — same actions as the #/cloud
 * room, sized for the 248px panel. Status, send form, recent handoffs.
 */
export default function CloudPanel() {
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
      /* backend briefly unavailable: keep last state */
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
          ? `Sent on ${handoff.branch}.`
          : `Staged on ${handoff.branch} (VPS not configured).`,
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
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex items-center gap-1.5 px-3.5 pt-3 pb-2 shrink-0">
        {status && status.runs.length > 0
          ? <Cloud size={14} className="text-sky-300 animate-pulse" />
          : <CloudOff size={14} className="text-zinc-500" />}
        <span className="text-[15px] font-semibold tracking-tight text-zinc-100">Cloud</span>
        <button
          onClick={() => void refresh()}
          title="Refresh"
          className="ml-auto grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer"
        >
          <RefreshCw size={12} />
        </button>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto house-thread px-3.5 pb-3.5">
        <div className="rounded-lg border border-white/[0.07] bg-white/[0.02] px-2.5 py-2 text-[11px] space-y-1">
          <div className="flex justify-between gap-2">
            <span className="text-zinc-500">origin</span>
            <span className="font-mono text-zinc-300">{status?.origin ?? '…'}</span>
          </div>
          <div className="flex justify-between gap-2">
            <span className="text-zinc-500">running</span>
            <span className="font-mono text-zinc-300">
              {status ? `${status.runs.length} / ${status.cap}` : '…'}
            </span>
          </div>
          <div className="flex justify-between gap-2">
            <span className="text-zinc-500">VPS</span>
            <span className={`truncate font-mono ${status?.configured ? 'text-emerald-300' : 'text-amber-300'}`}>
              {status?.configured ? (status.cloud_host ?? 'set') : 'not set'}
            </span>
          </div>
        </div>

        <div className="mt-3 space-y-1.5">
          <label className="block text-[10px] uppercase tracking-widest text-zinc-500">
            Send to Cloud
          </label>
          <select
            value={workspaceId}
            onChange={(e) => { setWorkspaceId(e.target.value); setChatId(''); }}
            className="w-full rounded-md bg-zinc-900 border border-zinc-700/60 px-2 py-1.5 text-[11.5px] text-zinc-100"
          >
            {workspaces.map((w) => (
              <option key={w.id} value={w.id}>{w.name}</option>
            ))}
          </select>
          <select
            value={chatId}
            onChange={(e) => setChatId(e.target.value)}
            className="w-full rounded-md bg-zinc-900 border border-zinc-700/60 px-2 py-1.5 text-[11.5px] text-zinc-100"
          >
            <option value="">Whole workspace</option>
            {chats.map((c) => (
              <option key={c.id} value={c.id}>{c.title || 'Untitled'}</option>
            ))}
          </select>
          <input
            value={task}
            onChange={(e) => setTask(e.target.value)}
            placeholder="Task (optional)"
            className="w-full rounded-md bg-zinc-900 border border-zinc-700/60 px-2 py-1.5 text-[11.5px] text-zinc-100 placeholder:text-zinc-600"
          />
          <button
            onClick={() => void handleSend()}
            disabled={!workspaceId || busy}
            className="w-full flex items-center justify-center gap-1.5 rounded-md bg-sky-500/20 border border-sky-500/30 px-2 py-1.5 text-[11.5px] font-semibold text-sky-200 hover:bg-sky-500/30 transition-colors disabled:opacity-40 cursor-pointer"
          >
            {busy ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} />}
            {busy ? 'Sending…' : 'Send to Cloud'}
          </button>
          {notice && <div className="text-[10.5px] text-zinc-400 leading-4">{notice}</div>}
        </div>

        <div className="mt-3 pt-2 border-t border-white/[0.06]">
          <div className="text-[10px] uppercase tracking-widest text-zinc-500 mb-1.5">
            Handoffs
          </div>
          {handoffs.length === 0 ? (
            <div className="text-[11px] text-zinc-600 leading-4">
              Nothing sent yet. Handoffs carry the chat, its rollout, and workspace id.
            </div>
          ) : (
            <div className="space-y-1.5">
              {handoffs.slice(0, 6).map((h) => (
                <div key={h.id} className="rounded-md border border-white/[0.06] bg-white/[0.02] px-2 py-1.5">
                  <div className="flex items-center gap-1.5">
                    <span className={`px-1 py-px rounded border text-[9px] font-semibold ${
                      h.status === 'sent' || h.status === 'received'
                        ? 'text-emerald-300 border-emerald-500/30 bg-emerald-500/10'
                        : h.status === 'failed'
                          ? 'text-red-300 border-red-500/30 bg-red-500/10'
                          : 'text-sky-300 border-sky-500/30 bg-sky-500/10'
                    }`}>
                      {h.status}
                    </span>
                    <span className="truncate font-mono text-[10px] text-zinc-400">{h.branch}</span>
                  </div>
                  {h.task && <div className="mt-0.5 truncate text-[10.5px] text-zinc-500">{h.task}</div>}
                  {h.error && <div className="mt-0.5 truncate text-[10px] text-red-300">{h.error}</div>}
                </div>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
