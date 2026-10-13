import { useEffect, useMemo, useRef, useState } from 'react';
import {
  ChevronDown, ChevronRight, Code2, ExternalLink, FileText, Folder, FolderOpen,
  PanelLeftClose, PanelLeftOpen, PanelRightClose, PanelRightOpen, RefreshCw, Search,
  TerminalSquare, Wrench, Lightbulb, Info, GitCompare, Plus,
} from 'lucide-react';
import { Prism as SyntaxHighlighter } from 'react-syntax-highlighter';
import oneDark from 'react-syntax-highlighter/dist/esm/styles/prism/one-dark';
import type { ArtifactFile, RunArtifacts, SessionTab } from '../types';
import { fetchWorkspaceFile, fetchWorkspaceFiles } from '../services/api';
import type { WorkspaceFileEntry } from '../services/api';
import { buildTree, type TreeNode } from '../utils/fileTree';
import { parseLogToSteps, type TraceStep } from '../utils/logParser';
import type { useSessionRunner } from './useSessionRunner';
import HouseChat from './HouseChat';
import SubagentSessionView from './SubagentSessionView';

// ── Helpers ──────────────────────────────────────────────────────────────────

function langOf(path: string): string {
  const ext = path.split('.').pop()?.toLowerCase() ?? '';
  const map: Record<string, string> = {
    ts: 'typescript', tsx: 'tsx', js: 'javascript', jsx: 'jsx', mjs: 'javascript', cjs: 'javascript',
    py: 'python', json: 'json', css: 'css', scss: 'scss', html: 'html', md: 'markdown',
    yml: 'yaml', yaml: 'yaml', sh: 'bash', bat: 'batch', ps1: 'powershell', sql: 'sql',
    rs: 'rust', go: 'go', java: 'java', c: 'c', cpp: 'cpp', h: 'c', cs: 'csharp',
    toml: 'toml', ini: 'ini', env: 'ini', txt: 'text', xml: 'xml', svg: 'xml',
  };
  return map[ext] ?? 'text';
}

function stepsOf(messages: { role: string; metadata?: Record<string, unknown> }[]): TraceStep[] {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const m = messages[i];
    if (m.role === 'agent' && m.metadata?.isTrace && Array.isArray(m.metadata.steps)) {
      return m.metadata.steps as TraceStep[];
    }
  }
  return [];
}

function artifactsOf(messages: { role: string; metadata?: Record<string, unknown> }[]): RunArtifacts | null {
  for (let i = messages.length - 1; i >= 0; i -= 1) {
    const m = messages[i];
    if (m.role === 'agent' && m.metadata?.isTrace && m.metadata.artifacts) {
      return m.metadata.artifacts as RunArtifacts;
    }
  }
  return null;
}

// ── File rail (left) ─────────────────────────────────────────────────────────

interface FileRailProps {
  session: SessionTab;
  files: WorkspaceFileEntry[];
  loading: boolean;
  activeFile: string | null;
  onOpenFile: (path: string) => void;
  onRefresh: () => void;
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

function FileRail({ session, files, loading, activeFile, onOpenFile, onRefresh, collapsed, onToggleCollapsed }: FileRailProps) {
  const [filter, setFilter] = useState('');
  const [openDirs, setOpenDirs] = useState<Set<string>>(new Set());

  const tree = useMemo(() => buildTree(files), [files]);
  const filteredTree = useMemo(() => {
    if (!filter.trim()) return tree;
    const q = filter.toLowerCase();
    const walk = (nodes: TreeNode[]): TreeNode[] => nodes
      .map((n) => ({ ...n, children: walk(n.children) }))
      .filter((n) => n.name.toLowerCase().includes(q) || n.children.length > 0);
    return walk(tree);
  }, [tree, filter]);

  const toggleDir = (path: string) => {
    setOpenDirs((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path); else next.add(path);
      return next;
    });
  };

