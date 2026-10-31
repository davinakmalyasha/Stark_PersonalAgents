import { useState } from 'react';
import { Copy, CircleAlert, Wrench, Terminal, FileText, Search, Image, ChevronDown, Plus, Pencil, Minus, CheckCircle2, RotateCcw, X, FolderGit2, ExternalLink } from 'lucide-react';
import type { ArtifactFile, ChatAttachment, ChatMessage, ProjectRef, RunArtifacts } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import type { StreamSegment } from '../../utils/streamAccum';
import { presentStream, stripDelegationNarration } from '../../utils/streamPresentation';
import { undoRun } from '../../services/api';
import TraceAccordion from './TraceAccordion';
import ActivityTimeline from './ActivityTimeline';
import Markdown from './Markdown';
import DiffView from './DiffView';

interface Props {
  message: ChatMessage;
  onOpenSubagent: (name: string, task: string) => void;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
  activeSubagent?: string | null;
  traceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
  onOpenProject?: (ref: { workspace_id: string; chat_id: string }) => void;
  onRetry?: () => void;
  onRewind?: (message: ChatMessage) => void;
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

/**
 * Phase 8-E1: streaming markdown splitter.
 * Renders the stable prefix as real Markdown; defers only the trailing
 * fragment that might be mid-construct (unclosed code fence / ** / ` / table
 * row). Prevents mid-fence visual breakage while keeping live rendering.
 */
function StreamingMarkdown({ content }: { content: string }) {
  if (!content) return null;

  let split = content.length;

  // Unclosed fenced code block: render everything before the ``` as Markdown,
  // keep the fence + the rest as plain text.
  const fences = content.split('```');
  if (fences.length > 1 && fences.length % 2 === 0) {
    const openIdx = content.lastIndexOf('```');
    const lineStart = content.lastIndexOf('\n', openIdx - 1) + 1;
    // Only treat as "open" if the opening fence isn't followed by a closer.
    if (openIdx >= 0 && (content.indexOf('```', openIdx + 3) === -1)) {
      split = lineStart;
    }
  }

  // Unclosed inline code / bold — drop the dangling opener so Markdown
  // doesn't swallow the rest of the message.
  let trailing = content.slice(split);
  const dangling = trailing.match(/^.*?(\*\*|`|~~)\s*$/);
  if (dangling && content.slice(0, split).includes(dangling[1])) {
    trailing = trailing.replace(/(\*\*|`|~~)\s*$/, '');
  }

  const stable = content.slice(0, split);
  return (
    <span className="whitespace-pre-wrap break-words">
      {stable ? <Markdown content={stable} /> : null}
      {trailing}
    </span>
  );
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
          <button type="button" className="artifact-action-pill" onClick={handleUndo} disabled={!artifacts.run_id || undoState === 'working' || undoState === 'undone'} title={!artifacts.run_id ? 'Undo is only available for recent runs' : 'Undo this run'}>{undoState === 'working' ? 'Undoing…' : undoState === 'undone' ? 'Undone' : undoState === 'conflict' ? 'Conflict' : 'Undo'} <RotateCcw size={15} /></button>
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
          <div className="artifact-review-body">{files.map((file) => <section key={`review-${file.path}`}><div className="artifact-review-file"><span>{statusIcon(file.status)}{file.path}</span><span><b className="artifact-total-add">+{file.additions}</b> <b className="artifact-total-delete">-{file.deletions}</b></span></div><DiffView diff={file.diff || ''} filename={file.path} /></section>)}</div>
        </div>
      </div>}
    </div>
  );
}

/** P9: compact card for replies that were routed straight to another
 * project's Dev agent (full reply lives in that project's chat). */
