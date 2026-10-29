import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  Bot,
  Brain,
  ChevronDown,
  ChevronRight,
  GitBranch,
  Loader2,
  Mail,
  MoreHorizontal,
  Send,
  X,
} from 'lucide-react';
import type { AgentDetail, AgentNode } from '../../types';
import {
  cancelAgent,
  discardAgent,
  fetchAgent,
  fetchAgentDiff,
  fetchAgents,
  mergeAgent,
  sendAgentMessage,
} from '../../services/api';

interface Props {
  collapsed: boolean;
  onOpen: (agent: AgentNode) => void;
  refreshSignal?: number;
  workspaceId?: string | null;
}

const STATUS_COLOR: Record<string, string> = {
  running: 'bg-emerald-400',
  queued: 'bg-amber-400',
  idle: 'bg-zinc-500',
  interrupted: 'bg-orange-400',
  completed: 'bg-sky-400',
  failed: 'bg-red-400',
  cancelled: 'bg-zinc-600',
};

function relativeTime(value?: string | null): string {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const seconds = Math.max(0, Math.round((Date.now() - date.getTime()) / 1000));
  if (seconds < 10) return 'just now';
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  return `${days}d ago`;
}

function accessBadge(agent: AgentNode): string | null {
  const access = (agent.access || '').toLowerCase();
  if (access === 'read') return 'read-only';
  if (access === 'worktree') return 'isolated';
  if (access === 'write' && agent.worktree?.path) return 'isolated';
  if (access === 'write') return 'direct';
  if (agent.worktree?.path) return 'isolated';
  return null;
}

type DrawerTab = 'overview' | 'mailbox' | 'worktree';