  const renderNode = (node: TreeNode, depth: number): React.ReactNode => {
    const pad = { paddingLeft: 8 + depth * 14 };
    if (node.type === 'dir') {
      const open = openDirs.has(node.path);
      return (
        <div key={node.path}>
          <button
            type="button"
            onClick={() => toggleDir(node.path)}
            className="w-full flex items-center gap-1.5 px-1.5 py-[3px] text-left text-[12px] text-zinc-300 hover:bg-white/[0.05] rounded-md cursor-pointer"
            style={pad}
          >
            {open ? <ChevronDown size={11} className="text-zinc-600 shrink-0" /> : <ChevronRight size={11} className="text-zinc-600 shrink-0" />}
            {open ? <FolderOpen size={12} className="text-zinc-400 shrink-0" /> : <Folder size={12} className="text-zinc-400 shrink-0" />}
            <span className="truncate">{node.name}</span>
          </button>
          {open && node.children.map((c) => renderNode(c, depth + 1))}
        </div>
      );
    }
    const isSel = activeFile === node.path;
    return (
      <button
        key={node.path}
        type="button"
        onClick={() => onOpenFile(node.path)}
        className={`w-full flex items-center gap-1.5 px-1.5 py-[3px] text-left text-[12px] rounded-md cursor-pointer ${
          isSel ? 'bg-white/[0.10] text-white' : 'text-zinc-500 hover:bg-white/[0.05] hover:text-zinc-300'
        }`}
        style={pad}
      >
        <FileText size={12} className={`${isSel ? 'text-zinc-300' : 'text-zinc-600'} shrink-0`} />
        <span className="truncate flex-1">{node.name}</span>
      </button>
    );
  };

  if (collapsed) {
    return (
      <div className="w-10 shrink-0 border-r house-hairline bg-[#0d0d11]/70 flex flex-col items-center py-2 gap-1.5">
        <button onClick={onToggleCollapsed} title="Show explorer" className="grid place-items-center w-7 h-7 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer">
          <PanelRightOpen size={14} className="rotate-180" />
        </button>
        <button onClick={onRefresh} title="Refresh files" className="grid place-items-center w-7 h-7 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer">
          <RefreshCw size={13} />
        </button>
      </div>
    );
  }

  return (
    <aside className="w-[236px] shrink-0 border-r house-hairline bg-[#0d0d11]/70 backdrop-blur-sm flex flex-col min-h-0">
      <div className="flex items-center gap-1.5 px-2.5 pt-2.5 pb-2 shrink-0">
        <Code2 size={13} className="text-zinc-400 shrink-0" />
        <span className="flex-1 min-w-0 truncate text-[12px] font-semibold text-zinc-200" title={session.workspacePath || session.workspaceName}>
          {session.workspaceName}
        </span>
        <button onClick={onRefresh} title="Refresh files" className="grid place-items-center w-6 h-6 rounded-md text-zinc-600 hover:text-white hover:bg-white/[0.07] cursor-pointer">
          <RefreshCw size={12} />
        </button>
        <button
          onClick={() => { window.location.hash = `#/coding?ws=${encodeURIComponent(session.workspaceId)}&chat=${encodeURIComponent(session.chatId)}`; }}
          title="Open this chat in the full Coding Workspace"
          className="grid place-items-center w-6 h-6 rounded-md text-zinc-600 hover:text-white hover:bg-white/[0.07] cursor-pointer"
        >
          <ExternalLink size={12} />
        </button>
        <button onClick={onToggleCollapsed} title="Hide explorer" className="grid place-items-center w-6 h-6 rounded-md text-zinc-600 hover:text-white hover:bg-white/[0.07] cursor-pointer">
          <PanelLeftClose size={13} />
        </button>
      </div>
      <div className="flex items-center gap-1.5 mx-2 mb-1.5 px-2 py-1 rounded-md bg-white/[0.04] border border-white/[0.06] shrink-0">
        <Search size={11} className="text-zinc-600 shrink-0" />
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="Filter files…"
          className="flex-1 min-w-0 bg-transparent text-[11.5px] text-zinc-300 placeholder:text-zinc-600 focus:outline-none"
        />
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto house-thread px-1.5 pb-2">
        {loading ? (
          <div className="text-[11.5px] text-zinc-600 px-2 py-3">Loading project…</div>
        ) : filteredTree.length === 0 ? (
          <div className="text-[11.5px] text-zinc-600 px-2 py-3">
            {files.length === 0 ? 'No files yet — ask the agent to build something.' : 'No matches.'}
          </div>
        ) : (
          filteredTree.map((n) => renderNode(n, 0))
        )}
      </div>
    </aside>
  );
}

