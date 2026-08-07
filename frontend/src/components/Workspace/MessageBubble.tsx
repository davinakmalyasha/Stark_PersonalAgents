import { useState } from 'react';
import { Copy, AlertTriangle, Wrench, Terminal, FileText, Search, Image, ChevronDown, Plus, Pencil, Minus, CheckCircle2, RotateCcw, X } from 'lucide-react';
import type { ArtifactFile, ChatAttachment, ChatMessage, RunArtifacts } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import { undoRun } from '../../services/api';
import TraceAccordion from './TraceAccordion';
import Markdown from './Markdown';

interface Props {
  message: ChatMessage;
  onOpenSubagent: (name: string, task: string) => void;
  activeSubagent?: string | null;
  traceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
}

/** Pick an icon for tool calls based on the tool name */
function toolIcon(content: string) {
  if (content.includes('[read_file]')) return <FileText size={12} className="text-blue-400" />;
  if (content.includes('[write_file]')) return <FileText size={12} className="text-green-400" />;
  if (content.includes('[edit_file]')) return <FileText size={12} className="text-yellow-400" />;
  if (content.includes('[run_command]')) return <Terminal size={12} className="text-cyan-400" />;
  if (content.includes('[search_code]')) return <Search size={12} className="text-pink-400" />;
  if (content.includes('[list_files]')) return <FileText size={12} className="text-zinc-400" />;
  return <Wrench size={12} className="text-purple-400" />;
}

/** Check if a message content is a tool call */
function isToolCall(content: string): boolean {
  return content.startsWith('[TOOL_CALL]');
}

function ArtifactSummary({ artifacts }: { artifacts: RunArtifacts }) {
  const [expanded, setExpanded] = useState(false);
  const [reviewOpen, setReviewOpen] = useState(false);
  const [undoState, setUndoState] = useState<'idle' | 'working' | 'undone' | 'conflict' | 'error'>('idle');
  const files: ArtifactFile[] = Array.isArray(artifacts.files) && artifacts.files.length > 0
    ? artifacts.files
    : [
      ...(artifacts.files_created || []).map((path) => ({ path, status: 'added' as const, additions: 0, deletions: 0 })),
      ...(artifacts.files_modified || []).map((path) => ({ path, status: 'modified' as const, additions: 0, deletions: 0 })),
      ...(artifacts.files_deleted || []).map((path) => ({ path, status: 'deleted' as const, additions: 0, deletions: 0 })),
    ];
  if (!files.length) return null;
  const additions = artifacts.total_additions ?? files.reduce((sum, file) => sum + file.additions, 0);
  const deletions = artifacts.total_deletions ?? files.reduce((sum, file) => sum + file.deletions, 0);
  const visibleFiles = expanded ? files : files.slice(0, 3);
  const hiddenCount = Math.max(0, files.length - visibleFiles.length);

  const handleUndo = async () => {
    if (!artifacts.run_id || undoState === 'working' || undoState === 'undone') return;
    setUndoState('working');
    try {
      const result = await undoRun(artifacts.run_id);
      setUndoState(result.status === 'conflict' ? 'conflict' : 'undone');
    } catch {
      setUndoState('error');
    }
  };

  const statusIcon = (status: ArtifactFile['status']) => {
    if (status === 'added') return <Plus size={14} className="artifact-icon artifact-icon--created" />;
    if (status === 'deleted') return <Minus size={14} className="artifact-icon artifact-icon--deleted" />;
    return <Pencil size={12} className="artifact-icon artifact-icon--modified" />;
  };

  return (
    <div className="artifact-summary">
      <div className="artifact-summary-top">
        <div className="artifact-summary-icon"><FileText size={22} /></div>
        <div className="artifact-summary-heading">
          <strong>Edited {files.length} file{files.length === 1 ? '' : 's'}</strong>
          <span><b className="artifact-total-add">+{additions}</b> <b className="artifact-total-delete">-{deletions}</b>{artifacts.tests_passing && <em><CheckCircle2 size={11} /> Tests passed</em>}</span>
        </div>
        <div className="artifact-summary-actions">
          <button type="button" onClick={handleUndo} disabled={!artifacts.run_id || undoState === 'working' || undoState === 'undone'} title={!artifacts.run_id ? 'Undo is only available for recent runs' : 'Undo this run'}>{undoState === 'working' ? 'Undoing…' : undoState === 'undone' ? 'Undone' : undoState === 'conflict' ? 'Conflict' : 'Undo'} <RotateCcw size={15} /></button>
          <button type="button" onClick={() => setReviewOpen(true)}>Review</button>
        </div>
      </div>
      <div className="artifact-file-list">
        {visibleFiles.map((file) => <div className="artifact-file-row" key={`${file.status}-${file.path}`}>
          <span className="artifact-file-name">{statusIcon(file.status)}<span>{file.path}</span></span>
          <span className="artifact-file-stats"><b className="artifact-total-add">+{file.additions}</b> <b className="artifact-total-delete">-{file.deletions}</b></span>
        </div>)}
      </div>
      {files.length > 3 && <button type="button" className="artifact-show-more" onClick={() => setExpanded((value) => !value)}>Show {expanded ? 'less' : `${hiddenCount} more`} file{hiddenCount === 1 ? '' : 's'} <ChevronDown size={15} className={expanded ? 'is-expanded' : ''} /></button>}
      {reviewOpen && <div className="artifact-review-backdrop" onClick={() => setReviewOpen(false)}>
        <div className="artifact-review-modal" onClick={(event) => event.stopPropagation()}>
          <div className="artifact-review-header"><div><span>RUN REVIEW</span><h3>Edited {files.length} files</h3></div><button type="button" onClick={() => setReviewOpen(false)} aria-label="Close review"><X size={17} /></button></div>
          <div className="artifact-review-body">{files.map((file) => <section key={`review-${file.path}`}><div className="artifact-review-file"><span>{statusIcon(file.status)}{file.path}</span><span><b className="artifact-total-add">+{file.additions}</b> <b className="artifact-total-delete">-{file.deletions}</b></span></div><pre>{file.diff || 'No text diff available for this file.'}</pre></section>)}</div>
        </div>
      </div>}
    </div>
  );
}

