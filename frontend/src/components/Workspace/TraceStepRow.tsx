import { useState } from 'react';
import { FileText, File, Folder, ArrowUpRight, Bot, Check, Loader2, ChevronDown, ChevronUp } from 'lucide-react';
import type { TraceStep } from '../../utils/logParser';
import { delegationTarget, isDelegationToolName } from '../../utils/logParser';
import SubagentActivityPanel from './SubagentActivityPanel';
import DiffView from './DiffView';

interface Props {
  step: TraceStep;
  onOpenSubagent?: (name: string, task: string) => void;
  /** Session-keyed drill-in (steps carrying childId) → opens the subagent view. */
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
  activeSubagent?: string | null;
  allTraceSteps?: TraceStep[];
  isActive?: boolean;
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
}

function getFileIcon(fileName: string, colorClass = '', size = 14) {
  const name = fileName.toLowerCase();
  if (name.endsWith('.py')) {
    return (
      <svg width={size} height={size} viewBox="0 0 110 110" className="flex-shrink-0 select-none">
        <path d="M52.33 2c-12.87 0-22.02 1.15-22.02 10.74v7.88h22.44v3.18H10.31C1.15 23.8 2 34.21 2 45.47c0 10.97-.85 21.67 8.31 21.67h7.88V44.7c0-10.74 8.73-19.47 19.47-19.47h22.44v-12.5c0-8.31-7.88-10.73-17.77-10.73z" fill="#3776AB"/>
        <path d="M57.67 108c12.87 0 22.02-1.15 22.02-10.74v-7.88H57.25v-3.18h42.44c9.16 0 8.31-10.41 8.31-21.67 0-10.97.85-21.67-8.31-21.67h-7.88v22.44c0 10.74-8.73 19.47-19.47 19.47H39.9v12.5c0 8.31 7.88 10.73 17.77 10.73z" fill="#FFD43B"/>
        <circle cx="39.09" cy="10.31" r="3.18" fill="#F7F7F7"/>
        <circle cx="70.91" cy="99.69" r="3.18" fill="#3776AB"/>
      </svg>
    );
  }
  if (name.endsWith('.js')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <rect width="24" height="24" rx="2" fill="#F7DF1E"/>
        <path d="M19.14 18.25c-.77.42-1.8.63-2.67.63-1.63 0-2.48-.68-2.48-2.22h1.8c0 .65.3.93.9.93.38 0 .66-.18.66-.54 0-.82-2.58-.57-2.58-2.48 0-1.22.95-1.92 2.37-1.92.83 0 1.63.2 2.22.5l-.54 1.34c-.45-.25-1.09-.43-1.68-.43-.45 0-.7.19-.7.49 0 .8 2.62.58 2.62 2.5 0 1.25-.92 1.95-2.1 1.95zm-6.28.61c-1.32 0-2.25-.66-2.25-2.07v-3.75h1.82v3.73c0 .54.26.78.7.78.34 0 .65-.11.83-.24l.32 1.35c-.34.13-.88.2-1.42.2z" fill="#000000"/>
      </svg>
    );
  }
  if (name.endsWith('.ts')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <rect width="24" height="24" rx="2" fill="#3178C6"/>
        <path d="M19.34 18.15c-.48.33-1.22.49-1.88.49-1.28 0-2.01-.52-2.01-1.66h1.56c0 .44.22.65.65.65.28 0 .5-.13.5-.38 0-.58-1.9-.4-1.9-1.84 0-.88.67-1.4 1.76-1.4.61 0 1.23.15 1.64.39l-.42 1c-.34-.18-.83-.31-1.24-.31-.33 0-.53.13-.53.35 0 .56 1.9.43 1.9 1.83 0 .9-.68 1.4-1.77 1.4zm-6.52.47c-.82 0-1.43-.22-1.77-.47v-4.57H8.8v-1.12h4.52v1.12H11v3.45c.23.11.64.22.95.22.56 0 .84-.25.84-.79v-2.88h1.52v2.9c0 1.28-.77 1.64-1.99 1.64z" fill="#FFFFFF"/>
      </svg>
    );
  }
  if (name.endsWith('.tsx')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <rect width="24" height="24" rx="2" fill="#3178C6"/>
        <text x="12" y="16" fill="#FFFFFF" fontSize="8" fontWeight="black" fontFamily="system-ui, sans-serif" textAnchor="middle">TSX</text>
      </svg>
    );
  }
  if (name.endsWith('.jsx')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <rect width="24" height="24" rx="2" fill="#F7DF1E"/>
        <text x="12" y="16" fill="#000000" fontSize="8" fontWeight="black" fontFamily="system-ui, sans-serif" textAnchor="middle">JSX</text>
      </svg>
    );
  }
  if (name.endsWith('.php')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <ellipse cx="12" cy="12" rx="11" ry="7" fill="#8892BF" />
        <path d="M6.2 14.5H7.7L8.2 12.3H9.4C10.7 12.3 11.4 11.7 11.6 10.6C11.8 9.5 11.2 9 9.9 9H7.6L6.2 14.5ZM8.5 11.1L8.9 9.8H9.7C10.2 9.8 10.4 9.9 10.3 10.3C10.2 10.7 9.9 11.1 9.4 11.1H8.5ZM12.7 14.5H14.1L14.7 12.3H15.9C17.2 12.3 17.9 11.7 18.1 10.6C18.3 9.5 17.7 9 16.4 9H14.1L12.7 14.5ZM15 11.1L15.4 9.8H16.2C16.7 9.8 16.9 9.9 16.8 10.3C16.7 10.7 16.4 11.1 15.9 11.1H15ZM10.5 14.5H12L13.4 9H11.9L10.5 14.5Z" fill="#232531" />
        <path d="M6.5 14.2H8L8.5 12H9.7C11 12 11.7 11.4 11.9 10.3C12.1 9.2 11.5 8.7 10.2 8.7H7.9L6.5 14.2ZM8.8 10.8L9.2 9.5H10C10.5 9.5 10.7 9.6 10.6 10C10.5 10.4 10.2 10.8 9.7 10.8H8.8ZM13 14.2H14.4L15 12H16.2C17.5 12 18.2 11.4 18.4 10.3C18.6 9.2 18 8.7 16.7 8.7H14.4L13 14.2ZM15.3 10.8L15.7 9.5H16.5C17 9.5 17.2 9.6 17.1 10C17 10.4 16.7 10.8 16.2 10.8H15.3ZM10.8 14.2H12.3L13.7 8.7H12.2L10.8 14.2Z" fill="#FFFFFF" />
      </svg>
    );
  }
  if (name.endsWith('.go')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <path d="M1.3 11h5.8v2H1.3zm1.5-3h4.3v2H2.8zm-.8 6h5.1v2H2z" fill="#00ADD8"/>
        <path d="M14.3 6.6c-2.4 0-4.3 1.5-4.3 4.4 0 3 1.9 4.4 4.3 4.4 2.1 0 3.7-1.1 4-2.8H14v-1.7h6.2v.7c0 3.1-2.4 5.3-5.9 5.3-3.8 0-6.4-2.5-6.4-6.1s2.6-6.1 6.4-6.1c3.2 0 5.4 1.9 5.8 4.2h-2c-.3-1.2-1.7-2-3.8-2z" fill="#00ADD8"/>
        <path d="M22.7 11.2c0 2.2-1.6 4.2-4.2 4.2s-4.2-2-4.2-4.2 1.6-4.2 4.2-4.2 4.2 2 4.2 4.2zm-6.2 0c0 1.2.8 2.3 2 2.3s2-1.1 2-2.3-.8-2.3-2-2.3-2 1.1-2 2.3z" fill="#00ADD8"/>
      </svg>
    );
  }
  if (name.endsWith('.rs')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <circle cx="12" cy="12" r="8" fill="#CE412B" />
        <circle cx="12" cy="12" r="5" fill="#1D1F21" />
        <path d="M10.2 9.5H12c.8 0 1.2.3 1.2.8s-.4.8-1.2.8h-1.8V9.5z M10.2 12h1.5c.8 0 1.2.3 1.2.8s-.4.8-1.2.8h-1.5V12z" fill="#FFFFFF" />
        <path d="M9.5 8.5h3c1.2 0 2 .6 2 1.5 0 .8-.5 1.3-1.2 1.5.8.2 1.4.7 1.4 1.6 0 1-.8 1.6-2.2 1.6H9.5V8.5z" fill="#FFFFFF" />
        <path d="M12 2v2M12 20v2M4 12h2M20 12h2M6.3 6.3l1.4 1.4M16.3 16.3l1.4 1.4M6.3 17.7l1.4-1.4M16.3 7.7l1.4-1.4" stroke="#CE412B" strokeWidth="2.5" strokeLinecap="round" />
      </svg>
    );
  }
  if (name.endsWith('.html')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <path d="M3 2l1.6 17.8L12 22l7.4-2.2L21 2H3zm13.6 6H9.7l-.2 2.5h6.8l-.6 6.3L12 18l-3.7-1.2-.2-2.3h2.3l.1 1.1 1.5.5 1.5-.5.2-2.3H8.3l-.6-7.3h10v3h-1.1z" fill="#E34F26"/>
      </svg>
    );
  }
  if (name.endsWith('.css')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <path d="M3 2l1.6 17.8L12 22l7.4-2.2L21 2H3zm13.5 11.2h-5.2l.2 2.5h5l-.6 6.3L12 18l-3.7-1.2-.2-2.3H15l.1 1.1 1.5.5 1.5-.5.2-2.3H8.3l-.6-7.3h10l-.7 7.3z" fill="#1572B6"/>
      </svg>
    );
  }
  if (name.endsWith('.json')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none">
        <rect width="24" height="24" rx="4" fill="#000000" stroke="#333333" strokeWidth="1"/>
        <text x="12" y="16.5" fill="#FFFFFF" fontSize="13" fontWeight="bold" fontFamily="monospace" textAnchor="middle">{"{}"}</text>
      </svg>
    );
  }
  if (name.endsWith('.md')) {
    return (
      <svg width={size} height={size} viewBox="0 0 24 24" className="flex-shrink-0 select-none" fill="#FFFFFF">
        <path d="M22 3H2c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h20c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zM19 18l-4-4h3V9h2v5h3l-4 4zM10 18H8v-5l-2 2-2-2v5H2V9h2l2 2 2-2h2v9z" fill="#8F9CAE" />
      </svg>
    );
  }

  if (name.endsWith('.txt') || name.endsWith('.log') || name.endsWith('.cfg') || name.endsWith('.ini')) {
    return <FileText size={size} className={`flex-shrink-0 ${colorClass || 'text-amber-400'}`} />;
  }
  return <File size={size} className={`flex-shrink-0 ${colorClass || 'text-zinc-400'}`} />;
}

