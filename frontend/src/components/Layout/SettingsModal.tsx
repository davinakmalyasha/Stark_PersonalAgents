import { useCallback, useEffect, useState } from 'react';
import { Download, FolderOpen, HardDrive, RefreshCw, Settings as SettingsIcon, Upload, X } from 'lucide-react';
import {
  exportStarkData,
  fetchDesktopHarness,
  fetchHarness,
  fetchMigrationStatus,
  importMigration,
  importStarkData,
  pickFolder,
  setDesktopHarnessRoot,
  type HarnessInfo,
  type MigrationCandidate,
} from '../../services/api';

interface Props {
  isOpen: boolean;
  onClose: () => void;
}

export default function SettingsModal({ isOpen, onClose }: Props) {
  const [harness, setHarness] = useState<HarnessInfo | null>(null);
  const [desktop, setDesktop] = useState<{ harness_root: string; install_path: string; default_root: string; dev: boolean; error?: string | null } | null>(null);
  const [candidates, setCandidates] = useState<MigrationCandidate[]>([]);
  const [active, setActive] = useState('');
  const [status, setStatus] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [backupBusy, setBackupBusy] = useState(false);
  const [backupRoot, setBackupRoot] = useState('');
  const [backupFile, setBackupFile] = useState<File | null>(null);

  const load = useCallback(async () => {
    setError('');
    try {
      const [server, migration] = await Promise.all([
        fetchHarness().catch(() => null),
        fetchMigrationStatus().catch(() => null),
      ]);
      if (server) setHarness(server);
      if (migration) {
        setCandidates(migration.candidates || []);
        setActive(migration.active || '');
      }
      try {
        setDesktop(await fetchDesktopHarness());
      } catch {
        setDesktop(null);
      }
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : 'Could not load settings.');
    }
  }, []);

  useEffect(() => {
    if (isOpen) void load();
  }, [isOpen, load]);

  const changeRoot = async () => {
    setBusy(true);
    setError('');
    setStatus('');
    try {
      const folder = await pickFolder();
      if (!folder) return;
      await setDesktopHarnessRoot(folder);
      setStatus('Harness root saved. Restart the app to use it.');
      await load();
    } catch (changeError) {
      setError(changeError instanceof Error ? changeError.message : 'Could not save harness root.');
    } finally {
      setBusy(false);
    }
  };

  const runImport = async (source: string) => {
    setBusy(true);
    setError('');
    setStatus('');
    try {
      const result = await importMigration(source);
      setStatus(`${result.message} Copied: ${result.copied.join(', ')}.`);
      await load();
    } catch (importError) {
      setError(importError instanceof Error ? importError.message : 'Migration failed.');
    } finally {
      setBusy(false);
    }
  };

  const importFromFolder = async () => {
    setBusy(true);
    setError('');
    setStatus('');
    try {
      const folder = await pickFolder();
      if (!folder) return;
      const result = await importMigration(folder);
      setStatus(`${result.message} Copied: ${result.copied.join(', ')}.`);
      await load();
    } catch (importError) {
      setError(importError instanceof Error ? importError.message : 'Migration failed.');
    } finally {
      setBusy(false);
    }
  };

  const runExport = async () => {
    setBackupBusy(true);
    setError('');
    setStatus('');
    try {
      const name = await exportStarkData();
      setStatus(`Exported ${name}. Keep it safe — it contains chats and config, never API keys.`);
    } catch (exportError) {
      setError(exportError instanceof Error ? exportError.message : 'Export failed.');
    } finally {
      setBackupBusy(false);
    }
  };

  const runBackupImport = async () => {
    if (!backupFile) return;
    setBackupBusy(true);
    setError('');
    setStatus('');
    try {
      const result = await importStarkData(backupFile, backupRoot);
      setStatus(
        `Backup staged (${result.from_root || 'unknown'} → ${result.to_root}). Restart the app/server to apply it. A pre-import copy was kept.`,
      );
      setBackupFile(null);
    } catch (importError) {
      setError(importError instanceof Error ? importError.message : 'Import failed.');
    } finally {
      setBackupBusy(false);
    }
  };

  if (!isOpen) return null;

  const rowClass = 'flex items-start justify-between gap-3 rounded-lg border border-white/[0.07] bg-white/[0.02] px-3 py-2';
  const labelClass = 'text-[10px] font-semibold uppercase tracking-[0.14em] text-zinc-500';
  const valueClass = 'text-[12px] text-zinc-200 font-mono break-all';

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 backdrop-blur-md p-4 animate-fade-in">
      <div
        className="modal-panel-enter border bg-zinc-950 flex flex-col overflow-hidden"
        style={{
          borderColor: 'var(--border-color)',
          borderRadius: '16px',
          width: 'min(720px, calc(100vw - 48px))',
          maxHeight: 'min(640px, calc(100vh - 48px))',
          boxShadow: '0 25px 50px -12px rgba(0, 0, 0, 0.5)',
        }}
      >
        <div
          className="flex items-center justify-between flex-shrink-0"
          style={{ padding: '18px 22px', borderBottom: '1px solid rgba(255,255,255,0.08)' }}
        >
          <div className="flex items-center gap-3">
            <div className="p-2 rounded-lg text-zinc-300 border" style={{ backgroundColor: 'rgba(255,255,255,0.04)', borderColor: 'rgba(255,255,255,0.09)' }}>
              <SettingsIcon size={18} />
            </div>
            <div>
              <h2 className="text-sm font-semibold text-zinc-100 tracking-tight">Settings</h2>
              <p className="text-[11px] text-zinc-500 mt-0.5">App, harness data, and one-time migration</p>
            </div>
          </div>
          <button onClick={onClose} className="p-1.5 text-zinc-400 hover:text-zinc-200 hover:bg-white/5 rounded-md transition-all" aria-label="Close">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto space-y-4" style={{ padding: '18px 22px' }}>
          <section className="space-y-2">
            <div className={labelClass}>Install & data paths</div>
            <div className={rowClass}>
              <div className="min-w-0">
                <div className={labelClass}>App install</div>
                <div className={valueClass}>{desktop?.install_path || harness?.install_path || '—'}</div>
              </div>
            </div>
            <div className={rowClass}>
              <div className="min-w-0 flex-1">
                <div className={labelClass}>Harness root (chats, sessions, config, .env)</div>
                <div className={valueClass}>{desktop?.harness_root || harness?.harness_root || '—'}</div>
                {desktop?.error && <div className="text-[11px] text-red-300 mt-1">{desktop.error}</div>}
                {harness?.error && <div className="text-[11px] text-red-300 mt-1">{harness.error}</div>}
                <div className="text-[10.5px] text-zinc-500 mt-1">
                  Default: {desktop?.default_root || String.raw`D:\StarkData`}. Your projects stay where they are;
                  only harness data lives here.
                </div>
              </div>
              <button
                type="button"
                onClick={() => void changeRoot()}
                disabled={busy}
                className="flex-shrink-0 inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-white/10 text-[11.5px] text-zinc-300 hover:bg-white/[0.05] disabled:opacity-50"
              >
                <FolderOpen size={14} /> Change…
              </button>
            </div>
            {!!harness?.workspaces?.length && (
              <div className={rowClass}>
                <div className="min-w-0">
                  <div className={labelClass}>Workspaces ({harness.workspaces.length})</div>
                  <div className="mt-1 space-y-1">
                    {harness.workspaces.slice(0, 8).map((workspace) => (
                      <div key={workspace.id} className="text-[11px] text-zinc-400 font-mono truncate">
                        {workspace.name} — {workspace.path}
                      </div>
                    ))}
                    {harness.workspaces.length > 8 && (
                      <div className="text-[10.5px] text-zinc-500">+{harness.workspaces.length - 8} more</div>
                    )}
                  </div>
                </div>
              </div>
            )}
          </section>

          <section className="space-y-2">
            <div className="flex items-center gap-2">
              <Download size={13} className="text-zinc-500" />
              <span className={labelClass}>Backup &amp; move</span>
            </div>
            <p className="text-[11px] text-zinc-500">
              One portable zip: chats, sessions, config, memory, workspaces (relative paths) and a
              consistent database snapshot. API keys, voice models, logs and WAL files are excluded.
            </p>
            <div className={rowClass}>
              <div className="min-w-0">
                <div className="text-[12px] text-zinc-200 font-medium">Export this machine</div>
                <div className="text-[10.5px] text-zinc-500 mt-0.5">
                  Downloads a timestamped zip you can import on another machine.
                </div>
              </div>
              <button
                type="button"
                onClick={() => void runExport()}
                disabled={backupBusy || busy}
                className="flex-shrink-0 inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg bg-indigo-500/90 text-[11.5px] font-semibold text-white hover:bg-indigo-400 disabled:opacity-50"
              >
                <Download size={14} /> Export
              </button>
            </div>
            <div className={rowClass}>
              <div className="min-w-0 flex-1 space-y-1.5">
                <div className="text-[12px] text-zinc-200 font-medium">Import a backup</div>
                <div className="text-[10.5px] text-zinc-500">
                  Applied on the next start, after backing up the current data. Workspace paths are
                  rebased onto the projects root below.
                </div>
                <input
                  type="file"
                  accept=".zip,application/zip"
                  onChange={(event) => setBackupFile(event.target.files?.[0] ?? null)}
                  className="block w-full text-[11px] text-zinc-400 file:mr-2 file:rounded-md file:border-0 file:bg-white/10 file:px-2 file:py-1 file:text-[11px] file:text-zinc-200"
                />
                <input
                  value={backupRoot}
                  onChange={(event) => setBackupRoot(event.target.value)}
                  placeholder={`Projects root here (default: ${desktop?.harness_root ? 'STARK_PROJECTS_ROOT' : 'D:\\\\MyProject'})`}
                  className="block w-full rounded-md border border-white/10 bg-black/30 px-2 py-1 text-[11px] text-zinc-200 placeholder:text-zinc-600 focus:border-indigo-500/60 focus:outline-none"
                />
              </div>
              <button
                type="button"
                onClick={() => void runBackupImport()}
                disabled={backupBusy || busy || !backupFile}
                className="flex-shrink-0 inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-white/10 text-[11.5px] text-zinc-300 hover:bg-white/[0.05] disabled:opacity-50"
              >
                <Upload size={14} /> Import
              </button>
            </div>
          </section>

          <section className="space-y-2">
            <div className="flex items-center gap-2">
              <HardDrive size={13} className="text-zinc-500" />
              <span className={labelClass}>One-time migration</span>
              <button
                type="button"
                onClick={() => void load()}
                disabled={busy}
                className="ml-auto p-1 text-zinc-500 hover:text-zinc-200"
                title="Refresh"
              >
                <RefreshCw size={13} />
              </button>
            </div>
            <p className="text-[11px] text-zinc-500">
              Import chats, sessions, and config from an old harness root into the active one
              ({active || '—'}). Existing files are backed up to <span className="font-mono">.bak</span> first;
              nothing is merged automatically.
            </p>
            {candidates.filter((candidate) => !candidate.active).map((candidate) => (
              <div key={candidate.id} className={rowClass}>
                <div className="min-w-0">
                  <div className="text-[12px] text-zinc-200 font-medium">{candidate.id}</div>
                  <div className="text-[11px] text-zinc-500 font-mono break-all">{candidate.path}</div>
                  <div className="text-[10.5px] text-zinc-500 mt-0.5">
                    {[
                      candidate.has_chats_db ? 'chats.db' : null,
                      candidate.has_sessions ? 'sessions' : null,
                      candidate.has_config ? 'config' : null,
                    ].filter(Boolean).join(' · ') || 'empty'}
                  </div>
                </div>
                <button
                  type="button"
                  onClick={() => void runImport(candidate.id)}
                  disabled={busy || (!candidate.has_chats_db && !candidate.has_sessions && !candidate.has_config)}
                  className="flex-shrink-0 px-3 py-1.5 rounded-lg bg-indigo-500/90 text-[11.5px] font-semibold text-white hover:bg-indigo-400 disabled:opacity-50"
                >
                  Import
                </button>
              </div>
            ))}
            {!candidates.filter((candidate) => !candidate.active).length && (
              <div className="text-[11px] text-zinc-500">No other harness roots with data were found.</div>
            )}
            <button
              type="button"
              onClick={() => void importFromFolder()}
              disabled={busy}
              className="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-white/10 text-[11.5px] text-zinc-300 hover:bg-white/[0.05] disabled:opacity-50"
            >
              <FolderOpen size={14} /> Import from a folder…
            </button>
          </section>

          {error && <div className="text-[11px] text-red-300">{error}</div>}
          {status && <div className="text-[11px] text-emerald-300/90">{status}</div>}
        </div>
      </div>
    </div>
  );
}
