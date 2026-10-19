import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  X, FolderOpen, Pause, Play, Check, AlertTriangle, Clock,
  ChevronRight, CheckCircle2, Terminal,
} from 'lucide-react';
import type {
  GroupEvent, GroupItem, GroupMilestone, GroupOp, GroupProjectDetail,
} from '../types';
import { fetchGroupProject, groupProjectOp, resolveGroupApproval } from '../services/api';

interface Props {
  projectId: string;
  /** Opens a project chosen from the panel — lets the user move between
   *  projects without closing and reopening the modal. */
  onSwitchProject?: (projectId: string) => void;
  onClose: () => void;
}

/** How often the open modal re-reads the board. The supervisor ticks every
 *  15s, so polling faster than this is pure waste. */
const POLL_MS = 3000;

const EVENT_LABELS: Record<string, string> = {
  'project.created': 'Project created',
  'project.paused': 'Paused',
  'project.resumed': 'Resumed',
  'project.closed': 'Closed',
  'project.stalled': 'Stalled — no possible progress',
  'milestone.created': 'Milestone added',
  'milestone.ready': 'Milestone started',
  'milestone.done': 'Milestone done',
  'milestone.failed': 'Milestone failed',
  'item.created': 'Item added',
  'item.ready': 'Item ready',
  'item.claimed': 'Item claimed',
  'item.released': 'Item released',
  'item.progress': 'Progress',
  'item.done': 'Item done',
  'item.failed': 'Item failed',
  'item.blocked': 'Item blocked',
  'item.invalidated': 'Item invalidated — plan was wrong',
  'item.cancelled': 'Item cancelled',
  'plan.revised': 'Plan revised',
  'idea.proposed': 'Idea proposed',
  'op.submitted': 'Change queued',
  'op.applied': 'Change applied',
  'op.rejected': 'Change rejected',
  'lease.expired': 'Worker stopped responding',
  'budget.warn': 'Budget warning',
  'budget.stop': 'Budget ceiling reached',
  'deadline.reached': 'Deadline reached',
  'approval.requested': 'Approval requested',
  'approval.granted': 'Approved',
  'approval.rejected': 'Rejected',
  'member.added': 'Member added',
  'member.removed': 'Member removed',
  'member.halted': 'Member halted',
  'capability.changed': 'Capability changed',
};

function eventLabel(kind: string): string {
  return EVENT_LABELS[kind] || kind;
}

function statusChipClass(status: string, awaiting = false): string {
  if (awaiting) return 'gp-chip is-waiting';
  switch (status) {
    case 'done': return 'gp-chip is-done';
    case 'doing': return 'gp-chip is-doing';
    case 'failed': return 'gp-chip is-failed';
    case 'blocked': return 'gp-chip is-blocked';
    case 'invalidated': return 'gp-chip is-invalidated';
    default: return 'gp-chip';
  }
}

function timeAgo(seconds: number): string {
  const ms = Date.now() - seconds * 1000;
  const mins = Math.floor(ms / 60000);
  if (mins < 1) return 'just now';
  if (mins < 60) return `${mins}m ago`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `${hours}h ago`;
  return `${Math.floor(hours / 24)}d ago`;
}

function money(value: number): string {
  return value >= 1 ? `$${value.toFixed(2)}` : `$${value.toFixed(4)}`;
}

// ── small pieces ────────────────────────────────────────────────────────────

function Skeleton() {
  return (
    <div className="space-y-3" aria-hidden="true">
      <div className="gp-skeleton h-4 w-2/3" />
      <div className="gp-skeleton h-3 w-1/2" />
      <div className="mt-6 space-y-2">
        <div className="gp-skeleton h-3 w-1/3" />
        <div className="gp-skeleton h-9 w-full" />
        <div className="gp-skeleton h-9 w-full" />
        <div className="gp-skeleton h-9 w-4/5" />
      </div>
    </div>
  );
}

function SectionTitle({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500 mb-2">
      {children}
    </div>
  );
}