function parseDiffStats(content: string): { added: number; removed: number } | null {
  // Try "[OK] Edit applied (+5 -2)" format
  let m = content.match(/\(?\+(\d+)\s*-(\d+)\)?/);
  if (m) return { added: parseInt(m[1]), removed: parseInt(m[2]) };
  // Try " +5 -2" anywhere
  m = content.match(/\+(\d+)\s+-(\d+)/);
  if (m) return { added: parseInt(m[1]), removed: parseInt(m[2]) };
  return null;
}

function parseLineCount(content: string): number | null {
  // Try "(15 lines, 435 chars)" or "(15 lines)"
  const m = content.match(/\((\d+)\s*lines?/);
  if (m) return parseInt(m[1]);
  return null;
}

export default function TraceStepRow({ step, onOpenSubagent, onOpenSubagentSession, activeSubagent, allTraceSteps = [], isActive = false, workspaceId = null, chatId = null, onCloseSubagent }: Props) {
  const [diffOpen, setDiffOpen] = useState(false);
  let fileName = '', cmdDisplay = '', lineRange = '';

  if (step.args) {
    try {
      const parsed = JSON.parse(step.args);
      const rawPath = parsed.file_path || parsed.path || '';
      if (rawPath) fileName = rawPath.split(/[/\\]/).pop() || '';
      if (step.toolName === 'run_command' && parsed.command) {
        cmdDisplay = parsed.command.length > 60 ? parsed.command.substring(0, 60) + '...' : parsed.command;
      }
      if (step.toolName === 'search_code' && parsed.pattern) cmdDisplay = parsed.pattern;
      if (step.toolName === 'list_files' && parsed.path) cmdDisplay = parsed.path.split(/[/\\]/).pop() || parsed.path;

      const offset = parsed.offset || 1, limit = parsed.limit;
      if (limit) lineRange = `#L${offset}-${offset + limit - 1}`;
      else if (offset > 1) lineRange = `#L${offset}`;
    } catch {
      const fileMatch = step.args.match(/"(?:file_)?path"\s*:\s*"([^"]+)"/);
      if (fileMatch) fileName = fileMatch[1].split(/[/\\]/).pop() || '';
      const cmdMatch = step.args.match(/"command"\s*:\s*"([^"]+)"/);
      if (cmdMatch) cmdDisplay = cmdMatch[1];
    }
  }

  // Extract line range from read_file output: "(lines 1-15 of 15)"
  if (step.content && step.toolName === 'read_file') {
    const match = step.content.match(/\(lines (\d+)-(\d+)/);
    if (match) lineRange = `#L${match[1]}-${match[2]}`;
  }

  if (step.toolName === 'read_file') {
    return (
      <div className="flex items-center gap-2 py-0.5 text-zinc-400 font-sans text-[13.5px] leading-relaxed">
        {getFileIcon(fileName, 'text-orange-400')}
        <span className="text-zinc-200 font-mono font-medium">{fileName || 'file'}</span>
        {lineRange && <span className="text-zinc-600 font-mono text-[12px]">{lineRange}</span>}
      </div>
    );
  }

  if (step.toolName === 'write_file') {
    const lines = step.content ? parseLineCount(step.content) : null;
    return (
      <div className="flex items-center gap-2 py-0.5 text-zinc-400 font-sans text-[13.5px] leading-relaxed">
        {getFileIcon(fileName, 'text-green-400')}
        <span className="text-zinc-200 font-mono font-medium">{fileName || 'file'}</span>
        {lines !== null && (
          <span className="text-emerald-400 font-mono text-[12px] font-medium">+{lines}</span>
        )}
      </div>
    );
  }

  if (step.toolName === 'edit_file') {
    const diff = step.content ? parseDiffStats(step.content) : null;
    const hasDiff = !!step.content && step.content.includes('@@');
    return (
      <div className="flex flex-col py-0.5">
        <div className="flex items-center gap-2 text-zinc-400 font-sans text-[13.5px] leading-relaxed">
          {getFileIcon(fileName, 'text-yellow-400')}
          <span className="text-zinc-200 font-mono font-medium">{fileName || 'file'}</span>
          {diff && (
            <span className="font-mono text-[12px] font-medium">
              {diff.added > 0 && <span className="text-emerald-400">+{diff.added}</span>}
              {diff.added > 0 && diff.removed > 0 && ' '}
              {diff.removed > 0 && <span className="text-red-400">-{diff.removed}</span>}
            </span>
          )}
          {hasDiff && (
            <button
              type="button"
              onClick={() => setDiffOpen((o) => !o)}
              className="inline-flex items-center gap-1 px-1.5 py-0.5 rounded-md text-[11px] text-zinc-500 hover:text-zinc-200 hover:bg-zinc-800/60 transition-colors cursor-pointer"
            >
              {diffOpen ? <ChevronUp size={11} /> : <ChevronDown size={11} />}
              {diffOpen ? 'Hide diff' : 'Diff'}
            </button>
          )}
        </div>
        {diffOpen && hasDiff && (
          <div className="mt-1.5 max-h-[420px] overflow-y-auto rounded-lg border border-zinc-800/60">
            <DiffView diff={step.content} />
          </div>
        )}
      </div>
    );
  }

  if (step.toolName === 'search_code') {
    return (
      <div className="flex items-center gap-1.5 py-0.5 text-zinc-400 font-sans text-[13.5px] leading-relaxed">
        <span>Searched</span>
        <span className="text-zinc-300 font-mono font-medium bg-zinc-900 px-1 py-0.2 rounded border border-zinc-800/60">{cmdDisplay || 'pattern'}</span>
      </div>
    );
  }

  if (step.toolName === 'run_command') {
    return (
      <div className="flex flex-col py-0.5 text-zinc-400 font-sans text-[13.5px]">
        <div className="flex items-center gap-1.5">
          <span>Ran command</span>
          <span className="text-zinc-300 font-mono font-medium bg-zinc-900 px-1 py-0.2 rounded border border-zinc-800/60 truncate max-w-sm">{cmdDisplay}</span>
        </div>
        {step.content && (
          <pre className="mt-1 p-2 bg-zinc-950/50 border border-zinc-900 text-zinc-500 rounded overflow-x-auto max-h-32 font-mono text-[11.5px] leading-normal max-w-xl">
            {step.content}
          </pre>
        )}
      </div>
    );
  }

  if (step.toolName === 'list_files') {
    return (
      <div className="flex items-center gap-1.5 py-0.5 text-zinc-400 font-sans text-[13.5px] leading-relaxed">
        <span>Listed files in</span>
        <Folder size={14} className="flex-shrink-0 text-amber-500/90" />
        <span className="text-zinc-200 font-mono font-medium">{cmdDisplay || '.'}</span>
      </div>
    );
  }

  if (step.toolName === 'view_signatures') {
    return (
      <div className="flex items-center gap-2 py-0.5 text-zinc-400 font-sans text-[13.5px] leading-relaxed">
        <span>Analyzed signatures in</span>
        {getFileIcon(fileName, 'text-blue-400')}
        <span className="text-zinc-200 font-mono font-medium">{fileName || 'file'}</span>
      </div>
    );
  }

  if (isDelegationToolName(step.toolName)) {
    const { name: targetName, task: targetTask } = delegationTarget(step.args);
    const name = targetName || 'Subagent';
    const taskDesc = targetTask;
    const isAsync = (step.toolName === 'call_subagent' || step.toolName === 'delegate') && !!step.childId;

    // Session-keyed spawn (new traces): opens the read-only drill-in view.
    // Whole card is clickable per UX request — not just the Open text.
    if (step.childId) {
      const label = step.label || taskDesc.slice(0, 80) || name;
      const displayLabel = label.length > 90 ? label.slice(0, 90) + '…' : label;
      const wsHint = step.childWorkspace ? String(step.childWorkspace).replace(/^_/, '') : '';
      return (
        <div
          role="button"
          tabIndex={0}
          onClick={() => onOpenSubagentSession?.(step.childId!, label, step.childWorkspace)}
          onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); onOpenSubagentSession?.(step.childId!, label, step.childWorkspace); } }}
          className={`subagent-live-card w-full text-left cursor-pointer hover:border-zinc-700/60 transition-colors ${isActive ? 'is-working' : ''}`}
        >
          <div className="subagent-card-top">
            <span className="subagent-avatar">{isActive ? <Bot size={14} /> : <Check size={14} />}</span>
            <div className="min-w-0 flex-1">
              <div className="subagent-name">{name} <span>subagent</span>{wsHint ? <span className="opacity-60"> · {wsHint}</span> : null}</div>
              <div className={`subagent-state ${isActive ? 'is-working' : 'is-complete'}`}>
                {isActive ? <Loader2 size={10} /> : <Check size={10} />}
                {isActive ? (isAsync ? 'Running asynchronously' : 'Stark → working in this chat') : (isAsync ? 'Async task completed' : 'Delegated task completed')}
              </div>
            </div>
            <span className="subagent-open pointer-events-none">
              Open <ArrowUpRight size={12} />
            </span>
          </div>
          {displayLabel && displayLabel !== name && <p className="subagent-task line-clamp-2">{displayLabel}</p>}
        </div>
      );
    }

    // Legacy spawn (old traces without childId): name-keyed inline activity.
    const activityOpen = activeSubagent?.toLowerCase() === name.toLowerCase();
    return (
      <div className={`subagent-live-card ${activityOpen ? 'is-open' : ''} ${isActive ? 'is-working' : ''}`}>
        <div className="subagent-card-top">
          <span className="subagent-avatar">{isActive ? <Bot size={14} /> : <Check size={14} />}</span>
          <div className="min-w-0 flex-1">
            <div className="subagent-name">{name} <span>subagent</span></div>
            <div className={`subagent-state ${isActive ? 'is-working' : 'is-complete'}`}>
              {isActive ? <Loader2 size={10} /> : <Check size={10} />}
              {isActive ? (isAsync ? 'Running asynchronously' : 'Working on delegated task') : (isAsync ? 'Async task completed' : 'Delegated task completed')}
            </div>
          </div>
          <button type="button" className="subagent-open" onClick={() => activityOpen ? onCloseSubagent?.() : onOpenSubagent?.(name, taskDesc)}>
            {activityOpen ? 'Hide activity' : 'View activity'} {activityOpen ? <ArrowUpRight size={12} className="rotate-90" /> : <ArrowUpRight size={12} />}
          </button>
        </div>
        {taskDesc && <p className="subagent-task">{taskDesc}</p>}
        {activityOpen && (
          <SubagentActivityPanel
            name={name}
            taskDesc={taskDesc}
            steps={allTraceSteps}
            isActive={isActive}
            workspaceId={workspaceId}
            chatId={chatId}
            onClose={() => onCloseSubagent?.()}
          />
        )}
      </div>
    );
  }

  return (
    <div className="py-0.5 text-zinc-400 font-sans text-[13.5px]">
      {step.title || step.toolName || 'Tool Call'}
    </div>
  );
}
