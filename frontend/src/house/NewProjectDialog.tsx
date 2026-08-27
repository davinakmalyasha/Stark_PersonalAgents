import { useEffect, useRef, useState } from 'react';
import { FolderOpen, Loader2, Plus, X } from 'lucide-react';
import type { WorkspaceInfo } from '../types';

interface NewProjectDialogProps {
  onClose: () => void;
  /** Register the folder as a workspace → returns it (or null on failure). */
  onCreateProject: (folderPath: string, name: string) => Promise<WorkspaceInfo | null>;
}

/**
 * "New project" dialog for the sidebar's Coding filter: name + native folder
 * picker (with manual path fallback). On success the caller creates the
 * project's first chat and opens it as a session tab. The parent mounts it
 * fresh for every open (conditional render), so inputs always start empty.
 */
export default function NewProjectDialog({ onClose, onCreateProject }: NewProjectDialogProps) {
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    window.setTimeout(() => nameRef.current?.focus(), 30);
  }, []);

  const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose(); };
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

  const create = async () => {
    if (busy || !path.trim()) return;
    setBusy(true);
    setError(null);
    const ws = await onCreateProject(path.trim(), name.trim());
    setBusy(false);
    if (ws) onClose();
    else setError('Could not create the project — check the folder path.');
  };

  return (
    <div className="fixed inset-0 z-[70] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={onClose}>
      <div
        className="w-[340px] rounded-2xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl overflow-hidden session-flow-enter"
        onClick={(e) => e.stopPropagation()}
      >        <div className="flex items-center justify-between px-4 pt-3.5 pb-2">
          <span className="text-[10px] uppercase tracking-widest font-semibold text-zinc-500">New project</span>
          <button onClick={onClose} className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.08] transition-colors cursor-pointer" aria-label="Close">
            <X size={13} />
          </button>
        </div>
        <div className="px-4 pb-4 flex flex-col gap-2.5">
          <input
            ref={nameRef}
            value={name}
            onChange={(e) => setName(e.target.value)}
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
            placeholder="Or paste a folder path"
            className="w-full rounded-lg border border-white/10 bg-black/40 px-3 py-2 text-[11.5px] font-mono text-zinc-400 placeholder:text-zinc-600 outline-none focus:border-white/25"
          />
          {error && <div className="text-[11px] text-red-400/90">{error}</div>}
          <div className="flex items-center justify-end gap-2 pt-1">
            <button
              onClick={onClose}
              className="rounded-lg px-3 py-1.5 text-[12px] text-zinc-500 hover:text-zinc-200 hover:bg-white/[0.05] transition-colors cursor-pointer"
            >
              Back
            </button>
            <button
              onClick={create}
              disabled={busy || !path.trim()}
              className="flex items-center gap-1.5 rounded-lg bg-white text-[#0a0a0d] px-3.5 py-1.5 text-[12px] font-semibold hover:bg-zinc-200 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer"
            >
              {busy ? <Loader2 size={12} className="animate-spin" /> : <Plus size={12} />}
              Create
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