function RoutedProjectCard({ projectRef, onOpenProject }: { projectRef: ProjectRef; onOpenProject?: (ref: { workspace_id: string; chat_id: string }) => void }) {
  const [expanded, setExpanded] = useState(false);
  const fullReply = projectRef.full_reply || '';
  const preview = fullReply.length > 220 ? fullReply.slice(0, 220).trimEnd() + '…' : fullReply;

  return (
    <div className="routed-card rounded-xl border border-indigo-500/20 bg-indigo-500/[0.04] overflow-hidden">
      <div className="flex items-center gap-2.5 px-3.5 py-2.5 border-b border-indigo-500/10">
        <FolderGit2 size={15} className="text-indigo-400 flex-shrink-0" />
        <div className="min-w-0 flex-1">
          <div className="text-[12px] font-bold text-zinc-200 truncate">Project {projectRef.name} · Dev reply</div>
          <div className="text-[10.5px] text-zinc-500">Routed directly to the project's Dev agent</div>
        </div>
        {onOpenProject && (
          <button
            type="button"
            className="pressable flex items-center gap-1.5 px-2.5 py-1.5 rounded-lg bg-indigo-500/10 hover:bg-indigo-500/20 text-indigo-300 hover:text-indigo-200 text-[11px] font-semibold transition-colors cursor-pointer flex-shrink-0"
            onClick={() => onOpenProject({ workspace_id: projectRef.workspace_id, chat_id: projectRef.chat_id })}
          >
            <ExternalLink size={12} /> Open in {projectRef.name}
          </button>
        )}
      </div>
      <div className="px-3.5 py-2.5">
        {expanded ? (
          <div className="text-[14px] text-zinc-300 leading-relaxed">
            <Markdown content={fullReply} />
          </div>
        ) : (
          <p className="text-[13.5px] text-zinc-400 leading-relaxed whitespace-pre-wrap">{preview}</p>
        )}
        <button
          type="button"
          className="pressable flex items-center gap-1 mt-2 text-[11.5px] font-semibold text-indigo-400 hover:text-indigo-300 transition-colors cursor-pointer"
          onClick={() => setExpanded((v) => !v)}
        >
          <ChevronDown size={13} className={expanded ? 'rotate-180 transition-transform' : 'transition-transform'} />
          {expanded ? 'Hide details' : 'Show details'}
        </button>
      </div>
    </div>
  );
}

/** Lightweight "Thinking" placeholder shown right after send, before any
 * reasoning deltas arrive. Fades in/out and disappears once the first
 * reasoning segment appears. */
function ThinkingPlaceholder() {
  return (
    <div className="animate-[thinkingFadeIn_220ms_ease-out] flex items-center gap-2 text-[13px] text-zinc-500 mb-2">
      <span className="relative flex h-1.5 w-1.5 flex-shrink-0">
        <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-white opacity-25" />
        <span className="relative inline-flex rounded-full h-1.5 w-1.5 bg-zinc-400" />
      </span>
      <span className="tracking-wide animate-pulse">Thinking</span>
      <span className="inline-flex gap-0.5 ml-0.5">
        <span className="w-1 h-1 rounded-full bg-zinc-500 animate-[thinkingDot_1.1s_ease-in-out_infinite]" style={{ animationDelay: '0ms' }} />
        <span className="w-1 h-1 rounded-full bg-zinc-500 animate-[thinkingDot_1.1s_ease-in-out_infinite]" style={{ animationDelay: '160ms' }} />
        <span className="w-1 h-1 rounded-full bg-zinc-500 animate-[thinkingDot_1.1s_ease-in-out_infinite]" style={{ animationDelay: '320ms' }} />
      </span>
    </div>
  );
}

