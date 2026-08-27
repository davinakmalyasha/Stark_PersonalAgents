import { useCallback, useEffect, useState } from 'react';
import { ChevronRight, ChevronDown, LoaderCircle, Wrench, ArrowLeft } from 'lucide-react';
import type { SubagentSession, SubagentSessionEntry } from '../types';
import { fetchSubagentSession } from '../services/api';
import Markdown from '../components/Workspace/Markdown';

const POLL_MS = 2500;

interface SubagentSessionViewProps {
  workspaceId: string;
  childId: string;
  /** Fallback label while the session loads (from the clicked block). */
  label: string;
  parentTitle: string;
  onBack: () => void;
}

function ToolRow({ entry }: { entry: SubagentSessionEntry }) {
  const [open, setOpen] = useState(false);
  const firstLine = (entry.content || '').split('\n')[0] || '(no output)';
  return (
    <div className="rounded-lg border border-white/[0.06] bg-white/[0.02] overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left cursor-pointer hover:bg-white/[0.04] transition-colors"
      >
        {open
          ? <ChevronDown size={11} className="text-zinc-600 shrink-0" />
          : <ChevronRight size={11} className="text-zinc-600 shrink-0" />}
        <span className={`grid place-items-center w-4 h-4 rounded ${entry.isError ? 'text-red-400' : 'text-zinc-500'}`}>
          <Wrench size={10} />
        </span>
        <span className="text-[11.5px] font-medium text-zinc-400 shrink-0">{entry.toolName || 'tool'}</span>
        <span className="text-[11px] text-zinc-600 truncate flex-1">{firstLine}</span>
      </button>
      {open && (
        <pre className="px-3 py-2 text-[11px] leading-5 text-zinc-400 whitespace-pre-wrap max-h-56 overflow-y-auto border-t border-white/[0.05] font-mono">
          {entry.content}
        </pre>
      )}
    </div>
  );
}

/**
 * Read-only drill-in for one spawned subagent session: breadcrumb
 * (parent / label), the child's transcript, and a disabled dock —
 * "Subagent sessions cannot be prompted. Back to main session."
 * Polls while the child reports status=running.
 */
export default function SubagentSessionView({
  workspaceId, childId, label, parentTitle, onBack,
}: SubagentSessionViewProps) {
  const [session, setSession] = useState<SubagentSession | null>(null);
  const [loadedOnce, setLoadedOnce] = useState(false);

  // Reset when a different child is opened (state-tracked previous key)
  const [prevKey, setPrevKey] = useState(`${workspaceId}:${childId}`);
  if (prevKey !== `${workspaceId}:${childId}`) {
    setPrevKey(`${workspaceId}:${childId}`);
    setSession(null);
    setLoadedOnce(false);
  }

  const load = useCallback(() => {
    fetchSubagentSession(workspaceId, childId)
      .then((data) => { if (data) setSession(data); })
      .catch(() => {})
      .finally(() => setLoadedOnce(true));
  }, [workspaceId, childId]);

  useEffect(() => { load(); }, [load]);

  const failed = loadedOnce && !session;

  const running = session?.info.status === 'running';
  useEffect(() => {
    if (!running) return;
    const id = window.setInterval(load, POLL_MS);
    return () => window.clearInterval(id);
  }, [running, load]);

  // Session label wins when it carries a real task label; descriptor-less
  // sub- sessions only know the agent name — keep the clicked block's label.
  const sessionLabel = session?.info.label || '';
  const sessionName = session?.info.name || '';
  const displayLabel = sessionLabel && sessionLabel !== sessionName ? sessionLabel : (label || sessionLabel);

  return (
    <div className="h-full min-h-0 flex flex-col">
      {/* ── Breadcrumb: parent (back) / subagent label ── */}
      <div className="flex items-center gap-1.5 px-1 py-2 shrink-0 select-none min-w-0">
        <button
          type="button"
          onClick={onBack}
          title="Back to main session"
          className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer shrink-0"
        >
          <ArrowLeft size={13} />
        </button>
        <button
          type="button"
          onClick={onBack}
          className="text-[12.5px] text-zinc-500 hover:text-zinc-300 truncate max-w-[45%] transition-colors cursor-pointer"
        >
          {parentTitle}
        </button>
        <span className="text-zinc-700 text-[12px] shrink-0">/</span>
        <span className="text-[12.5px] font-semibold text-zinc-200 truncate">
          {displayLabel}
        </span>
        {running && (
          <span className="flex items-center gap-1.5 shrink-0 ml-1">
            <span className="session-run-dot" title="Running" />
            <span className="text-[10px] text-zinc-500">running</span>
          </span>
        )}
      </div>

      {/* ── Transcript ── */}
      <div className="flex-1 min-h-0 overflow-y-auto house-thread px-1 pt-1 pb-4">
        {failed && !session ? (
          <div className="h-full flex flex-col items-center justify-center gap-2 text-zinc-600 py-6">
            <p className="text-[12px]">Subagent session not found.</p>
            <button type="button" onClick={onBack} className="text-[11.5px] text-zinc-400 hover:text-white underline underline-offset-2 cursor-pointer">
              Back to main session
            </button>
          </div>
        ) : !session ? (
          <div className="h-full flex items-center justify-center py-6">
            <LoaderCircle size={18} className="text-zinc-600 animate-spin" />
          </div>
        ) : (
          <div className="flex flex-col gap-3.5">
            {session.entries.length === 0 && (
              <p className="text-[12px] text-zinc-600 text-center py-6">No transcript recorded yet.</p>
            )}
            {session.entries.map((entry, i) => {
              if (entry.kind === 'user') {
                return (
                  <div key={i} className="flex justify-end house-enter">
                    <div className="max-w-[85%] px-3.5 py-2 rounded-2xl rounded-br-xs bg-white/[0.09] border border-white/[0.08] text-[13px] leading-6 text-zinc-100 whitespace-pre-wrap">
                      {entry.content}
                    </div>
                  </div>
                );
              }
              if (entry.kind === 'tool') {
                return <ToolRow key={i} entry={entry} />;
              }
              return (
                <div key={i} className="text-[13px] leading-6 text-zinc-200 house-enter">
                  <Markdown content={entry.content} />
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* ── Disabled dock ── */}
      <div className="shrink-0 pt-1">
        <button
          type="button"
          onClick={onBack}
          className="w-full flex items-center justify-center gap-2 rounded-xl border house-hairline bg-white/[0.03] hover:bg-white/[0.06] px-4 py-3 text-[12.5px] text-zinc-500 hover:text-zinc-300 transition-colors cursor-pointer"
        >
          <span className="truncate">Subagent sessions cannot be prompted. Back to main session.</span>
        </button>
      </div>
    </div>
  );
}
