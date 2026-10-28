import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  AlertTriangle, CheckCircle2, FolderKanban, FolderOpen, Info, PanelRightClose,
  PanelRightOpen, Pause, Play, Terminal, X,
} from 'lucide-react';
import type { GroupOp, GroupProjectDetail } from '../types';
import { fetchGroupProject, groupProjectOp, resolveGroupApproval } from '../services/api';
import NotesButton from '../components/Notes/NotesButton';
import GroupProjectChat from './group/GroupProjectChat';
import GroupProjectRail from './group/GroupProjectRail';
import { statusChipClass } from './group/groupProjectUi';

interface Props {
  projectId: string;
  onSwitchProject?: (projectId: string) => void;
  onClose: () => void;
}

const POLL_MS = 3000;

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

export default function GroupProjectModal({ projectId, onClose }: Props) {
  const [detail, setDetail] = useState<GroupProjectDetail | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [busyItem, setBusyItem] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  const [queued, setQueued] = useState<string | null>(null);
  const [opBusy, setOpBusy] = useState<GroupOp | null>(null);
  const [pathNote, setPathNote] = useState<string | null>(null);
  const [showRail, setShowRail] = useState(false);

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

  useEffect(() => {
    let cancelled = false;
    const first = window.setTimeout(() => { if (!cancelled) reload(); }, 0);
    const id = window.setInterval(() => {
      if (!cancelled && !document.hidden) reload();
    }, POLL_MS);
    return () => {
      cancelled = true;
      window.clearTimeout(first);
      window.clearInterval(id);
    };
  }, [reload]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (showRail) setShowRail(false);
        else onClose();
      }
    };
    document.addEventListener('keydown', onKey);
    const timer = window.setTimeout(() => (closeRef.current || panelRef.current)?.focus(), 0);
    return () => {
      document.removeEventListener('keydown', onKey);
      window.clearTimeout(timer);
    };
  }, [onClose, showRail]);

  const project = useMemo(() => detail?.project || {}, [detail]);
  const items = useMemo(() => detail?.items || [], [detail]);
  const milestones = useMemo(() => detail?.milestones || [], [detail]);
  const events = useMemo(() => detail?.events || [], [detail]);
  const messages = useMemo(() => detail?.messages || [], [detail]);
  const waiting = useMemo(
    () => items.filter((item) => item.blocked_reason === 'awaiting_approval'),
    [items],
  );

  const itemsDone = items.filter((item) => item.status === 'done').length;
  const milestonesDone = milestones.filter((m) => m.status === 'done').length;
  const status = String(project.status || 'unknown');

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
    try {
      await navigator.clipboard.writeText(detail?.path || '');
      setPathNote('Path copied.');
    } catch {
      setPathNote(detail?.path || '');
    }
  };

  return (
    <div
      className="fixed inset-0 z-[200] flex items-center justify-center bg-black/65 backdrop-blur-sm p-3 md:p-6"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-label={`Project ${projectId}`}
        className="modal-panel-enter gp-modal flex w-full max-w-[1180px] h-[min(860px,calc(100vh-32px))] flex-col overflow-hidden rounded-2xl border border-zinc-800 bg-[#111114] shadow-2xl outline-none"
      >
        <header className="gp-modal-header">
          <div className="gp-modal-identity">
            <div className="gp-modal-icon"><FolderKanban size={18} /></div>
            <div className="min-w-0 flex-1">
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <h2>{String(project.goal || projectId)}</h2>
                <span className={statusChipClass(status)}>{status}</span>
                {detail?.shell_enabled && (
                  <span className="gp-shell-badge" title="Members can run shell commands">
                    <Terminal size={9} /> shell
                  </span>
                )}
              </div>
              <div className="gp-modal-subline">
                <span>{projectId}</span>
                <span>·</span>
                <span>{milestonesDone}/{milestones.length} milestones</span>
                <span>·</span>
                <span>{itemsDone}/{items.length} items</span>
                {waiting.length > 0 && <span className="gp-modal-waiting">{waiting.length} waiting on you</span>}
              </div>
            </div>
          </div>

          <div className="gp-modal-actions">
            <button
              type="button"
              className="gp-icon-button gp-rail-toggle"
              onClick={() => setShowRail((value) => !value)}
              aria-label={showRail ? 'Hide details' : 'Show details'}
              title={showRail ? 'Hide details' : 'Show details'}
            >
              {showRail ? <PanelRightClose size={15} /> : <PanelRightOpen size={15} />}
            </button>
            <NotesButton scopeType="group" scopeId={projectId} />
            <button type="button" className="gp-icon-button" onClick={onOpenFolder} title={detail?.path || 'Open folder'} aria-label="Open folder">
              <FolderOpen size={15} />
            </button>
            {status === 'running' ? (
              <button type="button" className="gp-button" disabled={opBusy !== null} onClick={() => onOp('pause')}>
                <Pause size={12} /> {opBusy === 'pause' ? 'Queueing…' : 'Pause'}
              </button>
            ) : status === 'paused' ? (
              <button type="button" className="gp-button gp-button--primary" disabled={opBusy !== null} onClick={() => onOp('resume')}>
                <Play size={12} /> {opBusy === 'resume' ? 'Queueing…' : 'Resume'}
              </button>
            ) : null}
            {status !== 'done' && status !== 'closed' && (
              <button type="button" className="gp-button" disabled={opBusy !== null} onClick={() => onOp('close')}>
                {opBusy === 'close' ? 'Queueing…' : 'Close'}
              </button>
            )}
            <button
              ref={closeRef}
              type="button"
              className="gp-icon-button"
              onClick={onClose}
              aria-label="Close"
              title="Close"
            >
              <X size={16} />
            </button>
          </div>
        </header>

        {(actionError || queued || pathNote) && (
          <div className="gp-modal-notice">
            {actionError && <span className="is-error"><AlertTriangle size={12} /> {actionError}</span>}
            {queued && <span><CheckCircle2 size={12} /> {queued}</span>}
            {pathNote && <span>{pathNote}</span>}
          </div>
        )}
        {detail?.runtime_notice && (
          <div className="gp-modal-notice" role="note">
            <span><Info size={12} /> {detail.runtime_notice}</span>
          </div>
        )}

        <div className={`gp-layout flex-1 min-h-0 ${showRail ? 'is-rail-open' : ''}`}>
          {loading && !detail ? (
            <div className="flex-1 p-6"><Skeleton /></div>
          ) : error && !detail ? (
            <div className="flex-1 p-6 text-[13px] text-red-400">{error}</div>
          ) : (
            <>
              <GroupProjectChat
                messages={messages}
                events={events}
                items={items}
                waiting={waiting}
                busyItem={busyItem}
                onResolve={onResolve}
                loading={loading}
                error={error}
              />
              <GroupProjectRail
                project={project}
                path={detail?.path}
                shellEnabled={detail?.shell_enabled}
                milestones={milestones}
                items={items}
                events={events}
                members={detail?.members}
                waiting={waiting}
                busyItem={busyItem}
                onResolve={onResolve}
                onOpenFolder={onOpenFolder}
              />
            </>
          )}
        </div>
      </div>
    </div>
  );
}