export default function AgentTreePanel({ collapsed, onOpen, refreshSignal = 0, workspaceId }: Props) {
  const [agents, setAgents] = useState<AgentNode[]>([]);
  const [open, setOpen] = useState(true);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<AgentDetail | null>(null);
  const [detailTab, setDetailTab] = useState<DrawerTab>('overview');
  const [detailBusy, setDetailBusy] = useState(false);
  const [diffBusy, setDiffBusy] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [actionNotice, setActionNotice] = useState<string | null>(null);
  const [messageDraft, setMessageDraft] = useState('');
  const [confirmAction, setConfirmAction] = useState<'merge' | 'discard' | null>(null);

  const load = useCallback(() => {
    fetchAgents().then(setAgents).catch(() => setAgents([]));
  }, []);

  useEffect(() => {
    load();
    const timer = window.setInterval(() => {
      if (!document.hidden) load();
    }, 5000);
    const onVisible = () => {
      if (!document.hidden) load();
    };
    document.addEventListener('visibilitychange', onVisible);
    return () => {
      window.clearInterval(timer);
      document.removeEventListener('visibilitychange', onVisible);
    };
  }, [load, refreshSignal]);

  const loadDetail = useCallback(async (agentId: string) => {
    setDetailBusy(true);
    setActionError(null);
    try {
      const next = await fetchAgent(agentId);
      setDetail(next);
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setDetailBusy(false);
    }
  }, []);

  useEffect(() => {
    if (!selectedId) {
      setDetail(null);
      return;
    }
    void loadDetail(selectedId);
  }, [selectedId, loadDetail, refreshSignal]);

  const children = useMemo(() => {
    const map = new Map<string | null, AgentNode[]>();
    for (const agent of agents) {
      const key = agent.parentId ?? null;
      map.set(key, [...(map.get(key) ?? []), agent]);
    }
    return map;
  }, [agents]);

  if (collapsed) return null;

  const toggle = (id: string) => {
    setExpanded((previous) => {
      const next = new Set(previous);
      if (next.has(id)) next.delete(id); else next.add(id);
      return next;
    });
  };

  const refreshAll = useCallback(async () => {
    load();
    if (selectedId) await loadDetail(selectedId);
  }, [load, loadDetail, selectedId]);

  const sendMessage = async () => {
    if (!selectedId || !messageDraft.trim()) return;
    setDetailBusy(true);
    setActionError(null);
    setActionNotice(null);
    try {
      const result = await sendAgentMessage(selectedId, messageDraft.trim());
      if (!result.ok) throw new Error(result.error || 'Send failed');
      setMessageDraft('');
      setActionNotice(result.steered ? 'Steered the running agent.' : 'Queued for the next run.');
      await refreshAll();
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setDetailBusy(false);
    }
  };

  const runMerge = async () => {
    if (!selectedId) return;
    setDetailBusy(true);
    setActionError(null);
    try {
      // Show the latest diff before merging.
      await fetchAgentDiff(selectedId).catch(() => null);
      const result = await mergeAgent(selectedId);
      if (!result.ok) throw new Error(result.error || 'Merge failed');
      setActionNotice('Worktree merged.');
      setConfirmAction(null);
      await refreshAll();
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setDetailBusy(false);
    }
  };

  const runDiscard = async () => {
    if (!selectedId) return;
    setDetailBusy(true);
    setActionError(null);
    try {
      const result = await discardAgent(selectedId);
      if (!result.ok) throw new Error(result.error || 'Discard failed');
      setActionNotice('Worktree discarded.');
      setConfirmAction(null);
      await refreshAll();
    } catch (cause) {
      setActionError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setDetailBusy(false);
    }
  };

  const openDetail = (agent: AgentNode) => {
    setSelectedId(agent.id);
    setDetailTab('overview');
    setActionError(null);
    setActionNotice(null);
    setConfirmAction(null);
  };

  const render = (parent: string | null, depth: number) => {
    const rows = children.get(parent) ?? [];
    return rows.map((agent) => {
      const kids = children.get(agent.id) ?? [];
      const isOpen = expanded.has(agent.id);
      const status = STATUS_COLOR[agent.status] ?? 'bg-zinc-500';
      const badge = accessBadge(agent);
      return (
        <div key={agent.id}>
          <div
            className="group flex items-center gap-1.5 rounded-md py-1 pr-1 text-[11px] text-zinc-400 hover:bg-white/[0.04]"
            style={{ paddingLeft: 10 + depth * 12 }}
          >
            {kids.length > 0 ? (
              <button onClick={() => toggle(agent.id)} className="text-zinc-500 hover:text-zinc-200" title={isOpen ? 'Collapse' : 'Expand'}>
                {isOpen ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
              </button>
            ) : <span className="w-[11px]" />}
            <span className={`h-1.5 w-1.5 flex-shrink-0 rounded-full ${status}`} title={agent.status} />
            <button onClick={() => onOpen(agent)} className="min-w-0 flex-1 truncate text-left hover:text-zinc-100" title={`${agent.path} · ${agent.status}`}>
              <span className="text-zinc-300">{agent.nickname || agent.role || agent.scope}</span>
              <span className="ml-1 text-[9px] uppercase tracking-wide text-zinc-600">{agent.kind}</span>
            </button>
            {badge && (
              <span className="hidden rounded border border-white/10 px-1 text-[8.5px] uppercase tracking-wide text-zinc-600 group-hover:inline">
                {badge}
              </span>
            )}
            {(agent.pendingMessages ?? 0) > 0 && (
              <span className="rounded-full bg-amber-400/15 px-1 text-[8.5px] text-amber-300" title="Pending messages">
                {agent.pendingMessages}
              </span>
            )}
            <button
              onClick={() => openDetail(agent)}
              className="hidden text-zinc-600 hover:text-zinc-200 group-hover:block"
              title="Agent details"
            >
              <MoreHorizontal size={11} />
            </button>
            {(agent.status === 'running' || agent.status === 'queued') && (
              <button
                onClick={() => cancelAgent(agent.id).then(() => void refreshAll())}
                className="hidden text-zinc-600 hover:text-red-300 group-hover:block"
                title="Cancel agent"
              >
                <X size={10} />
              </button>
            )}
          </div>
          <div className="pl-6 text-[9px] text-zinc-700">
            {agent.status} · {relativeTime(agent.lastActive || agent.updatedAt)}
            {agent.model ? ` · ${agent.model}` : ''}
          </div>
          {isOpen && kids.length > 0 && render(agent.id, depth + 1)}
        </div>
      );
    });
  };

  const selected = selectedId ? agents.find((agent) => agent.id === selectedId) ?? detail?.agent ?? null : null;
  const worktree = detail?.agent.worktree ?? selected?.worktree ?? null;
  const pending = detail?.messages ?? [];
  const history = detail?.history ?? [];

  return (
    <div className="mb-2" style={{ padding: '0 8px' }}>
      <button
        onClick={() => setOpen((value) => !value)}
        className="flex w-full items-center gap-1 px-1.5 py-1 text-[11px] font-bold uppercase tracking-widest text-zinc-500 hover:text-zinc-300"
      >
        <Bot size={12} />
        <span>Agents</span>
        <span className="ml-auto text-[9px] text-zinc-600">{agents.length}</span>
        <ChevronDown size={12} className={`transition-transform ${open ? '' : '-rotate-90'}`} />
      </button>
      {open && (
        <>
          <button
            type="button"
            onClick={() => { window.location.hash = workspaceId ? `#/memory?ws=${encodeURIComponent(workspaceId)}` : '#/memory'; }}
            className="mt-1 flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-[10px] text-zinc-500 hover:bg-white/[0.04] hover:text-zinc-300"
            title="Inspect workspace memory"
          >
            <Brain size={11} />
            <span>Workspace memory</span>
          </button>
          {agents.length === 0 ? (
            <p className="mt-1 px-1.5 text-[10px] text-zinc-700">No agents yet. Delegations appear here.</p>
          ) : (
            <div className="mt-0.5 flex flex-col gap-0.5">{render(null, 0)}</div>
          )}
        </>
      )}

      {selected && (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm" onClick={() => setSelectedId(null)}>
          <div
            className="flex max-h-[85vh] w-full max-w-xl flex-col overflow-hidden rounded-2xl border border-white/15 bg-[#0c0c10]"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex items-center gap-2 border-b border-white/10 px-4 py-3">
              <span className={`h-2 w-2 rounded-full ${STATUS_COLOR[selected.status] ?? 'bg-zinc-500'}`} />
              <div className="min-w-0">
                <h3 className="truncate text-[13px] font-semibold text-white">
                  {selected.nickname || selected.role || selected.scope}
                </h3>
                <p className="truncate text-[10px] text-zinc-500">
                  {selected.status} · {relativeTime(selected.lastActive || selected.updatedAt)}
                  {selected.model ? ` · ${selected.model}` : ''}
                </p>
              </div>
              <button type="button" onClick={() => setSelectedId(null)} className="ml-auto text-zinc-500 hover:text-zinc-200">
                <X size={15} />
              </button>
            </div>

            <div className="flex gap-1 border-b border-white/10 px-3 py-2">
              {(['overview', 'mailbox', 'worktree'] as DrawerTab[]).map((tab) => (
                <button
                  key={tab}
                  type="button"
                  onClick={() => setDetailTab(tab)}
                  className={`rounded-md px-2.5 py-1 text-[11px] capitalize ${
                    detailTab === tab ? 'bg-white/[0.09] text-white' : 'text-zinc-500 hover:text-zinc-200'
                  }`}
                >
                  {tab}
                  {tab === 'mailbox' && pending.length > 0 && (
                    <span className="ml-1 rounded-full bg-amber-400/20 px-1 text-[9px] text-amber-300">
                      {pending.length}
                    </span>
                  )}
                </button>
              ))}
              <button
                type="button"
                onClick={() => onOpen(selected)}
                className="ml-auto rounded-md border border-white/10 px-2 py-1 text-[11px] text-zinc-300 hover:bg-white/[0.06]"
              >
                Open chat
              </button>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto p-4">
              {actionError && (
                <p className="mb-3 rounded-lg border border-red-500/25 bg-red-500/10 px-3 py-2 text-[11px] text-red-300">
                  {actionError}
                </p>
              )}
              {actionNotice && (
                <p className="mb-3 rounded-lg border border-emerald-500/25 bg-emerald-500/10 px-3 py-2 text-[11px] text-emerald-300">
                  {actionNotice}
                </p>
              )}

              {detailTab === 'overview' && (
                <div className="flex flex-col gap-3 text-[12px] text-zinc-300">
                  <div className="grid grid-cols-2 gap-2">
                    <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                      <div className="text-[9px] uppercase tracking-widest text-zinc-600">Kind</div>
                      <div className="mt-0.5">{selected.kind} · depth {selected.depth}</div>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                      <div className="text-[9px] uppercase tracking-widest text-zinc-600">Access</div>
                      <div className="mt-0.5">{accessBadge(selected) ?? selected.access ?? 'direct'}</div>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                      <div className="text-[9px] uppercase tracking-widest text-zinc-600">Scope</div>
                      <div className="mt-0.5 truncate">{selected.scope}</div>
                    </div>
                    <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                      <div className="text-[9px] uppercase tracking-widest text-zinc-600">Workspace</div>
                      <div className="mt-0.5 truncate" title={selected.workspaceId}>{selected.workspaceId}</div>
                    </div>
                  </div>
                  <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                    <div className="text-[9px] uppercase tracking-widest text-zinc-600">Path</div>
                    <div className="mt-0.5 break-all font-mono text-[11px]">{selected.path}</div>
                  </div>
                  {selected.result && (
                    <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                      <div className="text-[9px] uppercase tracking-widest text-zinc-600">Result</div>
                      <pre className="mt-1 max-h-[220px] overflow-y-auto whitespace-pre-wrap break-words text-[11px] leading-relaxed">
                        {selected.result.slice(0, 6000)}
                      </pre>
                    </div>
                  )}
                  <div className="flex flex-wrap gap-2">
                    {(selected.status === 'running' || selected.status === 'queued') && (
                      <button
                        type="button"
                        disabled={detailBusy}
                        onClick={() => cancelAgent(selected.id).then(() => void refreshAll())}
                        className="rounded-lg border border-red-500/30 px-3 py-1.5 text-[11px] text-red-300 hover:bg-red-500/10 disabled:opacity-40"
                      >
                        Cancel
                      </button>
                    )}
                    <button
                      type="button"
                      disabled={detailBusy}
                      onClick={() => void refreshAll()}
                      className="rounded-lg border border-white/10 px-3 py-1.5 text-[11px] text-zinc-300 hover:bg-white/[0.06] disabled:opacity-40"
                    >
                      Refresh
                    </button>
                  </div>
                </div>
              )}

              {detailTab === 'mailbox' && (
                <div className="flex flex-col gap-3">
                  <div className="flex gap-2">
                    <input
                      value={messageDraft}
                      onChange={(event) => setMessageDraft(event.target.value)}
                      onKeyDown={(event) => { if (event.key === 'Enter') void sendMessage(); }}
                      placeholder={selected.status === 'running' ? 'Steer the running agent…' : 'Queue a message for the next run…'}
                      className="min-w-0 flex-1 rounded-lg border border-white/10 bg-black/30 px-3 py-2 text-[12px] text-zinc-200 outline-none focus:border-white/25"
                    />
                    <button
                      type="button"
                      disabled={detailBusy || !messageDraft.trim()}
                      onClick={() => void sendMessage()}
                      className="inline-flex items-center gap-1.5 rounded-lg bg-white px-3 py-2 text-[11px] font-semibold text-[#0a0a0d] disabled:opacity-40"
                    >
                      {detailBusy ? <Loader2 size={12} className="animate-spin" /> : <Send size={12} />}
                      Send
                    </button>
                  </div>
                  <div>
                    <h4 className="mb-1.5 flex items-center gap-1.5 text-[10px] font-bold uppercase tracking-widest text-zinc-500">
                      <Mail size={11} /> Pending ({pending.length})
                    </h4>
                    {pending.length === 0 ? (
                      <p className="text-[11px] text-zinc-600">No queued messages.</p>
                    ) : (
                      <div className="flex flex-col gap-1.5">
                        {pending.map((message) => (
                          <div key={message.id} className="rounded-lg border border-amber-400/20 bg-amber-400/[0.05] px-3 py-2">
                            <div className="text-[9px] uppercase tracking-wide text-amber-300/70">
                              {message.kind} · {message.delivery} · {relativeTime(message.createdAt)}
                            </div>
                            <p className="mt-0.5 whitespace-pre-wrap break-words text-[11.5px] text-zinc-200">{message.body}</p>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                  <div>
                    <h4 className="mb-1.5 text-[10px] font-bold uppercase tracking-widest text-zinc-500">
                      History ({history.length})
                    </h4>
                    {history.length === 0 ? (
                      <p className="text-[11px] text-zinc-600">No mailbox history.</p>
                    ) : (
                      <div className="flex max-h-[260px] flex-col gap-1.5 overflow-y-auto">
                        {history.slice(-30).reverse().map((message) => (
                          <div key={message.id} className="rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2">
                            <div className="text-[9px] uppercase tracking-wide text-zinc-600">
                              {message.kind} · {message.delivery}
                              {message.consumedAt ? ' · consumed' : ' · pending'} · {relativeTime(message.createdAt)}
                            </div>
                            <p className="mt-0.5 line-clamp-4 whitespace-pre-wrap break-words text-[11px] text-zinc-400">{message.body}</p>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                </div>
              )}

              {detailTab === 'worktree' && (
                <div className="flex flex-col gap-3 text-[12px] text-zinc-300">
                  {!worktree?.path ? (
                    <p className="text-[11.5px] text-zinc-500">
                      No isolated worktree. This agent edits the live workspace directly.
                    </p>
                  ) : (
                    <>
                      <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                        <div className="flex items-center gap-1.5 text-[11px] text-zinc-400">
                          <GitBranch size={12} /> {worktree.branch}
                          <span className="ml-auto rounded border border-white/10 px-1.5 py-0.5 text-[9px] uppercase tracking-wide text-zinc-500">
                            {worktree.state || 'active'}
                          </span>
                        </div>
                        <div className="mt-1 break-all font-mono text-[10.5px] text-zinc-500">{worktree.path}</div>
                        <div className="mt-0.5 font-mono text-[10px] text-zinc-600">base {worktree.base || 'unknown'}</div>
                        {worktree.error && (
                          <p className="mt-1.5 text-[11px] text-red-300">{worktree.error}</p>
                        )}
                      </div>
                      {worktree.diff?.stat && (
                        <pre className="max-h-[240px] overflow-y-auto whitespace-pre-wrap break-words rounded-lg border border-white/10 bg-black/30 p-3 font-mono text-[10.5px] leading-relaxed text-zinc-300">
                          {worktree.diff.stat}
                        </pre>
                      )}
                      {worktree.diff?.files && worktree.diff.files.length > 0 && (
                        <div className="rounded-lg border border-white/10 bg-white/[0.03] p-2.5">
                          <div className="text-[9px] uppercase tracking-widest text-zinc-600">Files</div>
                          <div className="mt-1 flex flex-col gap-0.5 font-mono text-[10.5px]">
                            {worktree.diff.files.slice(0, 40).map((file, index) => (
                              <div key={`${file.path}-${index}`} className="flex gap-2">
                                <span className="w-16 shrink-0 text-zinc-500">{file.status}</span>
                                <span className="min-w-0 truncate text-zinc-300">{file.path}</span>
                              </div>
                            ))}
                          </div>
                        </div>
                      )}
                      <div className="flex flex-wrap gap-2">
                        <button
                          type="button"
                          disabled={detailBusy || diffBusy}
                          onClick={async () => {
                            setDiffBusy(true);
                            setActionError(null);
                            try {
                              const result = await fetchAgentDiff(selected.id);
                              if (result.error) throw new Error(result.error);
                              await loadDetail(selected.id);
                            } catch (cause) {
                              setActionError(cause instanceof Error ? cause.message : String(cause));
                            } finally {
                              setDiffBusy(false);
                            }
                          }}
                          className="rounded-lg border border-white/10 px-3 py-1.5 text-[11px] text-zinc-300 hover:bg-white/[0.06] disabled:opacity-40"
                        >
                          {diffBusy ? 'Loading…' : 'Refresh diff'}
                        </button>
                        <button
                          type="button"
                          disabled={detailBusy}
                          onClick={() => setConfirmAction('merge')}
                          className="rounded-lg bg-white px-3 py-1.5 text-[11px] font-semibold text-[#0a0a0d] disabled:opacity-40"
                        >
                          Merge
                        </button>
                        <button
                          type="button"
                          disabled={detailBusy}
                          onClick={() => setConfirmAction('discard')}
                          className="rounded-lg border border-red-500/30 px-3 py-1.5 text-[11px] text-red-300 hover:bg-red-500/10 disabled:opacity-40"
                        >
                          Discard
                        </button>
                      </div>
                      {confirmAction && (
                        <div className="rounded-lg border border-amber-400/25 bg-amber-400/[0.06] p-3">
                          <p className="text-[11.5px] text-amber-200">
                            {confirmAction === 'merge'
                              ? 'Merge this worktree into the target tree? Review the diff first.'
                              : 'Discard this worktree? Unmerged changes are lost.'}
                          </p>
                          <div className="mt-2 flex justify-end gap-2">
                            <button
                              type="button"
                              onClick={() => setConfirmAction(null)}
                              className="rounded-lg border border-white/10 px-3 py-1.5 text-[11px] text-zinc-400 hover:bg-white/[0.05]"
                            >
                              Cancel
                            </button>
                            <button
                              type="button"
                              disabled={detailBusy}
                              onClick={() => void (confirmAction === 'merge' ? runMerge() : runDiscard())}
                              className={`rounded-lg px-3 py-1.5 text-[11px] font-semibold disabled:opacity-40 ${
                                confirmAction === 'merge'
                                  ? 'bg-white text-[#0a0a0d]'
                                  : 'bg-red-500 text-white'
                              }`}
                            >
                              {detailBusy ? 'Working…' : confirmAction === 'merge' ? 'Confirm merge' : 'Confirm discard'}
                            </button>
                          </div>
                        </div>
                      )}
                    </>
                  )}
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