export default function GroupProjectModal({ projectId, onClose }: Props) {
  const [detail, setDetail] = useState<GroupProjectDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyItem, setBusyItem] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [queued, setQueued] = useState<string | null>(null);
  const [opBusy, setOpBusy] = useState<GroupOp | null>(null);
  const [pathNote, setPathNote] = useState<string | null>(null);
  const [showEvents, setShowEvents] = useState(true);

  const panelRef = useRef<HTMLDivElement | null>(null);
  const closeRef = useRef<HTMLButtonElement | null>(null);

  const reload = useCallback(async () => {
    const next = await fetchGroupProject(projectId);
    if (next) {
      setDetail(next);
      setError(null);
    } else {
      setError('Could not read this project.');
    }
    setLoading(false);
  }, [projectId]);

  // Load, then poll. This component is mounted with a `key` per project, so
  // the initial state is already "loading, nothing yet" — no reset needed
  // here. The first call is deferred a tick for the same reason `useChat`
  // defers `refreshTasks`: calling it inline reads as a state update during
  // the effect body.
  useEffect(() => {
    let cancelled = false;
    const first = window.setTimeout(() => { if (!cancelled) reload(); }, 0);
    const id = window.setInterval(() => { if (!cancelled) reload(); }, POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(first);
      window.clearInterval(id);
    };
  }, [reload]);

  // Escape closes, and focus starts inside the dialog so the keyboard is
  // never stranded behind it.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
    document.addEventListener('keydown', onKey);
    const t = window.setTimeout(() => (closeRef.current || panelRef.current)?.focus(), 0);
    return () => { document.removeEventListener('keydown', onKey); window.clearTimeout(t); };
  }, [onClose]);

  // Memoised on `detail`: a bare `detail?.items || []` makes a fresh array
  // every render, so every downstream hook would re-run each time.
  const project = useMemo(() => detail?.project || {}, [detail]);
  const items = useMemo(() => detail?.items || [], [detail]);
  const milestones = useMemo(() => detail?.milestones || [], [detail]);
  const events = useMemo(() => detail?.events || [], [detail]);

  const waiting = useMemo(
    () => items.filter((i) => i.blocked_reason === 'awaiting_approval'),
    [items],
  );

  /** Why an item is asking, taken from the event log — approving blind is how
   *  mistakes happen, so the reason is shown rather than just the fact. */
  const approvalReason = useCallback((itemId: string): string => {
    for (let i = events.length - 1; i >= 0; i -= 1) {
      const e = events[i];
      if (e.kind === 'approval.requested' && e.item_id === itemId) {
        return String((e.data?.detail as string) || '');
      }
    }
    return '';
  }, [events]);

  const itemsDone = items.filter((i) => i.status === 'done').length;
  const milestonesDone = milestones.filter((m) => m.status === 'done').length;
  const spent = Number(project.spent_usd || 0);
  const cap = project.budget_usd ? Number(project.budget_usd) : null;

  const onResolve = async (itemId: string, grant: boolean) => {
    setBusyItem(itemId);
    setActionError(null);
    try {
      await resolveGroupApproval(projectId, itemId, grant);
      await reload();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : 'Failed to reach the project.');
    } finally {
      setBusyItem(null);
    }
  };

  const onOp = async (op: GroupOp) => {
    setOpBusy(op);
    setActionError(null);
    setQueued(null);
    try {
      await groupProjectOp(projectId, op);
      setQueued(`${op === 'close' ? 'Close' : op === 'pause' ? 'Pause' : 'Resume'} queued — applies within ~15s.`);
      await reload();
    } catch (err) {
      setActionError(err instanceof Error ? err.message : `Failed to ${op}.`);
    } finally {
      setOpBusy(null);
    }
  };

  const onOpenFolder = async () => {
    setPathNote(null);
    const reveal = window.antigravity?.revealPath;
    if (reveal && detail?.path) {
      const res = await reveal(detail.path);
      if (!res?.ok) setPathNote(res?.error || 'Could not open the folder.');
      return;
    }
    // Browser fallback: no shell to hand a folder to, so hand over the path.
    try {
      await navigator.clipboard.writeText(detail?.path || '');
      setPathNote('Path copied.');
    } catch {
      setPathNote(detail?.path || '');
    }
  };

  const status = String(project.status || 'unknown');

  return (
    <div
      className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={`Project ${projectId}`}
        className="modal-panel-enter w-full max-w-2xl max-h-[85vh] flex flex-col rounded-2xl border border-zinc-800 bg-[#141417] shadow-2xl overflow-hidden outline-none"
      >
        {/* ── header ── */}
        <div className="flex items-start gap-2.5 px-5 py-4 border-b border-zinc-800">
          <div className="flex-1 min-w-0">
            <div className="flex items-center gap-2">
              <span className="text-[14px] font-bold text-zinc-100 truncate">
                {(project.goal as string) || projectId}
              </span>
              <span className={statusChipClass(status)}>{status}</span>
              {detail?.shell_enabled && (
                <span
                  className="gp-shell-badge"
                  title="Members of this project can run shell commands, so the rest of its envelope is advisory"
                >
                  <Terminal size={9} aria-hidden="true" />
                  shell
                </span>
              )}
            </div>
            <div className="text-[11px] text-zinc-500 mt-0.5 truncate">{projectId}</div>
          </div>
          <button
            ref={closeRef}
            type="button"
            onClick={onClose}
            className="p-1.5 text-zinc-500 hover:text-zinc-300 rounded-md focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/50"
            aria-label="Close"
          >
            <X size={15} />
          </button>
        </div>

        {/* ── body ── */}
        <div className="gp-modal-scroll px-5 py-4 overflow-y-auto flex-1 min-h-0">
          {loading && !detail ? (
            <Skeleton />
          ) : error ? (
            <div className="text-[12.5px] text-red-400">{error}</div>
          ) : (
            <div className="space-y-5">
              {/* progress */}
              <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-[12px] text-zinc-400 tabular-nums">
                <span>milestones <b className="text-zinc-200">{milestonesDone}/{milestones.length}</b></span>
                <span>items <b className="text-zinc-200">{itemsDone}/{items.length}</b></span>
                <span>
                  spend <b className="text-zinc-200">{money(spent)}</b>
                  {cap ? <span className="text-zinc-600">/{money(cap)}</span> : null}
                </span>
                {typeof project.deadline === 'number' && project.deadline > 0 ? (
                  <span className="flex items-center gap-1">
                    <Clock size={11} />
                    {new Date(project.deadline * 1000).toLocaleString()}
                  </span>
                ) : null}
              </div>

              {project.halt_reason ? (
                <div className="flex items-start gap-2 rounded-lg border border-amber-500/30 bg-amber-500/10 px-3 py-2">
                  <AlertTriangle size={14} className="text-amber-400 mt-0.5 flex-shrink-0" />
                  <span className="text-[12px] text-amber-200">{String(project.halt_reason)}</span>
                </div>
              ) : null}

              {/* waiting on you — the whole point of the board */}
              {waiting.length > 0 && (
                <div>
                  <SectionTitle>
                    Waiting on you · {waiting.length}
                  </SectionTitle>
                  <div className="space-y-2">
                    {waiting.map((item) => (
                      <div
                        key={item.id}
                        className="rounded-lg border border-amber-500/30 bg-amber-500/[0.07] px-3 py-2.5"
                      >
                        <div className="flex items-start gap-2">
                          <AlertTriangle size={14} className="text-amber-400 mt-0.5 flex-shrink-0" />
                          <div className="min-w-0 flex-1">
                            <div className="text-[13px] text-zinc-100 font-medium break-words">
                              {item.title}
                            </div>
                            <div className="text-[11px] text-zinc-500 mt-0.5">{item.id}</div>
                            {approvalReason(item.id) ? (
                              <div className="text-[12px] text-zinc-400 mt-1.5 break-words">
                                {approvalReason(item.id)}
                              </div>
                            ) : null}
                          </div>
                        </div>
                        <div className="flex items-center gap-2 mt-2.5 pl-6">
                          <button
                            type="button"
                            disabled={busyItem === item.id}
                            onClick={() => onResolve(item.id, true)}
                            className="flex items-center gap-1.5 h-8 px-3 rounded-lg bg-white text-[#0a0a0d] text-[12px] font-semibold hover:bg-zinc-200 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-default"
                          >
                            <Check size={13} />
                            {busyItem === item.id ? 'Working…' : 'Approve'}
                          </button>
                          <button
                            type="button"
                            disabled={busyItem === item.id}
                            onClick={() => onResolve(item.id, false)}
                            className="h-8 px-3 rounded-lg border border-zinc-700 text-zinc-300 text-[12px] font-medium hover:bg-white/[0.06] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-default"
                          >
                            Reject
                          </button>
                          <span className="text-[11px] text-zinc-600">
                            Rejecting blocks the item for a re-plan; it is not a retry.
                          </span>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {/* the board */}
              <div>
                <SectionTitle>Board</SectionTitle>
                <div className="space-y-3">
                  {milestones.map((m: GroupMilestone) => {
                    const mine = items.filter((i) => i.milestone_id === m.id);
                    return (
                      <div key={m.id}>
                        <div className="flex items-center gap-2">
                          <ChevronRight size={12} className="text-zinc-600 flex-shrink-0" />
                          <span className="text-[12.5px] text-zinc-300 font-medium truncate">{m.title}</span>
                          <span className={statusChipClass(m.status)}>{m.status}</span>
                        </div>
                        <div className="mt-1.5 ml-5 space-y-1">
                          {mine.map((item: GroupItem) => {
                            const isWaiting = item.blocked_reason === 'awaiting_approval';
                            const working = item.status === 'doing' && item.lease_owner;
                            return (
                              <div
                                key={item.id}
                                className="flex items-center gap-2 rounded-md border border-zinc-800/70 bg-zinc-900/30 px-2.5 py-1.5"
                              >
                                <span className="text-[11px] text-zinc-600 tabular-nums flex-shrink-0 w-12 truncate">
                                  {item.id}
                                </span>
                                <span className="text-[12px] text-zinc-300 truncate flex-1 min-w-0" title={item.title}>
                                  {item.title}
                                </span>
                                {working ? (
                                  <span className="text-[10px] text-zinc-500 truncate max-w-[100px]">
                                    {item.lease_owner}
                                  </span>
                                ) : null}
                                {item.blocked_reason && !isWaiting ? (
                                  <span className="text-[10px] text-zinc-500 truncate max-w-[120px]">
                                    {item.blocked_reason}
                                  </span>
                                ) : null}
                                {typeof item.cost_usd === 'number' && item.cost_usd > 0 ? (
                                  <span className="text-[10px] text-zinc-600 tabular-nums flex-shrink-0">
                                    {money(item.cost_usd)}
                                  </span>
                                ) : null}
                                <span className={statusChipClass(item.status, isWaiting)}>
                                  {isWaiting ? 'needs you' : item.status}
                                </span>
                              </div>
                            );
                          })}
                          {mine.length === 0 && (
                            <div className="text-[11px] text-zinc-600 px-2.5">No items yet.</div>
                          )}
                        </div>
                      </div>
                    );
                  })}
                  {milestones.length === 0 && (
                    <div className="text-[12px] text-zinc-600">No milestones yet.</div>
                  )}
                </div>
              </div>

              {/* what happened while you were away */}
              <div>
                <button
                  type="button"
                  onClick={() => setShowEvents((v) => !v)}
                  className="flex items-center gap-1.5 text-[10px] uppercase tracking-widest font-semibold text-zinc-500 hover:text-zinc-300 transition-colors cursor-pointer focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/50 rounded"
                  aria-expanded={showEvents}
                >
                  <ChevronRight
                    size={11}
                    className={`transition-transform ${showEvents ? 'rotate-90' : ''}`}
                  />
                  Activity · {events.length}
                </button>
                {showEvents && (
                  <div className="mt-2 space-y-0.5">
                    {events.slice(-40).reverse().map((e: GroupEvent) => (
                      <div key={e.seq} className="flex items-baseline gap-2 text-[11.5px]">
                        <span className="text-zinc-600 tabular-nums flex-shrink-0 w-14 text-right">
                          {timeAgo(e.ts)}
                        </span>
                        <span className="text-zinc-400 gp-event-kind truncate">
                          {eventLabel(e.kind)}
                        </span>
                        {e.item_id ? (
                          <span className="text-zinc-600 truncate">{e.item_id}</span>
                        ) : null}
                        <span className="text-zinc-700 truncate ml-auto flex-shrink-0">{e.actor}</span>
                      </div>
                    ))}
                    {events.length === 0 && (
                      <div className="text-[11.5px] text-zinc-600">Nothing logged yet.</div>
                    )}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>

        {/* ── footer ── */}
        <div className="px-5 py-3 border-t border-zinc-800 space-y-2">
          {actionError ? (
            <div className="text-[11.5px] text-red-400">{actionError}</div>
          ) : null}
          {queued ? (
            <div className="flex items-center gap-1.5 text-[11.5px] text-zinc-400">
              <CheckCircle2 size={12} className="text-zinc-500" />
              {queued}
            </div>
          ) : null}

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={onOpenFolder}
              className="flex items-center gap-1.5 h-8 px-2.5 rounded-lg text-[12px] text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.06] transition-colors cursor-pointer focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/50"
              title={detail?.path || ''}
            >
              <FolderOpen size={13} />
              Open folder
            </button>
            {pathNote ? (
              <span className="text-[11px] text-zinc-600 truncate max-w-[220px]">{pathNote}</span>
            ) : null}

            <div className="flex-1" />

            {status === 'running' ? (
              <button
                type="button"
                disabled={opBusy !== null}
                onClick={() => onOp('pause')}
                className="flex items-center gap-1.5 h-8 px-3 rounded-lg border border-zinc-700 text-[12px] text-zinc-300 hover:bg-white/[0.06] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-default"
              >
                <Pause size={12} />
                {opBusy === 'pause' ? 'Queueing…' : 'Pause'}
              </button>
            ) : status === 'paused' ? (
              <button
                type="button"
                disabled={opBusy !== null}
                onClick={() => onOp('resume')}
                className="flex items-center gap-1.5 h-8 px-3 rounded-lg bg-white text-[#0a0a0d] text-[12px] font-semibold hover:bg-zinc-200 transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-default"
              >
                <Play size={12} />
                {opBusy === 'resume' ? 'Queueing…' : 'Resume'}
              </button>
            ) : null}

            {status !== 'done' && status !== 'closed' ? (
              <button
                type="button"
                disabled={opBusy !== null}
                onClick={() => onOp('close')}
                className="h-8 px-3 rounded-lg border border-zinc-700 text-[12px] text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.06] transition-colors cursor-pointer disabled:opacity-50 disabled:cursor-default"
              >
                {opBusy === 'close' ? 'Queueing…' : 'Close'}
              </button>
            ) : null}
          </div>
        </div>
      </div>
    </div>
  );
}