export default function MessageBubble({ message, onOpenSubagent, onOpenSubagentSession, activeSubagent, traceSteps = [], workspaceId, chatId, onCloseSubagent, onOpenProject, onRetry, onRewind }: Props) {
  const [rewindConfirmOpen, setRewindConfirmOpen] = useState(false);
  const isUser = message.role === 'user';
  const isError = message.role === 'error';
  const isAgent = message.role === 'agent';
  const isSystem = message.role === 'system';
  const isTool = isAgent && isToolCall(message.content);
  const showAgentName = !!message.agentName;
  const streamingActive = !!message.metadata?.isStreaming;
  const persistedThinking = (message.metadata?.thinking as string | undefined) || '';
  // Unified timeline: live uses streaming.segments, history uses persisted liveTimeline
  // Fallback to synthesized reasoning segment when only thinking string exists (old messages)
  const rawTimeline = (message.metadata?.liveTimeline as StreamSegment[] | undefined) || [];
  const timelineSegments: StreamSegment[] = rawTimeline.length
    ? rawTimeline
    : persistedThinking
      ? [{ type: 'reasoning' as const, text: persistedThinking }]
      : [];
  const streamPresentation = presentStream(timelineSegments, message.content);
  const delegationSteps = Array.isArray(message.metadata?.steps) ? message.metadata.steps as TraceStep[] : [];
  const responseContent = stripDelegationNarration(streamPresentation.finalText || message.content, delegationSteps);
  const activitySegments = streamPresentation.activitySegments;
  // The "Worked for" block owns reasoning/tool history only. Plain Q&A has
  // neither, so render no collapsible at all — the answer below stays the
  // single, always-visible copy and the usage chip stays pinned beneath it.
  const hasRenderableTraceSteps = delegationSteps.some((s) =>
    !!s.childId || s.type === 'tool' || s.type === 'command' || !!s.content?.trim(),
  );
  const showTrace = !!message.metadata?.isTrace && (activitySegments.length > 0 || hasRenderableTraceSteps);
  const wasStopped = !!message.metadata?.stopped;
  const usageMeta = message.metadata?.usage as { total_input_tokens?: number; total_output_tokens?: number; total_cost?: number } | undefined;
  const isScheduledWake = !!message.metadata?.scheduledWake;
  const isCanvasEvent = !!message.metadata?.canvasEvent;

  return (
    <div className={`group ${isUser ? 'msg-user-enter' : isAgent ? 'msg-agent-enter' : 'animate-fade-in'}`} style={{ marginBottom: '24px' }}>
      {/* ────────────────────────────────────
          USER MESSAGE — simple text display
          ──────────────────────────────────── */}
      {isUser && (
        <div className="flex justify-end">
          <div 
            className="max-w-[85%] text-[15.5px] text-zinc-100 leading-relaxed user-message-premium"
            style={{ padding: '10px 16px', borderRadius: '14px' }}
          >
            {(isScheduledWake || isCanvasEvent) && (
              <div className="mb-1.5 text-[9px] font-semibold uppercase tracking-[0.16em] text-amber-300/80">
                {isScheduledWake ? 'Scheduled wake' : 'Canvas event'}
              </div>
            )}
            {message.content && <Markdown content={message.content} />}
            {Array.isArray(message.metadata?.attachments) && <div className="message-attachments">
              {(message.metadata.attachments as ChatAttachment[]).map((file) => <div className="message-attachment" key={file.id}>
                {file.isImage ? <Image size={14} /> : <FileText size={14} />}<span className="truncate">{file.name}</span>
              </div>)}
            </div>}
          </div>
        </div>
      )}
      {isUser && onRewind && (
        <div className="message-revert-row">
          {!rewindConfirmOpen ? (
            <button
              type="button"
              onClick={() => setRewindConfirmOpen(true)}
              title="Revert to before this message"
              className="message-revert-pill"
            >
              <RotateCcw size={11} /> Revert message
            </button>
          ) : (
            <div className="message-revert-confirm" role="group" aria-label="Confirm message revert">
              <span>Files changed after this message will be restored and the prompt will return to the composer.</span>
              <button type="button" onClick={() => setRewindConfirmOpen(false)}>Cancel</button>
              <button type="button" className="is-primary" onClick={() => { setRewindConfirmOpen(false); onRewind(message); }}>Revert</button>
            </div>
          )}
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
          {showAgentName && (
            <div className="text-[11px] font-bold text-zinc-500 uppercase tracking-wider mb-1.5">
              {message.agentName}
            </div>
          )}

          {/* Timeline — live shown inline; history timeline is inside "Worked for" dropdown */}
          {message.duration === 'active' && (
            activitySegments.length ? (
              <ActivityTimeline
                segments={activitySegments.slice(-18)}
                isActive
                delegationSteps={delegationSteps}
                onOpenSubagentSession={onOpenSubagentSession}
              />
            ) : !responseContent ? (
              <ThinkingPlaceholder />
            ) : null
          )}

          {/* Trace status owns the complete chronological activity history.
              Delegation cards are rendered by its ordered timeline, so a
              child chat never appears as a second, detached projection. */}
              {showTrace && (
                <div className="agent-message-trace">
                  <TraceAccordion
                  steps={(message.metadata?.steps as TraceStep[]) || []}
                  isActive={message.duration === 'active'}
                  duration={message.metadata?.duration as string || message.duration}
                  timeline={activitySegments}
                  onOpenSubagent={onOpenSubagent}
                  onOpenSubagentSession={onOpenSubagentSession}
                  activeSubagent={activeSubagent}
                  allTraceSteps={traceSteps}
                  workspaceId={workspaceId}
                  chatId={chatId}
                  onCloseSubagent={onCloseSubagent}
                  />
                </div>
              )}

          {/* P9: routed-to-project replies render as a compact card (the full
              reply lives in the project's own chat). */}
          {wasStopped && !streamingActive && (
            <div className="inline-flex items-center gap-1.5 mt-1.5 px-2 py-0.5 rounded-md bg-zinc-500/10 border border-zinc-500/20 text-[11px] font-semibold text-zinc-400">
              Stopped — partial response kept
            </div>
          )}
          {!!message.metadata?.project_ref && (
            <div className="mb-2">
              <RoutedProjectCard
                projectRef={message.metadata.project_ref as ProjectRef}
                onOpenProject={onOpenProject}
              />
            </div>
          )}

          {/* Main response text content */}
          {responseContent && responseContent !== '(Execution Trace)' && !message.metadata?.project_ref && (
            <div className="agent-message-response text-[15.5px] text-zinc-200 leading-relaxed">
              {activitySegments.length > 0 && !streamingActive && message.duration !== 'active' && (
                <div className="agent-response-kicker">Final response</div>
              )}
              {streamingActive ? (
                // Phase 8-E1: incremental streaming markdown. Parse the stable
                // prefix (everything before the last unfinished fence/emphasis)
                // as Markdown so headings/bullets/links render live, and render
                // only the trailing incomplete fragment as plain text — no
                // mid-fence breakage, no per-delta full re-parse.
                <StreamingMarkdown content={responseContent} />
              ) : (
                <Markdown content={responseContent} />
              )}
              {streamingActive && message.duration === 'active' && (
                <span className="stream-caret" aria-hidden="true">▍</span>
              )}
            </div>
          )}

          {/* P10: per-message usage chip (tokens + cost for this run) */}
          {!!usageMeta && !streamingActive && (
            <div className="agent-message-footer flex items-center gap-3 mt-2 text-[10.5px] font-mono text-zinc-600">
              <span title="Input tokens">↑{((usageMeta.total_input_tokens) || 0).toLocaleString()}</span>
              <span title="Output tokens">↓{((usageMeta.total_output_tokens) || 0).toLocaleString()}</span>
              <span title="Cost">${((usageMeta.total_cost) || 0).toFixed(5)}</span>
              {message.duration && <span title="Duration">{message.duration}</span>}
            </div>
          )}

          {!!message.metadata?.artifacts && (
            <div className="agent-message-footer">
              <ArtifactSummary artifacts={message.metadata.artifacts as RunArtifacts} />
            </div>
          )}

          {/* Inline action icons — right-aligned, revealed on hover */}
          <div className="agent-message-footer flex items-center justify-end gap-0.5 mt-2 opacity-0 group-hover:opacity-100 transition-opacity">
            {onRetry && !streamingActive && (
              <button
                className="pressable p-1.5 text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800 rounded-md transition-colors"
                title="Retry — re-run the last request"
                onClick={() => onRetry()}
              >
                <RotateCcw size={13} />
              </button>
            )}
            <button
              className="pressable p-1.5 text-zinc-500 hover:text-zinc-300 hover:bg-zinc-800 rounded-md transition-colors"
              title="Copy to clipboard"
               onClick={() => navigator.clipboard?.writeText(responseContent)}
            >
              <Copy size={13} />
            </button>
          </div>
        </div>
      )}

      {/* ────────────────────────────────────
          ERROR MESSAGE — inline, no banner box
          ──────────────────────────────────── */}
      {isError && (
        <div className="flex items-start gap-2.5 py-1">
          <CircleAlert size={15} className="text-red-400/90 flex-shrink-0 mt-[3px]" />
          <p className="min-w-0 text-[13px] text-red-300/90 leading-relaxed whitespace-pre-wrap break-words">
            {message.content}
          </p>
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
