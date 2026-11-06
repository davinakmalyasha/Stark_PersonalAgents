import { useEffect, useRef, useState } from 'react';
import { ArrowLeft, ArrowRight, FolderOpen, Loader2, Plus, Sparkles, Users, X } from 'lucide-react';
import type { WorkspaceInfo } from '../types';

interface NewProjectDialogProps {
  onClose: () => void;
  /** Register the folder as a workspace → returns it (or null on failure). */
  onCreateProject: (folderPath: string, name: string, initialPrompt?: string) => Promise<WorkspaceInfo | null>;
  /** Create the group project that lives inside the new workspace. */
  onCreateGroup: (workspace: WorkspaceInfo, task: string) => Promise<void>;
  /** Preselected working mode; group projects tab opens in group mode. */
  defaultMode?: Mode;
}

type Mode = 'group' | 'solo';

/**
 * Two-step project flow:
 *  1. folder + name (the existing workspace setup)
 *  2. task / prompt + working mode
 * Nothing is created until step 2, so Back never leaves a stray workspace
 * behind. Group mode creates a durable team board; solo mode opens a normal
 * chat with the prompt ready in the dock.
 */
export default function NewProjectDialog({ onClose, onCreateProject, onCreateGroup, defaultMode = 'group' }: NewProjectDialogProps) {
  const [step, setStep] = useState<1 | 2>(1);
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [task, setTask] = useState('');
  const [mode, setMode] = useState<Mode>(defaultMode);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement | null>(null);
  const taskRef = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    window.setTimeout(() => nameRef.current?.focus(), 30);
  }, []);

  useEffect(() => {
    if (step === 2) window.setTimeout(() => taskRef.current?.focus(), 40);
  }, [step]);

  const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !busy) onClose(); };
  useEffect(() => {
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  });

  const browse = async () => {
    try {
      const picked = await window.antigravity?.selectFolder?.();
      if (picked) setPath(picked);
    } catch { /* picker unavailable (browser dev) — manual path input still works */ }
  };

  const next = () => {
    if (!path.trim()) {
      setError('Choose a folder first.');
      return;
    }
    setError(null);
    setStep(2);
  };

  const create = async () => {
    if (busy || !path.trim()) return;
    const prompt = task.trim();
    if (mode === 'group' && !prompt) {
      setError('Tell the team what to build.');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const ws = await onCreateProject(path.trim(), name.trim(), mode === 'solo' ? prompt : undefined);
      if (!ws) throw new Error('Could not create the project — check the folder path.');
      if (mode === 'group') await onCreateGroup(ws, prompt);
      onClose();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Could not create the project.');
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={() => { if (!busy) onClose(); }}>
      <div
        className="w-[390px] max-w-full rounded-2xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl overflow-hidden session-flow-enter"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between px-4 pt-3.5 pb-2">
          <div className="flex items-center gap-2">
            <span className="grid place-items-center w-6 h-6 rounded-lg border border-white/[0.08] bg-white/[0.035] text-zinc-400">
              {step === 1 ? <FolderOpen size={12} /> : <Sparkles size={12} />}
            </span>
            <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500">
              {step === 1 ? 'New project · Folder' : 'New project · Task'}
            </span>
          </div>
          <button onClick={onClose} disabled={busy} className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer disabled:opacity-40" aria-label="Close">
            <X size={13} />
          </button>
        </div>

        <div className="flex items-center gap-1.5 px-4 pb-3">
          <span className={`h-0.5 flex-1 rounded-full ${step >= 1 ? 'bg-white/55' : 'bg-white/10'}`} />
          <span className={`h-0.5 flex-1 rounded-full ${step >= 2 ? 'bg-white/55' : 'bg-white/10'}`} />
          <span className="ml-1 text-[9px] tabular-nums text-zinc-600">{step}/2</span>
        </div>

        {step === 1 ? (
          <div className="px-4 pb-4 flex flex-col gap-2.5">
            <input
              ref={nameRef}
              value={name}
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') next(); }}
              placeholder="Project name"
              className="w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 text-[12.5px] text-zinc-200 placeholder:text-zinc-600 outline-none focus:border-white/25"
            />
            <button
              onClick={browse}
              title="Choose a folder"
              className="flex items-center gap-2 w-full rounded-lg border border-white/10 bg-white/[0.03] px-3 py-2 text-left text-[12px] text-zinc-400 hover:text-zinc-200 hover:border-white/[0.25] transition-colors cursor-pointer"
            >
              <FolderOpen size={13} className="shrink-0" />
              <span className="truncate">{path || 'Choose folder…'}</span>
            </button>
            <input
              value={path}
              onChange={(e) => setPath(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Enter') next(); }}
              placeholder="Or paste a folder path"
              className="w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 text-[11.5px] font-mono text-zinc-400 placeholder:text-zinc-600 outline-none focus:border-white/25"
            />
            {error && <div className="text-[11px] text-red-400/90">{error}</div>}
            <div className="flex items-center justify-end gap-2 pt-1">
              <button
                onClick={onClose}
                className="rounded-lg px-3 py-1.5 text-[12px] text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.05] transition-colors cursor-pointer"
              >
                Cancel
              </button>
              <button
                onClick={next}
                disabled={!path.trim()}
                className="flex items-center gap-1.5 rounded-lg bg-white text-[#0a0a0d] px-3.5 py-1.5 text-[12px] font-semibold hover:bg-zinc-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer"
              >
                Next <ArrowRight size={12} />
              </button>
            </div>
          </div>
        ) : (
          <div className="px-4 pb-4 flex flex-col gap-2.5">
            <div className="rounded-lg border border-white/[0.07] bg-white/[0.025] px-3 py-2">
              <div className="truncate text-[12px] font-medium text-zinc-200">{name.trim() || 'Untitled project'}</div>
              <div className="truncate text-[9.5px] font-mono text-zinc-600">{path}</div>
            </div>

            <div className="grid grid-cols-2 gap-1.5" role="radiogroup" aria-label="Project mode">
              <button
                type="button"
                role="radio"
                aria-checked={mode === 'group'}
                onClick={() => { setMode('group'); setError(null); }}
                className={`flex items-start gap-2 rounded-lg border px-2.5 py-2 text-left transition-colors cursor-pointer ${
                  mode === 'group'
                    ? 'border-white/25 bg-white/[0.075]'
                    : 'border-white/[0.08] bg-white/[0.02] hover:border-white/15'
                }`}
              >
                <Users size={13} className="mt-0.5 shrink-0 text-zinc-400" />
                <span className="min-w-0">
                  <span className="block text-[11px] font-semibold text-zinc-200">Group project</span>
                  <span className="block text-[9px] leading-snug text-zinc-600">Team board, agents, approvals</span>
                </span>
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={mode === 'solo'}
                onClick={() => { setMode('solo'); setError(null); }}
                className={`flex items-start gap-2 rounded-lg border px-2.5 py-2 text-left transition-colors cursor-pointer ${
                  mode === 'solo'
                    ? 'border-white/25 bg-white/[0.075]'
                    : 'border-white/[0.08] bg-white/[0.02] hover:border-white/15'
                }`}
              >
                <Sparkles size={13} className="mt-0.5 shrink-0 text-zinc-400" />
                <span className="min-w-0">
                  <span className="block text-[11px] font-semibold text-zinc-200">Solo chat</span>
                  <span className="block text-[9px] leading-snug text-zinc-600">One chat, prompt ready</span>
                </span>
              </button>
            </div>

            <textarea
              ref={taskRef}
              value={task}
              onChange={(e) => setTask(e.target.value)}
              placeholder={mode === 'group' ? 'What should the team build?' : 'What should Stark work on? (optional)'}
              rows={5}
              className="w-full resize-none rounded-lg border border-white/10 bg-black/40 px-3 py-2.5 text-[12px] leading-relaxed text-zinc-200 placeholder:text-zinc-600 outline-none focus:border-white/25"
            />

            <div className="rounded-lg border border-white/[0.06] bg-white/[0.018] px-2.5 py-2 text-[9.5px] leading-relaxed text-zinc-600">
              {mode === 'group'
                ? 'The board is created in this folder. The team plans milestones, runs them, and pings you for approvals.'
                : 'Opens the workspace with a normal chat and this prompt already in the dock.'}
            </div>

            {error && <div className="text-[11px] text-red-400/90">{error}</div>}
            <div className="flex items-center justify-between gap-2 pt-1">
              <button
                onClick={() => { setStep(1); setError(null); }}
                disabled={busy}
                className="flex items-center gap-1 rounded-lg px-2.5 py-1.5 text-[11.5px] text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.05] transition-colors cursor-pointer disabled:opacity-40"
              >
                <ArrowLeft size={12} /> Back
              </button>
              <button
                onClick={() => { void create(); }}
                disabled={busy || (mode === 'group' && !task.trim())}
                className="flex items-center gap-1.5 rounded-lg bg-white text-[#0a0a0d] px-3.5 py-1.5 text-[12px] font-semibold hover:bg-zinc-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer"
              >
                {busy ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
                {mode === 'group' ? 'Create project' : 'Create chat'}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
