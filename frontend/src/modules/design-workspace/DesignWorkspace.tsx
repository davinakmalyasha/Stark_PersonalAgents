import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowLeft, ExternalLink, FileText, Image as ImageIcon, MousePointer2,
  Palette, Pencil, RefreshCw,
} from 'lucide-react';
import HouseChat from '../../house/HouseChat';
import SubagentSessionView from '../../house/SubagentSessionView';
import QuestionModal from '../../components/Workspace/QuestionModal';
import type { WorkspaceFileEntry } from '../../services/api';
import { useDesignSession } from './useDesignSession';

interface Mark {
  id: number;
  file: string;
  box: { x: number; y: number; w: number; h: number }; // fractions 0..1
  note: string;
}

const IMG_EXTS = new Set(['png', 'jpg', 'jpeg', 'webp', 'gif', 'svg']);

function extOf(path: string): string {
  return (path.split('.').pop() || '').toLowerCase();
}

function isHtml(f: WorkspaceFileEntry): boolean {
  return f.type === 'file' && extOf(f.path) === 'html';
}
function isDesignMd(f: WorkspaceFileEntry): boolean {
  return f.type === 'file' && f.name.toLowerCase() === 'design.md';
}
function isShot(f: WorkspaceFileEntry): boolean {
  return f.type === 'file' && IMG_EXTS.has(extOf(f.path));
}