// ── Editor pane (center) ─────────────────────────────────────────────────────

interface EditorPaneProps {
  openFiles: string[];
  activeFile: string | null;
  content: { text: string; truncated: boolean } | null;
  loading: boolean;
  onActivateFile: (path: string) => void;
  onCloseFile: (path: string) => void;
}

function EditorPane({ openFiles, activeFile, content, loading, onActivateFile, onCloseFile }: EditorPaneProps) {
  const highlighted = useMemo(() => {
    if (!activeFile || !content) return null;
    return (
      <div className="min-h-full">
        <SyntaxHighlighter
          language={langOf(activeFile)}
          style={oneDark}
          showLineNumbers
          customStyle={{
            margin: 0,
            padding: '14px 0',
            background: 'transparent',
            fontSize: '12.5px',
            lineHeight: '1.6',
            minHeight: '100%',
          }}
          codeTagProps={{ style: { fontFamily: "'JetBrains Mono', ui-monospace, monospace" } }}
        >
          {content.text}
        </SyntaxHighlighter>
        {content.truncated && (
          <div className="px-4 pb-3 text-[11px] text-amber-400/80">File truncated at 200 KB — preview only.</div>
        )}
      </div>
    );
  }, [activeFile, content]);

  return (
    <section className="flex-1 min-w-0 flex flex-col min-h-0">
      {/* File tabs */}
      <div className="flex items-stretch h-[34px] border-b house-hairline bg-[#0b0b0e]/70 shrink-0 overflow-x-auto no-scrollbar">
        {openFiles.length === 0 && (
          <div className="flex items-center px-3 text-[11px] text-zinc-600 select-none">No files open</div>
        )}
        {openFiles.map((path) => {
          const name = path.split('/').pop() ?? path;
          const isActive = path === activeFile;
          return (
            <div
              key={path}
              onClick={() => onActivateFile(path)}
              title={path}
              className={`group flex items-center gap-1.5 pl-3 pr-1.5 border-r house-hairline cursor-pointer select-none shrink-0 max-w-[190px] transition-colors ${
                isActive ? 'bg-[#111116] text-zinc-100' : 'text-zinc-500 hover:text-zinc-300 hover:bg-white/[0.03]'
              }`}
            >
              <FileText size={11} className={isActive ? 'text-zinc-400' : 'text-zinc-600'} />
              <span className="truncate text-[11.5px] font-mono">{name}</span>
              <button
                onClick={(e) => { e.stopPropagation(); onCloseFile(path); }}
                title="Close file"
                aria-label={`Close ${name}`}
                className="grid place-items-center w-[17px] h-[17px] rounded text-zinc-600 hover:text-white hover:bg-white/[0.10] opacity-0 group-hover:opacity-100 transition-opacity"
              >
                ×
              </button>
            </div>
          );
        })}
      </div>

      {/* Code viewer */}
      <div className="flex-1 min-h-0 overflow-auto house-thread bg-[#09090b]">
        {!activeFile ? (
          <div className="h-full flex flex-col items-center justify-center gap-2.5 text-center select-none px-8">
            <Code2 size={26} className="text-zinc-700" />
            <p className="text-[12.5px] text-zinc-500">Select a file from the explorer</p>
            <p className="text-[11px] text-zinc-700 max-w-xs">Browse the project on the left, or ask the agent to build something from the chat panel.</p>
          </div>
        ) : loading ? (
          <div className="p-4 text-[12px] text-zinc-600 font-mono">Loading {activeFile}…</div>
        ) : content ? highlighted : (
          <div className="p-4 text-[12px] text-red-400/80">Failed to read file.</div>
        )}
      </div>

      {/* Status bar */}
      <div className="flex items-center gap-3 h-[24px] px-3 border-t house-hairline bg-[#0b0b0e]/70 text-[10.5px] text-zinc-600 shrink-0 select-none">
        <span className="truncate font-mono">{activeFile ?? '—'}</span>
        <span className="flex-1" />
        {content && <span>{content.text.split('\n').length} lines</span>}
        <span className="uppercase tracking-wider">{activeFile ? langOf(activeFile) : ''}</span>
        <span>read-only</span>
      </div>
    </section>
  );
}

