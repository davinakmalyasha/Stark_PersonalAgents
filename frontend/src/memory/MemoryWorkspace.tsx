import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  AlertTriangle,
  ArrowLeft,
  BookOpen,
  Brain,
  ChevronRight,
  Database,
  Eye,
  Loader2,
  Pin,
  PinOff,
  Plus,
  RefreshCw,
  RotateCcw,
  Save,
  Sparkles,
  Trash2,
  X,
} from 'lucide-react';
import WindowControls from '../components/Layout/WindowControls';
import {
  addMemoryNote,
  clearMemory,
  consolidateMemory,
  fetchMemory,
  fetchMemoryList,
  fetchMemoryTopic,
  forgetMemoryEntry,
  pinMemoryEntry,
  saveMemorySummary,
} from '../services/api';
import type {
  MemoryDetail,
  MemoryEntry,
  MemoryTopic,
  MemoryWorkspace as MemoryWorkspaceInfo,
} from '../services/api/memory';

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

function statusLabel(workspace: MemoryWorkspaceInfo): string {
  const entries = workspace.status.entryCount ?? 0;
  const pending = workspace.status.pendingCount ?? 0;
  if (pending > 0) return `${entries} entries · ${pending} pending`;
  return `${entries} entries`;
}

const KIND_OPTIONS = ['note', 'decision', 'fact', 'command', 'pitfall', 'preference', 'task'];
const TOPIC_OPTIONS = [
  'notes',
  'decisions',
  'facts',
  'commands',
  'pitfalls',
  'preferences',
  'tasks',
];