export default function DesignWorkspace() {
  const session = useDesignSession();
  const {
    workspaces, workspace, chatId, messages, isRunning,
    notification, files, usage, booted, bindWorkspace, send, cancel,
  } = session;

  const [previewPath, setPreviewPath] = useState<string | null>(null);
  const [previewContent, setPreviewContent] = useState<string | null>(null);
  const [previewFailed, setPreviewFailed] = useState(false);
  const [subagentView, setSubagentView] = useState<{ childId: string; label: string } | null>(null);
  const [annotate, setAnnotate] = useState(false);
  const [draft, setDraft] = useState<{ x0: number; y0: number; x1: number; y1: number } | null>(null);
  const [note, setNote] = useState('');
  const [marks, setMarks] = useState<Mark[]>([]);
  const markId = useRef(1);
  const overlayRef = useRef<HTMLDivElement | null>(null);
  const threadRef = useRef<HTMLDivElement | null>(null);
  const knownHtml = useRef<Set<string>>(new Set());

  const htmlFiles = useMemo(() => files.filter(isHtml), [files]);
  const designMd = useMemo(() => files.find(isDesignMd) || null, [files]);
  const shots = useMemo(() => files.filter(isShot), [files]);

  // Default + auto-select newly appeared artifacts (e.g. right after a run).
  useEffect(() => {
    const paths = new Set(htmlFiles.map((f) => f.path));
    const fresh = htmlFiles.find((f) => !knownHtml.current.has(f.path));
    knownHtml.current = paths;
    if (fresh) {
      setPreviewPath(fresh.path);
    } else if (!previewPath && htmlFiles.length > 0) {
      setPreviewPath(htmlFiles[0].path);
    } else if (!previewPath && !designMd && files.length > 0) {
      const first = files.find((f) => f.type === 'file');
      if (first) setPreviewPath(first.path);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [files]);

  // Load preview content for the selected file.
  useEffect(() => {
    if (!previewPath) {
      setPreviewContent(null);
      return;
    }
    let cancelled = false;
    setPreviewFailed(false);
    setPreviewContent(null);
    session.fetchFile(previewPath)
      .then((res) => {
        if (cancelled) return;
        if (!res) {
          setPreviewFailed(true);
          return;
        }
        setPreviewContent(res.content);
      })
      .catch(() => {
        if (!cancelled) setPreviewFailed(true);
      });
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewPath, files]);

  const previewKind = previewPath
    ? (extOf(previewPath) === 'html' ? 'html' : extOf(previewPath) === 'md' ? 'md' : 'other')
    : null;

  const goHome = () => {
    window.location.hash = '#/';
  };
  const openInCoding = () => {
    if (!workspace) return;
    window.location.hash = `#/coding?ws=${encodeURIComponent(workspace.id)}&chat=${encodeURIComponent(chatId || '')}`;
  };

  const startBox = (e: React.PointerEvent) => {
    if (!annotate || e.button !== 0) return;
    const el = overlayRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const x = (e.clientX - r.left) / r.width;
    const y = (e.clientY - r.top) / r.height;
    (e.target as HTMLElement).setPointerCapture?.(e.pointerId);
    setDraft({ x0: x, y0: y, x1: x, y1: y });
  };
  const moveBox = (e: React.PointerEvent) => {
    if (!annotate || !draft) return;
    const el = overlayRef.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setDraft({ ...draft, x1: (e.clientX - r.left) / r.width, y1: (e.clientY - r.top) / r.height });
  };
  const endBox = () => {
    if (!annotate || !draft) return;
    const w = Math.abs(draft.x1 - draft.x0);
    const h = Math.abs(draft.y1 - draft.y0);
    if (w < 0.01 || h < 0.01) {
      setDraft(null);
      return;
    }
    setNote('');
  };
  const submitMark = () => {
    if (!draft || !note.trim() || !previewPath) return;
    const box = {
      x: Math.min(draft.x0, draft.x1),
      y: Math.min(draft.y0, draft.y1),
      w: Math.abs(draft.x1 - draft.x0),
      h: Math.abs(draft.y1 - draft.y0),
    };
    const pct = (v: number) => `${Math.round(v * 100)}%`;
    const id = markId.current++;
    setMarks((prev) => [...prev, { id, file: previewPath, box, note: note.trim() }]);
    setDraft(null);
    setNote('');
    setAnnotate(false);
    if (!isRunning) {
      void send(
        `Design note on ${previewPath} at region (x ${pct(box.x)}, y ${pct(box.y)}, w ${pct(box.w)}, h ${pct(box.h)}): ${note.trim()} — apply it to the artifact.`,
        [],
      );
    }
  };

  if (!booted) {
    return (
      <div className="fixed inset-0 grid place-items-center bg-[#0a0a0d] text-[12.5px] text-zinc-500">
        Opening design module…
      </div>
    );
  }

  // ── Unbound: pick a project workspace ──
  if (!workspace) {
    return (
      <div className="fixed inset-0 flex flex-col items-center justify-center gap-4 bg-[#0a0a0d] px-6 select-none">
        <Palette size={26} className="text-zinc-600" />
        <p className="text-[13px] text-zinc-300 font-medium">OpenDesign needs a project workspace</p>
        <p className="text-[11.5px] text-zinc-600 max-w-sm text-center">
          Design sessions write real files. Pick the project the UI work belongs to.
        </p>
        <div className="flex flex-col gap-1.5 w-full max-w-sm max-h-[40vh] overflow-y-auto">
          {workspaces.map((w) => (
            <button
              key={w.id}
              type="button"
              onClick={() => void bindWorkspace(w)}
              className="text-left px-3 py-2 rounded-lg border house-hairline bg-white/[0.03] hover:bg-white/[0.07] cursor-pointer"
            >
              <span className="block truncate text-[12.5px] text-zinc-200">{w.name}</span>
              <span className="block truncate text-[10.5px] text-zinc-600 font-mono">{w.path}</span>
            </button>
          ))}
          {workspaces.length === 0 && (
            <p className="text-[11.5px] text-zinc-600 text-center">No workspaces yet — add one from the Coding workspace first.</p>
          )}
        </div>
        <button
          type="button"
          onClick={goHome}
          className="mt-1 flex items-center gap-1.5 text-[11.5px] text-zinc-500 hover:text-zinc-200 cursor-pointer"
        >
          <ArrowLeft size={12} /> Back home
        </button>
      </div>
    );
  }

  const hasArtifact = htmlFiles.length > 0;
  const hasShot = shots.length > 0;
  const hasDesignMd = !!designMd;

  return (
    <div className="fixed inset-0 flex min-h-0 bg-[#0a0a0d] text-zinc-200 select-none">
      {/* ── Left rail: versions + brand ── */}
      <aside className="w-[236px] shrink-0 border-r house-hairline bg-[#0d0d11]/70 flex flex-col min-h-0">
        <div className="flex items-center gap-1.5 px-2.5 pt-2.5 pb-2 shrink-0">
          <button onClick={goHome} title="Back home" className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer">
            <ArrowLeft size={13} />
          </button>
          <Palette size={13} className="text-zinc-400 shrink-0" />
          <span className="flex-1 min-w-0 truncate text-[12px] font-semibold text-zinc-200" title={workspace.path}>
            {workspace.name}
          </span>
          <button
            onClick={() => session.refreshFiles()}
            title="Refresh files"
            className="grid place-items-center w-6 h-6 rounded-md text-zinc-600 hover:text-white hover:bg-white/[0.07] cursor-pointer"
          >
            <RefreshCw size={12} />
          </button>
        </div>

        {/* Status chips */}
        <div className="flex items-center gap-1.5 px-2.5 pb-2 shrink-0">
          {[
            { label: 'Artifact', ok: hasArtifact },
            { label: 'Shot', ok: hasShot },
            { label: 'DESIGN.md', ok: hasDesignMd },
          ].map((c) => (
            <span
              key={c.label}
              title={c.ok ? `${c.label} present` : `${c.label} missing`}
              className={`px-1.5 py-0.5 rounded text-[9.5px] font-bold uppercase tracking-wider border ${
                c.ok ? 'text-emerald-400/90 border-emerald-400/20 bg-emerald-400/[0.06]'
                  : 'text-zinc-600 border-white/[0.06] bg-white/[0.02]'
              }`}
            >
              {c.label}
            </span>
          ))}
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto house-thread px-1.5 pb-2">
          <p className="px-2 pt-1 pb-1 text-[9.5px] font-bold tracking-widest uppercase text-zinc-600">Artifacts</p>
          {htmlFiles.length === 0 && (
            <p className="px-2 text-[11px] text-zinc-600 leading-5">No HTML yet — describe what to design in the chat.</p>
          )}
          {htmlFiles.map((f) => (
            <button
              key={f.path}
              type="button"
              onClick={() => setPreviewPath(f.path)}
              className={`w-full flex items-center gap-1.5 px-2 py-[5px] text-left text-[12px] rounded-md cursor-pointer ${
                previewPath === f.path ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:bg-white/[0.05] hover:text-zinc-300'
              }`}
            >
              <FileText size={12} className="shrink-0 text-zinc-600" />
              <span className="truncate flex-1 font-mono">{f.name}</span>
            </button>
          ))}
          <p className="px-2 pt-2 pb-1 text-[9.5px] font-bold tracking-widest uppercase text-zinc-600">Brand</p>
          {designMd ? (
            <button
              key={designMd.path}
              type="button"
              onClick={() => setPreviewPath(designMd.path)}
              className={`w-full flex items-center gap-1.5 px-2 py-[5px] text-left text-[12px] rounded-md cursor-pointer ${
                previewPath === designMd.path ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:bg-white/[0.05] hover:text-zinc-300'
              }`}
            >
              <FileText size={12} className="shrink-0 text-zinc-600" />
              <span className="truncate flex-1 font-mono">DESIGN.md</span>
            </button>
          ) : (
            <p className="px-2 text-[11px] text-zinc-600 leading-5">No DESIGN.md yet — it lands here at finalize.</p>
          )}
          <p className="px-2 pt-2 pb-1 text-[9.5px] font-bold tracking-widest uppercase text-zinc-600">Shots</p>
          {shots.length === 0 && (
            <p className="px-2 text-[11px] text-zinc-600 leading-5">No screenshots yet.</p>
          )}
          {shots.map((f) => (
            <button
              key={f.path}
              type="button"
              onClick={() => setPreviewPath(f.path)}
              className={`w-full flex items-center gap-1.5 px-2 py-[5px] text-left text-[12px] rounded-md cursor-pointer ${
                previewPath === f.path ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:bg-white/[0.05] hover:text-zinc-300'
              }`}
            >
              <ImageIcon size={12} className="shrink-0 text-zinc-600" />
              <span className="truncate flex-1 font-mono">{f.name}</span>
            </button>
          ))}
        </div>
      </aside>

      {/* ── Center: live preview ── */}
      <section className="flex-1 min-w-0 flex flex-col min-h-0 bg-[#09090b]">
        <div className="flex items-center gap-1.5 h-[38px] px-2.5 border-b house-hairline bg-[#0b0b0e]/70 shrink-0">
          <MousePointer2 size={12} className="text-zinc-600 shrink-0" />
          <span className="truncate text-[11.5px] font-mono text-zinc-400" title={previewPath || ''}>
            {previewPath || 'No file selected'}
          </span>
          {isRunning && <span className="session-run-dot ml-1" title="Agent working" />}
          <span className="flex-1" />
          <button
            type="button"
            onClick={() => { setAnnotate((v) => !v); setDraft(null); }}
            title={annotate ? 'Stop annotating' : 'Annotate on the preview: drag a box, add a note, send to the agent'}
            className={`flex items-center gap-1 px-2 h-[26px] rounded-md text-[11px] cursor-pointer transition-colors ${
              annotate ? 'bg-white/[0.12] text-white' : 'text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.05]'
            }`}
          >
            <Pencil size={11} /> Annotate
          </button>
          <button
            type="button"
            onClick={() => session.refreshFiles()}
            title="Refresh preview"
            className="grid place-items-center w-7 h-7 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer"
          >
            <RefreshCw size={13} />
          </button>
          <button
            type="button"
            onClick={openInCoding}
            title="Open this chat in the full Coding Workspace"
            className="grid place-items-center w-7 h-7 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer"
          >
            <ExternalLink size={13} />
          </button>
        </div>

        <div className="relative flex-1 min-h-0 overflow-auto house-thread">
          {!previewPath ? (
            <div className="h-full flex flex-col items-center justify-center gap-2.5 text-center px-8">
              <Palette size={26} className="text-zinc-700" />
              <p className="text-[12.5px] text-zinc-500">No preview yet</p>
              <p className="text-[11px] text-zinc-700 max-w-xs">Ask the agent to design something from the chat panel.</p>
            </div>
          ) : previewFailed ? (
            <div className="p-4 text-[12px] text-red-400/80">Failed to read file.</div>
          ) : previewContent === null ? (
            <div className="p-4 text-[12px] text-zinc-600 font-mono">Loading {previewPath}…</div>
          ) : previewKind === 'html' ? (
            <iframe
              key={previewPath}
              title={`Design preview: ${previewPath}`}
              srcDoc={previewContent}
              sandbox="allow-scripts"
              className="absolute inset-0 w-full h-full bg-white"
            />
          ) : previewKind === 'md' ? (
            <pre className="p-4 text-[12px] leading-5 text-zinc-300 whitespace-pre-wrap break-words font-mono">{previewContent}</pre>
          ) : (
            <div className="p-4 text-[12px] text-zinc-500">
              Binary preview isn't supported in this view yet — open the file from the Coding workspace.
            </div>
          )}

          {/* Annotate overlay (above the preview, captures drags only in annotate mode) */}
          <div
            ref={overlayRef}
            onPointerDown={startBox}
            onPointerMove={moveBox}
            onPointerUp={endBox}
            className={`absolute inset-0 z-10 ${annotate ? 'cursor-crosshair touch-none' : 'pointer-events-none'}`}
          >
            {marks.filter((m) => m.file === previewPath).map((m, i) => (
              <div
                key={m.id}
                title={m.note}
                className="absolute border-2 border-amber-400/90 bg-amber-400/10 rounded-[2px] pointer-events-none"
                style={{ left: `${m.box.x * 100}%`, top: `${m.box.y * 100}%`, width: `${m.box.w * 100}%`, height: `${m.box.h * 100}%` }}
              >
                <span className="absolute -top-4 left-0 text-[9px] font-bold text-amber-300 bg-black/70 px-1 rounded">#{i + 1}</span>
              </div>
            ))}
            {annotate && draft && (
              <div
                className="absolute border-2 border-dashed border-sky-400/90 bg-sky-400/10 rounded-[2px] pointer-events-none"
                style={{
                  left: `${Math.min(draft.x0, draft.x1) * 100}%`,
                  top: `${Math.min(draft.y0, draft.y1) * 100}%`,
                  width: `${Math.abs(draft.x1 - draft.x0) * 100}%`,
                  height: `${Math.abs(draft.y1 - draft.y0) * 100}%`,
                }}
              />
            )}
          </div>

          {/* Note composer for the drafted box */}
          {annotate && draft && Math.abs(draft.x1 - draft.x0) >= 0.01 && Math.abs(draft.y1 - draft.y0) >= 0.01 && (
            <div className="absolute z-20 left-1/2 -translate-x-1/2 bottom-4 w-[min(420px,90%)] rounded-xl border house-hairline bg-[#101014]/95 backdrop-blur p-2.5 shadow-2xl">
              <input
                // eslint-disable-next-line jsx-a11y/no-autofocus
                autoFocus
                value={note}
                onChange={(e) => setNote(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === 'Enter') submitMark();
                  if (e.key === 'Escape') { setDraft(null); setNote(''); }
                }}
                placeholder="What should change in this region?"
                className="w-full bg-transparent text-[12px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none px-1 py-0.5"
              />
              <div className="flex justify-end gap-1.5 mt-1.5">
                <button
                  type="button"
                  onClick={() => { setDraft(null); setNote(''); }}
                  className="px-2.5 h-[26px] rounded-md text-[11px] text-zinc-500 hover:text-zinc-200 cursor-pointer"
                >
                  Cancel
                </button>
                <button
                  type="button"
                  onClick={submitMark}
                  disabled={!note.trim() || isRunning}
                  className="px-2.5 h-[26px] rounded-md text-[11px] bg-white/[0.10] text-white disabled:opacity-40 cursor-pointer"
                >
                  Send to agent
                </button>
              </div>
            </div>
          )}
        </div>

        <div className="flex items-center gap-3 h-[24px] px-3 border-t house-hairline bg-[#0b0b0e]/70 text-[10.5px] text-zinc-600 shrink-0">
          <span className="truncate font-mono">{previewPath || '—'}</span>
          <span className="flex-1" />
          <span>live preview · read-only</span>
        </div>
      </section>

      {/* ── Right: agent chat ── */}
      <aside className="w-[380px] shrink-0 border-l house-hairline bg-[#0f0f13] flex flex-col min-h-0 overflow-hidden">
        <HouseChat
          variant="ide-panel"
          title="OpenDesign"
          subtitle={workspace.name}
          messages={messages}
          isRunning={isRunning}
          isLoading={false}
          notification={notification}
          threadRef={threadRef}
          isVoiceMode={false}
          totalCost={usage?.total_cost || 0}
          totalInput={usage?.total_input_tokens || 0}
          totalOutput={usage?.total_output_tokens || 0}
          totalCacheHitTokens={usage?.total_cache_hit_tokens || 0}
          activeContextTokens={0}
          onSend={(prompt, files_) => { if (!isRunning) void send(prompt, files_); }}
          onCancel={cancel}
          onToggleVoice={() => {}}
          onOpenStats={() => {}}
          onOpenInWorkspace={openInCoding}
          onCloseIde={goHome}
          onOpenSubagent={(childId, label) => setSubagentView({ childId, label })}
          workspaceId={workspace.id}
          chatId={chatId}
        />
      </aside>

      {/* Direction-lock + plan-review questions surface here */}
      <QuestionModal visible={isRunning} />


      {/* Subagent drill-in */}
      {subagentView && workspace && (
        <div className="absolute inset-0 z-40 bg-[#0c0c0f] flex flex-col px-4 sm:px-8 lg:px-12 py-4">
          <div className="max-w-[1100px] w-full mx-auto flex-1 min-h-0 flex flex-col">
            <SubagentSessionView
              workspaceId={workspace.id}
              childId={subagentView.childId}
              label={subagentView.label}
              parentTitle="OpenDesign"
              onBack={() => setSubagentView(null)}
            />
          </div>
        </div>
      )}
    </div>
  );
}