// ── Activity panel (right) ───────────────────────────────────────────────────

type ActivityTab = 'activity' | 'diff' | 'terminal';

interface ActivityPanelProps {
  steps: TraceStep[];
  artifacts: RunArtifacts | null;
  terminalLog: string;
  isRunning: boolean;
  collapsed: boolean;
  onToggleCollapsed: () => void;
}

function StepIcon({ step }: { step: TraceStep }) {
  if (step.type === 'thought') return <Lightbulb size={11} className="text-zinc-500 shrink-0 mt-[3px]" />;
  if (step.type === 'command' || step.toolName === 'run_command') return <TerminalSquare size={11} className="text-zinc-500 shrink-0 mt-[3px]" />;
  if (step.type === 'tool') return <Wrench size={11} className="text-zinc-500 shrink-0 mt-[3px]" />;
  return <Info size={11} className="text-zinc-500 shrink-0 mt-[3px]" />;
}

function ActivityPanel({ steps, artifacts, terminalLog, isRunning, collapsed, onToggleCollapsed }: ActivityPanelProps) {
  const [tab, setTab] = useState<ActivityTab>('activity');
  const [openStep, setOpenStep] = useState<string | null>(null);
  const [openDiff, setOpenDiff] = useState<string | null>(null);

  if (collapsed) {
    return (
      <div className="w-10 shrink-0 border-l house-hairline bg-[#0d0d11]/70 flex flex-col items-center py-2 gap-1.5">
        <button onClick={onToggleCollapsed} title="Show agent activity" className="grid place-items-center w-7 h-7 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] cursor-pointer">
          <PanelLeftOpen size={14} className="rotate-180" />
        </button>
      </div>
    );
  }

  const files: ArtifactFile[] = artifacts?.files ?? [];

  return (
    <aside className="w-[300px] shrink-0 border-l house-hairline bg-[#0d0d11]/70 backdrop-blur-sm flex flex-col min-h-0">
      <div className="flex items-center gap-0.5 px-2 pt-2 pb-1.5 border-b house-hairline shrink-0">
        {(['activity', 'diff', 'terminal'] as ActivityTab[]).map((t) => (
          <button
            key={t}
            onClick={() => setTab(t)}
            className={`px-2.5 h-[26px] rounded-md text-[11px] capitalize transition-colors cursor-pointer ${
              tab === t ? 'bg-white/[0.09] text-white' : 'text-zinc-500 hover:text-zinc-300 hover:bg-white/[0.04]'
            }`}
          >
            {t}
          </button>
        ))}
        <span className="flex-1" />
        {isRunning && <span className="session-run-dot mr-1" title="Agent working" />}
        <button onClick={onToggleCollapsed} title="Hide panel" className="grid place-items-center w-6 h-6 rounded-md text-zinc-600 hover:text-white hover:bg-white/[0.07] cursor-pointer">
          <PanelRightClose size={13} />
        </button>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto house-thread px-2 py-2">
        {tab === 'activity' && (
          steps.length === 0 ? (
            <div className="text-[11.5px] text-zinc-600 px-2 py-3 leading-5">
              {isRunning ? 'Agent is starting…' : 'No activity yet. Send a prompt and watch the agent work here.'}
            </div>
          ) : (
            <div className="flex flex-col gap-0.5">
              {steps.map((s) => {
                const open = openStep === s.id;
                return (
                  <div key={s.id} className="rounded-md">
                    <button
                      onClick={() => setOpenStep(open ? null : s.id)}
                      className="w-full flex items-start gap-1.5 px-1.5 py-1 text-left hover:bg-white/[0.04] rounded-md cursor-pointer"
                    >
                      <StepIcon step={s} />
                      <span className="flex-1 min-w-0">
                        <span className="block truncate text-[11.5px] text-zinc-300 leading-4">{s.title}</span>
                        <span className="block text-[9.5px] uppercase tracking-wider text-zinc-600 mt-0.5">{s.agent}{s.duration ? ` · ${s.duration}` : ''}</span>
                      </span>
                      {s.isError && <span className="text-[9px] text-red-400 shrink-0 mt-1">ERR</span>}
                    </button>
                    {open && s.content && (
                      <pre className="mx-1.5 mb-1 px-2.5 py-2 rounded-md bg-black/40 border border-white/[0.06] text-[10.5px] leading-4 text-zinc-400 whitespace-pre-wrap break-words max-h-[240px] overflow-y-auto house-thread font-mono">
                        {s.content}
                      </pre>
                    )}
                  </div>
                );
              })}
            </div>
          )
        )}

        {tab === 'diff' && (
          files.length === 0 ? (
            <div className="text-[11.5px] text-zinc-600 px-2 py-3 leading-5">No file changes recorded yet.</div>
          ) : (
            <div className="flex flex-col gap-1">
              {artifacts && (artifacts.total_additions !== undefined || artifacts.total_deletions !== undefined) && (
                <div className="flex items-center gap-2 px-1.5 py-1 text-[10.5px] text-zinc-500">
                  <GitCompare size={11} />
                  <span>{files.length} file{files.length > 1 ? 's' : ''} touched</span>
                  <span className="text-emerald-400/90">+{artifacts.total_additions ?? 0}</span>
                  <span className="text-red-400/90">−{artifacts.total_deletions ?? 0}</span>
                </div>
              )}
              {files.map((f) => {
                const open = openDiff === f.path;
                return (
                  <div key={f.path} className="rounded-md border border-white/[0.05] bg-white/[0.02] overflow-hidden">
                    <button
                      onClick={() => setOpenDiff(open ? null : f.path)}
                      className="w-full flex items-center gap-1.5 px-2 py-1.5 text-left hover:bg-white/[0.04] cursor-pointer"
                    >
                      <FileText size={11} className={`shrink-0 ${f.status === 'added' ? 'text-emerald-400/80' : f.status === 'deleted' ? 'text-red-400/80' : 'text-zinc-500'}`} />
                      <span className="flex-1 min-w-0 truncate text-[11px] font-mono text-zinc-300">{f.path}</span>
                      <span className="text-[10px] text-emerald-400/90 tabular-nums">+{f.additions}</span>
                      <span className="text-[10px] text-red-400/90 tabular-nums">−{f.deletions}</span>
                    </button>
                    {open && f.diff && (
                      <pre className="px-2.5 py-2 border-t border-white/[0.05] bg-black/40 text-[10px] leading-4 font-mono whitespace-pre-wrap break-words max-h-[280px] overflow-y-auto house-thread">
                        {f.diff}
                      </pre>
                    )}
                  </div>
                );
              })}
            </div>
          )
        )}

        {tab === 'terminal' && (
          terminalLog.trim() ? (
            <pre className="px-1 text-[10.5px] leading-[1.5] text-zinc-400 font-mono whitespace-pre-wrap break-words">
              {terminalLog}
            </pre>
          ) : (
            <div className="text-[11.5px] text-zinc-600 px-2 py-3 leading-5">Terminal output streams here while the agent works.</div>
          )
        )}
      </div>
    </aside>
  );
}