export default function MessageBubble({ message, onOpenSubagent, activeSubagent, traceSteps = [], workspaceId, chatId, onCloseSubagent }: Props) {
  const isUser = message.role === 'user';
  const isError = message.role === 'error';
  const isAgent = message.role === 'agent';
  const isSystem = message.role === 'system';
  const isTool = isAgent && isToolCall(message.content);

  return (
    <div className="animate-fade-in group" style={{ marginBottom: '24px' }}>
      {/* ────────────────────────────────────
          USER MESSAGE — simple text display
          ──────────────────────────────────── */}
      {isUser && (
        <div className="flex justify-end">
          <div 
            className="max-w-[85%] text-[15.5px] text-zinc-100 leading-relaxed user-message-premium"
            style={{ padding: '10px 16px', borderRadius: '14px' }}
          >
            {message.content && <Markdown content={message.content} />}
            {Array.isArray(message.metadata?.attachments) && <div className="message-attachments">
              {(message.metadata.attachments as ChatAttachment[]).map((file) => <div className="message-attachment" key={file.id}>
                {file.isImage ? <Image size={14} /> : <FileText size={14} />}<span className="truncate">{file.name}</span>
              </div>)}
            </div>}
          </div>
        </div>
      )}

      {/* ────────────────────────────────────
          TOOL CALL — special rendering with icon
          ──────────────────────────────────── */}
      {isTool && (
        <div 
          className="flex items-start gap-2 rounded-lg bg-indigo-500/5 border border-indigo-500/15"
          style={{ padding: '12px' }}
        >
          <div className="flex-shrink-0 mt-0.5">
            {toolIcon(message.content)}
          </div>
          <div className="min-w-0">
            <span className="text-zinc-500 font-bold text-[11px] uppercase tracking-wider">
              {message.agentName || 'Developer'}
            </span>
            <div className="text-zinc-300 font-mono text-[12.5px] mt-0.5 leading-relaxed whitespace-pre-wrap break-all">
              {message.content}
            </div>
          </div>
        </div>
      )}

      {/* ────────────────────────────────────
          AGENT MESSAGE — with status + actions
          ──────────────────────────────────── */}
      {isAgent && !isTool && (
        <div className="agent-message-content">
          {/* Agent name label */}
          {message.agentName && (
            <div className="text-[11px] font-bold text-zinc-500 uppercase tracking-wider mb-1.5">
              {message.agentName}
            </div>
          )}

          {/* Trace status above the response so duration reads first */}
          {!!message.metadata?.isTrace && Array.isArray(message.metadata.steps) && (
            <div className="agent-message-trace">
              <TraceAccordion
              steps={message.metadata.steps as TraceStep[]}
              isActive={message.duration === 'active'}
              duration={message.metadata?.duration as string || message.duration}
              onOpenSubagent={onOpenSubagent}
              activeSubagent={activeSubagent}
              allTraceSteps={traceSteps}
              workspaceId={workspaceId}
              chatId={chatId}
              onCloseSubagent={onCloseSubagent}
              />
            </div>
          )}

          {/* Main response text content */}
          {message.content && message.content !== '(Execution Trace)' && (
            <div className="agent-message-response text-[15.5px] text-zinc-200 leading-relaxed">
              <Markdown content={message.content} />
            </div>
          )}

          {!!message.metadata?.artifacts && (
            <ArtifactSummary artifacts={message.metadata.artifacts as RunArtifacts} />
          )}

          {/* Inline action icons — right-aligned, revealed on hover */}
          <div className="flex items-center justify-end gap-0.5 mt-2 opacity-0 group-hover:opacity-100 transition-opacity">
            <button
              className="p-1.5 text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800 rounded-md transition-colors"
              title="Copy to clipboard"
              onClick={() => navigator.clipboard?.writeText(message.content)}
            >
              <Copy size={13} />
            </button>
          </div>
        </div>
      )}

      {/* ────────────────────────────────────
          ERROR MESSAGE — alert/error block
          ──────────────────────────────────── */}
      {isError && (
        <div 
          className="flex items-start gap-2.5 rounded-lg bg-red-500/5 border border-red-500/15"
          style={{ padding: '12px' }}
        >
          <AlertTriangle size={14} className="text-red-400 flex-shrink-0 mt-0.5" />
          <div className="min-w-0">
            <span className="text-[14.5px] font-semibold text-red-400">Error</span>
            <p className="text-[14.5px] text-red-300/80 mt-0.5 leading-relaxed whitespace-pre-wrap">
              {message.content}
            </p>
          </div>
        </div>
      )}

      {/* ────────────────────────────────────
          SYSTEM MESSAGE — low-contrast italic
          ──────────────────────────────────── */}
      {isSystem && (
        <div className="system-message-note">
          <span className="system-message-dot" />
          <span>{message.content}</span>
        </div>
      )}
    </div>
  );
}