export default function MemoryWorkspace() {
  const initialWorkspace = useMemo(() => {
    const query = window.location.hash.split('?')[1] ?? '';
    return new URLSearchParams(query).get('ws') ?? '';
  }, []);
  const [workspaces, setWorkspaces] = useState<MemoryWorkspaceInfo[]>([]);
  const [activeId, setActiveId] = useState<string>(initialWorkspace);
  const [detail, setDetail] = useState<MemoryDetail | null>(null);
  const [summaryDraft, setSummaryDraft] = useState('');
  const [editingSummary, setEditingSummary] = useState(false);
  const [topicView, setTopicView] = useState<{ name: string; content: string } | null>(null);
  const [noteText, setNoteText] = useState('');
  const [noteKind, setNoteKind] = useState('note');
  const [noteTopic, setNoteTopic] = useState('notes');
  const [notePinned, setNotePinned] = useState(true);
  const [confirmClear, setConfirmClear] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const active = useMemo(
    () => workspaces.find((workspace) => workspace.id === activeId) ?? null,
    [workspaces, activeId],
  );

  const loadList = useCallback(async () => {
    try {
      const list = await fetchMemoryList();
      setWorkspaces(list.workspaces);
      setActiveId((previous) => {
        if (previous && list.workspaces.some((workspace) => workspace.id === previous)) {
          return previous;
        }
        if (initialWorkspace && list.workspaces.some((workspace) => workspace.id === initialWorkspace)) {
          return initialWorkspace;
        }
        return list.workspaces[0]?.id ?? '';
      });
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    }
  }, [initialWorkspace]);

  const loadDetail = useCallback(async (workspaceId: string) => {
    if (!workspaceId) {
      setDetail(null);
      return;
    }
    const next = await fetchMemory(workspaceId, 30);
    setDetail(next);
    setSummaryDraft(next.summary ?? '');
    setEditingSummary(false);
  }, []);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  useEffect(() => {
    if (!activeId) return;
    setError(null);
    void loadDetail(activeId).catch((cause) => {
      setError(cause instanceof Error ? cause.message : String(cause));
    });
  }, [activeId, loadDetail]);

  const refresh = useCallback(async () => {
    setBusy('refresh');
    setError(null);
    setNotice(null);
    try {
      await loadList();
      if (activeId) await loadDetail(activeId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }, [activeId, loadDetail, loadList]);

  const consolidate = useCallback(
    async (mode: 'model' | 'deterministic') => {
      if (!activeId) return;
      setBusy(mode);
      setError(null);
      setNotice(null);
      try {
        const result = await consolidateMemory(activeId, mode);
        if (result.error) throw new Error(result.error);
        setNotice(
          result.fallback
            ? 'Model consolidation failed; the deterministic fallback was used.'
            : `Consolidated ${result.entries ?? 0} durable entries (${result.source ?? mode}).`,
        );
        await loadDetail(activeId);
        await loadList();
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setBusy(null);
      }
    },
    [activeId, loadDetail, loadList],
  );

  const saveSummary = useCallback(async () => {
    if (!activeId) return;
    setBusy('summary');
    setError(null);
    try {
      await saveMemorySummary(activeId, summaryDraft);
      setEditingSummary(false);
      setNotice('Summary saved.');
      await loadDetail(activeId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }, [activeId, loadDetail, summaryDraft]);

  const addNote = useCallback(async () => {
    if (!activeId || !noteText.trim()) return;
    setBusy('note');
    setError(null);
    try {
      await addMemoryNote(activeId, noteText.trim(), noteKind, noteTopic, notePinned);
      setNoteText('');
      setNotice('Note added to durable memory.');
      await loadDetail(activeId);
      await loadList();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }, [activeId, loadDetail, loadList, noteKind, notePinned, noteText, noteTopic]);

  const forgetEntry = useCallback(
    async (entry: MemoryEntry) => {
      if (!activeId) return;
      setBusy(`forget-${entry.id}`);
      setError(null);
      try {
        await forgetMemoryEntry(activeId, entry.id);
        await loadDetail(activeId);
        await loadList();
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setBusy(null);
      }
    },
    [activeId, loadDetail, loadList],
  );

  const togglePin = useCallback(
    async (entry: MemoryEntry) => {
      if (!activeId) return;
      setBusy(`pin-${entry.id}`);
      setError(null);
      try {
        await pinMemoryEntry(activeId, entry.id, !entry.pinned);
        await loadDetail(activeId);
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setBusy(null);
      }
    },
    [activeId, loadDetail],
  );

  const openTopic = useCallback(
    async (topic: MemoryTopic) => {
      if (!activeId) return;
      setBusy(`topic-${topic.name}`);
      setError(null);
      try {
        const result = await fetchMemoryTopic(activeId, topic.name);
        setTopicView({ name: result.topic, content: result.content });
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        setBusy(null);
      }
    },
    [activeId],
  );

  const resetMemory = useCallback(async () => {
    if (!activeId) return;
    setBusy('clear');
    setError(null);
    try {
      await clearMemory(activeId);
      setConfirmClear(false);
      setNotice('Workspace memory cleared.');
      await loadDetail(activeId);
      await loadList();
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(null);
    }
  }, [activeId, loadDetail, loadList]);

  const entries = detail?.entries ?? [];
  const topics = detail?.topics ?? [];
  const records = detail?.records ?? [];

  return (
    <div className="fixed inset-0 flex flex-col overflow-hidden bg-[#09090b] text-zinc-200">
      <header className="flex h-[52px] shrink-0 items-center justify-between border-b border-white/10 px-4 sm:px-6">
        <div className="flex min-w-0 items-center gap-2.5">
          <button
            type="button"
            onClick={() => { window.location.hash = ''; }}
            title="Back to home"
            className="grid h-8 w-8 place-items-center rounded-lg text-zinc-400 transition-colors hover:bg-white/[0.07] hover:text-white"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="grid h-7 w-7 place-items-center rounded-lg bg-white text-[#0a0a0d]">
            <Brain size={14} />
          </div>
          <span className="truncate text-[15px] font-bold tracking-wide text-white">
            Workspace Memory
          </span>
          <span className="hidden rounded-full border border-white/10 px-2 py-0.5 text-[10px] uppercase tracking-widest text-zinc-500 sm:inline">
            index + topics
          </span>
        </div>
        <div className="flex items-center gap-2">
          <button
            type="button"
            onClick={refresh}
            disabled={!!busy}
            title="Refresh"
            className="grid h-8 w-8 place-items-center rounded-lg text-zinc-400 transition-colors hover:bg-white/[0.07] hover:text-white disabled:opacity-40"
          >
            <RefreshCw size={15} className={busy === 'refresh' ? 'animate-spin' : ''} />
          </button>
          <WindowControls />
        </div>
      </header>

      <div className="flex min-h-0 flex-1">
        <aside className="w-[230px] shrink-0 overflow-y-auto border-r border-white/10 p-3">
          <div className="mb-2 flex items-center gap-2 px-1 text-[10px] font-bold uppercase tracking-widest text-zinc-600">
            <Database size={12} /> Workspaces
          </div>
          <div className="flex flex-col gap-1">
            {workspaces.map((workspace) => (
              <button
                key={workspace.id}
                type="button"
                onClick={() => setActiveId(workspace.id)}
                className={`rounded-lg px-2.5 py-2 text-left transition-colors ${
                  workspace.id === activeId
                    ? 'bg-white/[0.09] text-white'
                    : 'text-zinc-400 hover:bg-white/[0.04] hover:text-zinc-200'
                }`}
              >
                <div className="flex items-center gap-2">
                  <span className="truncate text-[12.5px] font-medium">{workspace.name}</span>
                  {(workspace.status.pendingCount ?? 0) > 0 && (
                    <span className="ml-auto h-1.5 w-1.5 rounded-full bg-amber-400" />
                  )}
                </div>
                <div className="mt-0.5 truncate text-[10px] text-zinc-600">
                  {statusLabel(workspace)}
                </div>
              </button>
            ))}
            {workspaces.length === 0 && (
              <p className="px-1 text-[11px] text-zinc-600">No workspaces found.</p>
            )}
          </div>
        </aside>

        <main className="min-w-0 flex-1 overflow-y-auto px-4 py-5 sm:px-6">
          {error && (
            <div className="mb-4 flex items-start gap-2 rounded-xl border border-red-500/25 bg-red-500/10 px-3 py-2 text-[12px] text-red-300">
              <AlertTriangle size={14} className="mt-0.5 shrink-0" />
              <span className="whitespace-pre-wrap break-words">{error}</span>
              <button type="button" onClick={() => setError(null)} className="ml-auto text-red-300/60 hover:text-red-200">
                <X size={13} />
              </button>
            </div>
          )}
          {notice && (
            <div className="mb-4 flex items-center gap-2 rounded-xl border border-sky-500/25 bg-sky-500/10 px-3 py-2 text-[12px] text-sky-200">
              <Sparkles size={14} />
              <span>{notice}</span>
              <button type="button" onClick={() => setNotice(null)} className="ml-auto text-sky-200/60 hover:text-sky-100">
                <X size={13} />
              </button>
            </div>
          )}

          {!active && <p className="text-[13px] text-zinc-500">Select a workspace to inspect its memory.</p>}

          {active && (
            <div className="mx-auto flex max-w-5xl flex-col gap-4">
              <div className="flex flex-wrap items-end justify-between gap-3">
                <div className="min-w-0">
                  <h1 className="truncate text-[20px] font-bold tracking-tight text-white">
                    {active.name}
                  </h1>
                  <p className="mt-0.5 text-[11px] text-zinc-500">
                    {active.id} · {active.kind}
                    {active.path ? ` · ${active.path}` : ''}
                  </p>
                </div>
                <div className="flex flex-wrap gap-2">
                  <button
                    type="button"
                    disabled={!!busy}
                    onClick={() => void consolidate('model')}
                    className="inline-flex items-center gap-1.5 rounded-lg bg-white px-3 py-1.5 text-[11.5px] font-semibold text-[#0a0a0d] transition-opacity disabled:opacity-40"
                  >
                    {busy === 'model' ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />}
                    Consolidate
                  </button>
                  <button
                    type="button"
                    disabled={!!busy}
                    onClick={() => void consolidate('deterministic')}
                    className="inline-flex items-center gap-1.5 rounded-lg border border-white/10 px-3 py-1.5 text-[11.5px] text-zinc-300 transition-colors hover:bg-white/[0.06] disabled:opacity-40"
                  >
                    {busy === 'deterministic' ? <Loader2 size={13} className="animate-spin" /> : <RotateCcw size={13} />}
                    Rebuild local
                  </button>
                </div>
              </div>

              <section className="grid grid-cols-2 gap-2 sm:grid-cols-4">
                {[
                  ['Durable entries', detail?.status.entryCount ?? 0],
                  ['Pending turns', detail?.status.pendingCount ?? 0],
                  ['Topics', detail?.status.topicCount ?? 0],
                  ['Index chars', detail?.status.indexChars ?? 0],
                ].map(([label, value]) => (
                  <div key={String(label)} className="rounded-xl border border-white/10 bg-white/[0.03] px-3 py-2.5">
                    <div className="text-[10px] uppercase tracking-widest text-zinc-600">{label}</div>
                    <div className="mt-1 text-[18px] font-semibold text-zinc-100">{value}</div>
                  </div>
                ))}
              </section>

              <section className="rounded-xl border border-white/10 bg-white/[0.03]">
                <div className="flex items-center gap-2 border-b border-white/10 px-4 py-2.5">
                  <BookOpen size={13} className="text-zinc-500" />
                  <h2 className="text-[11px] font-bold uppercase tracking-widest text-zinc-400">
                    Injected index
                  </h2>
                  <span className="ml-auto text-[10px] text-zinc-600">
                    {detail?.status.updatedAt ? `updated ${formatTime(detail.status.updatedAt)}` : 'not consolidated yet'}
                  </span>
                </div>
                <pre className="max-h-[280px] overflow-y-auto whitespace-pre-wrap break-words px-4 py-3 font-mono text-[11.5px] leading-relaxed text-zinc-300">
                  {detail?.index?.trim() || 'No index yet. Consolidate the workspace to build MEMORY.md.'}
                </pre>
              </section>

              <section className="rounded-xl border border-white/10 bg-white/[0.03]">
                <div className="flex items-center gap-2 border-b border-white/10 px-4 py-2.5">
                  <Eye size={13} className="text-zinc-500" />
                  <h2 className="text-[11px] font-bold uppercase tracking-widest text-zinc-400">
                    Long-form summary
                  </h2>
                  <span className="ml-auto text-[10px] text-zinc-600">
                    {detail?.status.source ? `${detail.status.source}${detail.status.model ? ` · ${detail.status.model}` : ''}` : 'generated'}
                  </span>
                  {!editingSummary && (
                    <button
                      type="button"
                      onClick={() => setEditingSummary(true)}
                      className="rounded-md border border-white/10 px-2 py-1 text-[10.5px] text-zinc-300 hover:bg-white/[0.06]"
                    >
                      Edit
                    </button>
                  )}
                </div>
                {editingSummary ? (
                  <div className="p-3">
                    <textarea
                      value={summaryDraft}
                      onChange={(event) => setSummaryDraft(event.target.value)}
                      rows={10}
                      className="w-full resize-y rounded-lg border border-white/10 bg-black/30 px-3 py-2 font-mono text-[11.5px] leading-relaxed text-zinc-200 outline-none focus:border-white/25"
                      placeholder="## Objective&#10;..."
                    />
                    <div className="mt-2 flex justify-end gap-2">
                      <button
                        type="button"
                        onClick={() => { setEditingSummary(false); setSummaryDraft(detail?.summary ?? ''); }}
                        className="rounded-lg border border-white/10 px-3 py-1.5 text-[11px] text-zinc-400 hover:bg-white/[0.05]"
                      >
                        Cancel
                      </button>
                      <button
                        type="button"
                        disabled={busy === 'summary'}
                        onClick={() => void saveSummary()}
                        className="inline-flex items-center gap-1.5 rounded-lg bg-white px-3 py-1.5 text-[11px] font-semibold text-[#0a0a0d] disabled:opacity-40"
                      >
                        {busy === 'summary' ? <Loader2 size={12} className="animate-spin" /> : <Save size={12} />}
                        Save summary
                      </button>
                    </div>
                  </div>
                ) : (
                  <pre className="max-h-[320px] overflow-y-auto whitespace-pre-wrap break-words px-4 py-3 text-[12px] leading-relaxed text-zinc-300">
                    {detail?.summary?.trim() || 'No long-form summary yet.'}
                  </pre>
                )}
              </section>

              <section className="rounded-xl border border-white/10 bg-white/[0.03]">
                <div className="flex items-center gap-2 border-b border-white/10 px-4 py-2.5">
                  <Plus size={13} className="text-zinc-500" />
                  <h2 className="text-[11px] font-bold uppercase tracking-widest text-zinc-400">
                    Add durable note
                  </h2>
                </div>
                <div className="flex flex-col gap-2 p-3">
                  <textarea
                    value={noteText}
                    onChange={(event) => setNoteText(event.target.value)}
                    rows={2}
                    placeholder="Something the agent should remember across sessions…"
                    className="w-full resize-y rounded-lg border border-white/10 bg-black/30 px-3 py-2 text-[12px] text-zinc-200 outline-none focus:border-white/25"
                  />
                  <div className="flex flex-wrap items-center gap-2">
                    <select
                      value={noteKind}
                      onChange={(event) => setNoteKind(event.target.value)}
                      className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-[11px] text-zinc-300 outline-none"
                    >
                      {KIND_OPTIONS.map((kind) => <option key={kind} value={kind}>{kind}</option>)}
                    </select>
                    <select
                      value={noteTopic}
                      onChange={(event) => setNoteTopic(event.target.value)}
                      className="rounded-lg border border-white/10 bg-black/30 px-2 py-1.5 text-[11px] text-zinc-300 outline-none"
                    >
                      {TOPIC_OPTIONS.map((topic) => <option key={topic} value={topic}>{topic}</option>)}
                    </select>
                    <label className="inline-flex items-center gap-1.5 text-[11px] text-zinc-500">
                      <input
                        type="checkbox"
                        checked={notePinned}
                        onChange={(event) => setNotePinned(event.target.checked)}
                        className="accent-white"
                      />
                      pin
                    </label>
                    <button
                      type="button"
                      disabled={!!busy || !noteText.trim()}
                      onClick={() => void addNote()}
                      className="ml-auto inline-flex items-center gap-1.5 rounded-lg bg-white px-3 py-1.5 text-[11px] font-semibold text-[#0a0a0d] disabled:opacity-40"
                    >
                      {busy === 'note' ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
                      Add
                    </button>
                  </div>
                </div>
              </section>

              <section className="rounded-xl border border-white/10 bg-white/[0.03]">
                <div className="flex items-center gap-2 border-b border-white/10 px-4 py-2.5">
                  <Pin size={13} className="text-zinc-500" />
                  <h2 className="text-[11px] font-bold uppercase tracking-widest text-zinc-400">
                    Durable entries
                  </h2>
                  <span className="ml-auto text-[10px] text-zinc-600">{entries.length}</span>
                </div>
                <div className="divide-y divide-white/[0.06]">
                  {entries.map((entry) => (
                    <div key={entry.id} className="flex items-start gap-2 px-4 py-2.5">
                      <span className="mt-0.5 rounded border border-white/10 px-1.5 py-0.5 text-[9px] uppercase tracking-wide text-zinc-500">
                        {entry.kind}
                      </span>
                      <div className="min-w-0 flex-1">
                        <p className="text-[12px] leading-relaxed text-zinc-300">{entry.text}</p>
                        <p className="mt-0.5 text-[10px] text-zinc-600">
                          {entry.topic} · {entry.source}
                          {entry.model ? ` · ${entry.model}` : ''} · used {entry.usageCount}×
                        </p>
                      </div>
                      <button
                        type="button"
                        onClick={() => void togglePin(entry)}
                        disabled={!!busy}
                        title={entry.pinned ? 'Unpin' : 'Pin'}
                        className="mt-0.5 text-zinc-600 hover:text-zinc-200 disabled:opacity-40"
                      >
                        {entry.pinned ? <Pin size={13} /> : <PinOff size={13} />}
                      </button>
                      <button
                        type="button"
                        onClick={() => void forgetEntry(entry)}
                        disabled={!!busy}
                        title="Forget"
                        className="mt-0.5 text-zinc-600 hover:text-red-300 disabled:opacity-40"
                      >
                        <Trash2 size={13} />
                      </button>
                    </div>
                  ))}
                  {entries.length === 0 && (
                    <p className="px-4 py-4 text-[11.5px] text-zinc-600">
                      No durable entries yet. Consolidate or add a note.
                    </p>
                  )}
                </div>
              </section>

              <section className="grid gap-4 lg:grid-cols-2">
                <div className="rounded-xl border border-white/10 bg-white/[0.03]">
                  <div className="flex items-center gap-2 border-b border-white/10 px-4 py-2.5">
                    <BookOpen size={13} className="text-zinc-500" />
                    <h2 className="text-[11px] font-bold uppercase tracking-widest text-zinc-400">
                      Topic files
                    </h2>
                  </div>
                  <div className="divide-y divide-white/[0.06]">
                    {topics.map((topic) => (
                      <button
                        key={topic.name}
                        type="button"
                        onClick={() => void openTopic(topic)}
                        disabled={!!busy}
                        className="flex w-full items-center gap-2 px-4 py-2.5 text-left hover:bg-white/[0.04] disabled:opacity-50"
                      >
                        <span className="min-w-0 flex-1 truncate text-[12px] text-zinc-300">{topic.name}</span>
                        <span className="text-[10px] text-zinc-600">{topic.entries} entries</span>
                        {busy === `topic-${topic.name}` ? (
                          <Loader2 size={12} className="animate-spin text-zinc-500" />
                        ) : (
                          <ChevronRight size={12} className="text-zinc-600" />
                        )}
                      </button>
                    ))}
                    {topics.length === 0 && (
                      <p className="px-4 py-4 text-[11.5px] text-zinc-600">No topic files yet.</p>
                    )}
                  </div>
                </div>

                <div className="rounded-xl border border-white/10 bg-white/[0.03]">
                  <div className="flex items-center gap-2 border-b border-white/10 px-4 py-2.5">
                    <Database size={13} className="text-zinc-500" />
                    <h2 className="text-[11px] font-bold uppercase tracking-widest text-zinc-400">
                      Recent captured turns
                    </h2>
                    <span className="ml-auto text-[10px] text-zinc-600">{records.length}</span>
                  </div>
                  <div className="max-h-[320px] divide-y divide-white/[0.06] overflow-y-auto">
                    {records.map((record) => (
                      <div key={record.id} className="px-4 py-2.5">
                        <div className="flex items-center gap-2">
                          <span className="truncate text-[11.5px] text-zinc-300">{record.request}</span>
                          <span className={`ml-auto shrink-0 rounded px-1.5 py-0.5 text-[9px] uppercase tracking-wide ${
                            record.consolidatedAt
                              ? 'bg-emerald-500/10 text-emerald-300'
                              : 'bg-amber-500/10 text-amber-300'
                          }`}>
                            {record.consolidatedAt ? 'folded' : 'pending'}
                          </span>
                        </div>
                        <p className="mt-1 line-clamp-2 text-[10.5px] leading-relaxed text-zinc-600">
                          {record.summary}
                        </p>
                        <p className="mt-1 text-[9.5px] text-zinc-700">{formatTime(record.createdAt)}</p>
                      </div>
                    ))}
                    {records.length === 0 && (
                      <p className="px-4 py-4 text-[11.5px] text-zinc-600">No captured turns yet.</p>
                    )}
                  </div>
                </div>
              </section>

              <section className="flex flex-wrap items-center justify-between gap-3 rounded-xl border border-red-500/20 bg-red-500/[0.04] px-4 py-3">
                <div>
                  <h2 className="text-[11px] font-bold uppercase tracking-widest text-red-300/80">
                    Reset memory
                  </h2>
                  <p className="mt-0.5 text-[11px] text-zinc-500">
                    Deletes captured turns, durable entries, topic files, and the summary for this workspace.
                  </p>
                </div>
                <button
                  type="button"
                  onClick={() => setConfirmClear(true)}
                  disabled={!!busy}
                  className="inline-flex items-center gap-1.5 rounded-lg border border-red-500/30 px-3 py-1.5 text-[11px] text-red-300 hover:bg-red-500/10 disabled:opacity-40"
                >
                  <Trash2 size={12} /> Clear memory
                </button>
              </section>
            </div>
          )}
        </main>
      </div>

      {topicView && (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm" onClick={() => setTopicView(null)}>
          <div
            className="flex max-h-[80vh] w-full max-w-2xl flex-col overflow-hidden rounded-2xl border border-white/15 bg-[#0c0c10]"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex items-center gap-2 border-b border-white/10 px-4 py-3">
              <BookOpen size={14} className="text-zinc-500" />
              <h3 className="text-[12px] font-bold uppercase tracking-widest text-zinc-300">
                {topicView.name}
              </h3>
              <button type="button" onClick={() => setTopicView(null)} className="ml-auto text-zinc-500 hover:text-zinc-200">
                <X size={15} />
              </button>
            </div>
            <pre className="min-h-0 flex-1 overflow-y-auto whitespace-pre-wrap break-words px-4 py-3 font-mono text-[11.5px] leading-relaxed text-zinc-300">
              {topicView.content}
            </pre>
          </div>
        </div>
      )}

      {confirmClear && (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/70 p-4 backdrop-blur-sm" onClick={() => setConfirmClear(false)}>
          <div
            className="w-full max-w-sm rounded-2xl border border-white/15 bg-[#0c0c10] p-5"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="mb-3 grid h-9 w-9 place-items-center rounded-xl border border-red-500/25 bg-red-500/10">
              <AlertTriangle size={16} className="text-red-300" />
            </div>
            <h3 className="text-[15px] font-bold text-white">Clear memory?</h3>
            <p className="mt-1.5 text-[12px] leading-relaxed text-zinc-500">
              This removes all captured turns and durable memory for <span className="text-zinc-300">{active?.name}</span>.
              This cannot be undone.
            </p>
            <div className="mt-4 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setConfirmClear(false)}
                className="rounded-lg border border-white/10 px-3 py-1.5 text-[11px] text-zinc-400 hover:bg-white/[0.05]"
              >
                Cancel
              </button>
              <button
                type="button"
                disabled={busy === 'clear'}
                onClick={() => void resetMemory()}
                className="inline-flex items-center gap-1.5 rounded-lg bg-red-500 px-3 py-1.5 text-[11px] font-semibold text-white disabled:opacity-40"
              >
                {busy === 'clear' ? <Loader2 size={12} className="animate-spin" /> : <Trash2 size={12} />}
                Clear
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