// ── The mini-IDE background ──────────────────────────────────────────────────

type SessionRunner = ReturnType<typeof useSessionRunner>;

interface CodingIDEProps {
  session: SessionTab;
  /** The session runner lives in House so runs survive tab switches. */
  runner: SessionRunner;
  /** Any pipeline running anywhere — gates sending from this tab. */
  isGlobalRunning: boolean;
  /** Close the IDE — back to the chat card view (tab stays). */
  onCloseIde?: () => void;
}

export default function CodingIDE({ session, runner, isGlobalRunning, onCloseIde }: CodingIDEProps) {
  const [files, setFiles] = useState<WorkspaceFileEntry[]>([]);
  const [filesLoading, setFilesLoading] = useState(true);
  const [openFiles, setOpenFiles] = useState<string[]>([]);
  const [activeFile, setActiveFile] = useState<string | null>(null);
  const [contentCache, setContentCache] = useState<Record<string, { text: string; truncated: boolean } | null>>({});
  const [leftCollapsed, setLeftCollapsed] = useState(false);
  const [rightCollapsed, setRightCollapsed] = useState(false);
  const [subagentView, setSubagentView] = useState<{ childId: string; label: string; workspaceId?: string } | null>(null);
  const threadRef = useRef<HTMLDivElement | null>(null);
  const prevRunning = useRef(false);

  // ── Load the project tree (again after each finished run) ──
  const refreshFiles = useMemo(() => () => {
    fetchWorkspaceFiles(session.workspaceId)
      .then((f) => setFiles(f))
      .catch(() => setFiles([]))
      .finally(() => setFilesLoading(false));
  }, [session.workspaceId]);

  useEffect(() => { refreshFiles(); }, [refreshFiles]);

  useEffect(() => {
    if (prevRunning.current && !runner.isRunning) refreshFiles();
    prevRunning.current = runner.isRunning;
  }, [runner.isRunning, refreshFiles]);

  // ── Open + load files ──
  const openFile = (path: string) => {
    setActiveFile(path);
    setOpenFiles((prev) => (prev.includes(path) ? prev : [...prev, path]));
  };

  useEffect(() => {
    if (!activeFile || activeFile in contentCache) return;
    let cancelled = false;
    fetchWorkspaceFile(session.workspaceId, activeFile)
      .then((res) => {
        if (cancelled) return;
        setContentCache((prev) => ({ ...prev, [activeFile]: res ? { text: res.content, truncated: res.truncated } : null }));
      })
      .catch(() => {
        if (!cancelled) setContentCache((prev) => ({ ...prev, [activeFile]: null }));
      });
    return () => { cancelled = true; };
  }, [activeFile, contentCache, session.workspaceId]);

  // Loading = tab open but its content hasn't landed in the cache yet.
  const contentLoading = !!activeFile && !(activeFile in contentCache);

  const closeFile = (path: string) => {
    setOpenFiles((prev) => {
      const next = prev.filter((p) => p !== path);
      setActiveFile((cur) => {
        if (cur !== path) return cur;
        const idx = prev.indexOf(path);
        return next[Math.min(idx, next.length - 1)] ?? null;
      });
      return next;
    });
  };

  // ── Activity data: live while running, last persisted trace otherwise ──
  const liveParsed = useMemo(
    () => parseLogToSteps(runner.liveState?.live_terminal_log || ''),
    [runner.liveState],
  );
  const steps = runner.isRunning && liveParsed.steps.length > 0 ? liveParsed.steps : stepsOf(runner.messages);
  const artifacts = runner.isRunning ? (runner.liveState?.artifacts ?? null) : artifactsOf(runner.messages);
  const terminalLog = runner.isRunning ? (runner.liveState?.live_terminal_log || '') : '';

  // Gate sends: this tab's own run OR a run in another tab
  const sendBlocked = runner.isRunning || isGlobalRunning;

  return (
    <div className="absolute inset-0 flex min-h-0">
      <FileRail
        session={session}
        files={files}
        loading={filesLoading}
        activeFile={activeFile}
        onOpenFile={openFile}
        onRefresh={refreshFiles}
        collapsed={leftCollapsed}
        onToggleCollapsed={() => setLeftCollapsed((v) => !v)}
      />
      <EditorPane
        openFiles={openFiles}
        activeFile={activeFile}
        content={activeFile ? contentCache[activeFile] ?? null : null}
        loading={contentLoading}
        onActivateFile={setActiveFile}
        onCloseFile={closeFile}
      />
      <ActivityPanel
        steps={steps}
        artifacts={artifacts}
        terminalLog={terminalLog}
        isRunning={runner.isRunning}
        collapsed={rightCollapsed}
        onToggleCollapsed={() => setRightCollapsed((v) => !v)}
      />

      {/* Docked chat panel — same HouseChat chrome as the home card, on the right. */}
      <aside className="w-[380px] shrink-0 border-l house-hairline bg-[#0f0f13] flex flex-col min-h-0 overflow-hidden">
        <HouseChat
          variant="ide-panel"
          title={session.title}
          subtitle={session.workspaceName}
          messages={runner.messages}
          isRunning={sendBlocked}
          isLoading={false}
          notification={runner.notification}
          threadRef={threadRef}
          isVoiceMode={false}
          totalCost={runner.usage.total_cost}
          totalInput={runner.usage.total_input_tokens}
          totalOutput={runner.usage.total_output_tokens}
          totalCacheHitTokens={runner.usage.total_cache_hit_tokens || 0}
          activeContextTokens={runner.activeContextTokens}
          onSend={(prompt, files_) => { if (!sendBlocked) runner.send(session, prompt, files_); }}
          onCancel={runner.cancel}
          onToggleVoice={() => {}}
          onOpenStats={() => {}}
          onOpenInWorkspace={() => { window.location.hash = `#/coding?ws=${encodeURIComponent(session.workspaceId)}&chat=${encodeURIComponent(session.chatId)}`; }}
          onCloseIde={onCloseIde}
          onOpenSubagent={(childId, label, childWorkspace) => setSubagentView({ childId, label, workspaceId: childWorkspace })}
          workspaceId={session.workspaceId}
          chatId={session.chatId}
        />
      </aside>

      {/* Subagent drill-in: full-area read-only session view */}
      {subagentView && (
        <div className="absolute inset-0 z-40 bg-[#0c0c0f] flex flex-col px-4 sm:px-8 lg:px-12 py-4">
          <div className="max-w-[1100px] w-full mx-auto flex-1 min-h-0 flex flex-col">
            <SubagentSessionView
              workspaceId={subagentView.workspaceId || session.workspaceId}
              childId={subagentView.childId}
              label={subagentView.label}
              parentTitle={session.title}
              onBack={() => setSubagentView(null)}
              onOpenSubagentSession={(childId, label, childWorkspace) => setSubagentView({ childId, label, workspaceId: childWorkspace })}
            />
          </div>
        </div>
      )}

      {/* Quick "new file hint" when the project is completely empty */}
      {files.length === 0 && !filesLoading && openFiles.length === 0 && (
        <div className="absolute top-3 left-1/2 -translate-x-1/2 z-20 flex items-center gap-2 px-3 py-1.5 rounded-full border house-hairline bg-[#101014]/85 backdrop-blur text-[11px] text-zinc-400 select-none pointer-events-none">
          <Plus size={11} />
          Empty project — describe what to build in the chat panel
        </div>
      )}
    </div>
  );
}
