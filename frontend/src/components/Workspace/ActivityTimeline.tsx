import { useState } from 'react';
import { Bot, Check, CheckCircle2, ChevronDown, ChevronRight, Copy, FileText, Loader2, Search, Terminal, Wrench, X } from 'lucide-react';
import { delegationTarget, isDelegationToolName, type TraceStep } from '../../utils/logParser';
import { buildEntries, resolveDelegations, type ToolGroupEntry } from '../../utils/delegationMatch';
import { TOOL_SUMMARIES, TOOL_VERBS } from '../../utils/toolLabels.generated';
import type { LiveToolEvent, StreamSegment } from '../../utils/streamAccum';
import DiffView from './DiffView';
import Markdown from './Markdown';

interface Props {
  segments: StreamSegment[];
  isActive?: boolean;
  delegationSteps?: TraceStep[];
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}

function primaryArg(args?: string): string {
  if (!args) return '';
  try {
    const parsed = JSON.parse(args) as Record<string, unknown>;
    // Parallel fan-out: summarize the delegated agents.
    const items = parsed.items;
    if (Array.isArray(items) && items.length) {
      const names = items
        .map((item) => String((item as Record<string, unknown>)?.agent || (item as Record<string, unknown>)?.name || ''))
        .filter(Boolean);
      const text = names.length ? names.join(', ') : `${items.length} agents`;
      return text.length > 72 ? `${text.slice(0, 72)}…` : text;
    }
    const value = parsed.file_path ?? parsed.path ?? parsed.command ?? parsed.cmd
      ?? parsed.pattern ?? parsed.name ?? parsed.task ?? parsed.task_name ?? parsed.target
      ?? parsed.job_id ?? parsed.url ?? parsed.query ?? parsed.queries ?? parsed.prompt ?? parsed.message ?? '';
    const text = String(value || '');
    return text.length > 72 ? `${text.slice(0, 72)}…` : text;
  } catch {
    const text = args.trim();
    return text.length > 72 ? `${text.slice(0, 72)}…` : text;
  }
}

// Historical tool names no longer emitted (kept so old transcripts still label).
const LEGACY_VERBS: Record<string, string> = {
  read_image: 'Read image',
  list_files: 'Listed files',
  task: 'Delegated',
  send_message: 'Messaged agent',
  followup_task: 'Followed up',
  job_list: 'Listed jobs',
  job_output: 'Read job',
  job_kill: 'Killed job',
  exit_plan_mode: 'Proposed plan',
  working_directory: 'Changed directory',
};

const LEGACY_SUMMARIES: Record<string, string> = {
  read_image: 'read images',
  list_files: 'explored files',
  task: 'created an agent',
  send_message: 'messaged an agent',
  followup_task: 'sent a follow-up',
  job_list: 'listed jobs',
  job_output: 'read a job',
  job_kill: 'killed a job',
  exit_plan_mode: 'proposed a plan',
  working_directory: 'changed directory',
};

function toolLabel(event: LiveToolEvent): { verb: string; subject: string } {
  const subject = primaryArg(event.args);
  const verb = TOOL_VERBS[event.name] || LEGACY_VERBS[event.name] || event.name;
  return { verb, subject };
}

function toolSummaryPart(event: LiveToolEvent): string {
  return TOOL_SUMMARIES[event.name] || LEGACY_SUMMARIES[event.name] || 'used tools';
}

function toolGroupTitle(events: LiveToolEvent[]): string {
  const parts = Array.from(new Set(events.map(toolSummaryPart)));
  const title = parts.length <= 2 ? parts.join(', ') : `${parts.slice(0, 2).join(', ')}, and more`;
  return title ? `${title.charAt(0).toUpperCase()}${title.slice(1)}` : 'Used tools';
}

function ToolIcon({ name }: { name: string }) {
  if (name === 'bash' || name === 'run_command' || name === 'job') return <Terminal size={13} />;
  if (name === 'grep' || name === 'glob' || name === 'search_code' || name === 'web_search') return <Search size={13} />;
  if (name === 'read' || name === 'read_image' || name === 'read_file' || name === 'write' || name === 'write_file' || name === 'edit' || name === 'edit_file' || name === 'list_files') return <FileText size={13} />;
  return <Wrench size={13} />;
}

function structuredDelegationSteps(event: LiveToolEvent): TraceStep[] {
  return (event.children || []).filter((child) => !!child.chatId).map((child, index) => ({
    id: child.jobId || `${event.id}-child-${index}`,
    type: 'system',
    agent: event.agent || '',
    title: child.label || 'Delegated task',
    content: '',
    childId: child.chatId,
    label: child.label,
    childName: child.agentName,
    childWorkspace: child.workspaceId,
  }));
}

/**
 * Child steps for one delegation tool event, in call order. The first child
 * attaches to the delegation tool step itself; extra children from a parallel
 * `delegate` fan-out (and legacy orphans with no tool anchor) follow it as
 * standalone steps and are absorbed here — so every card renders inline at
 * its call site instead of being hoisted to the top.
 */
function ToolStatus({ event }: { event: LiveToolEvent }) {
  if (event.status === 'running') return <Loader2 size={13} className="animate-spin" />;
  if (event.ok === false) return <X size={13} />;
  return <CheckCircle2 size={13} />;
}

function delegationName(event?: LiveToolEvent, step?: TraceStep): string {
  if (step?.childName) return step.childName;
  if (!event?.args) return 'Subagent';
  return delegationTarget(event.args).name || 'Subagent';
}

function DelegationCard({ event, step, isActive, onOpenSubagentSession }: {
  event?: LiveToolEvent;
  step?: TraceStep;
  isActive?: boolean;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}) {
  const name = delegationName(event, step);
  const label = step?.label || step?.title || (event ? primaryArg(event.args) : '') || 'Delegated task';
  const isRunning = isActive ?? event?.status === 'running';
  const isFailed = event?.ok === false;
  const canOpen = !!step?.childId && !!onOpenSubagentSession;
  const status = isRunning ? 'Working' : isFailed ? 'Failed' : 'Completed';
  const content = (
    <>
      <span className="activity-delegation-icon"><Bot size={15} /></span>
      <span className="activity-delegation-body">
        <span className="activity-delegation-name">{name} <span>subagent</span></span>
        <span className="activity-delegation-task">{label}</span>
      </span>
      <span className={`activity-delegation-status ${isFailed ? 'is-error' : ''}`}>
        {isRunning ? <Loader2 size={12} className="animate-spin" /> : <CheckCircle2 size={12} />}
        <span>{status}</span>
        {canOpen && <span className="activity-delegation-open">Open</span>}
      </span>
    </>
  );

  return (
    <div className="activity-delegation-item" role="listitem">
      {canOpen ? (
        <button
          type="button"
          className="activity-delegation-card"
          onClick={() => onOpenSubagentSession?.(step!.childId!, label, step?.childWorkspace)}
          aria-label={`Open ${name} subagent session`}
        >
          {content}
        </button>
      ) : (
        <div className="activity-delegation-card is-static">{content}</div>
      )}
    </div>
  );
}

/**
 * Fallback projection for older traces whose persisted steps contain a child
 * session but whose live timeline has no matching task event.  It deliberately
 * lives inside the trace accordion, so the child card is part of the ordered
 * history rather than a second detached message.
 */
export function DelegationCards({
  delegationSteps,
  isActive = false,
  onOpenSubagentSession,
}: {
  delegationSteps?: TraceStep[];
  isActive?: boolean;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}) {
  const childSteps = (delegationSteps || []).filter((step) => !!step.childId);
  if (!childSteps.length) return null;

  return (
    <div className="activity-delegation-list" role="list" aria-label="Delegated agents">
      {childSteps.map((step) => (
        <DelegationCard
          key={step.id}
          step={step}
          isActive={isActive}
          onOpenSubagentSession={onOpenSubagentSession}
        />
      ))}
    </div>
  );
}

function parseToolArgs(args?: string): Record<string, unknown> | null {
  if (!args) return null;
  try {
    const parsed = JSON.parse(args) as unknown;
    return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
      ? parsed as Record<string, unknown>
      : null;
  } catch {
    return null;
  }
}

const ANSI_ESCAPE = String.fromCharCode(27);
const ANSI_CSI = new RegExp(`${ANSI_ESCAPE}\\[[0-9;?]*[ -/]*[@-~]`, 'g');
const ANSI_OSC = new RegExp(`${ANSI_ESCAPE}\\][^${String.fromCharCode(7)}]*(?:${String.fromCharCode(7)}|${ANSI_ESCAPE}\\\\)`, 'g');

function stripAnsi(text: string): string {
  return (text || '')
    .replace(ANSI_CSI, '')
    .replace(ANSI_OSC, '');
}

function textField(args: Record<string, unknown> | null, keys: string[]): string {
  if (!args) return '';
  for (const key of keys) {
    const value = args[key];
    if (typeof value === 'string' && value.trim()) return value;
  }
  return '';
}

function prettyInput(event: LiveToolEvent): string {
  const parsed = parseToolArgs(event.args);
  if (!parsed) return (event.args || '').trim();
  return JSON.stringify(parsed, null, 2);
}

function editDiff(event: LiveToolEvent, args: Record<string, unknown> | null): { diff: string; filename: string } | null {
  if (!args) return null;
  const path = textField(args, ['file_path', 'path', 'file']);
  const name = path || 'file';
  if (event.name === 'edit' || event.name === 'edit_file') {
    const oldText = textField(args, ['old_string', 'oldText', 'old']);
    const newText = textField(args, ['new_string', 'newText', 'new']);
    if (!oldText && !newText) return null;
    const oldLines = oldText.split(/\r?\n/);
    const newLines = newText.split(/\r?\n/);
    return {
      filename: name,
      diff: [
        `--- a/${name}`,
        `+++ b/${name}`,
        `@@ -1,${Math.max(1, oldLines.length)} +1,${Math.max(1, newLines.length)} @@`,
        ...oldLines.map((line) => `-${line}`),
        ...newLines.map((line) => `+${line}`),
      ].join('\n'),
    };
  }
  if (event.name === 'write' || event.name === 'write_file') {
    const content = textField(args, ['content', 'text', 'body']);
    if (!content) return null;
    const lines = content.split(/\r?\n/);
    return {
      filename: name,
      diff: [
        '--- /dev/null',
        `+++ b/${name}`,
        `@@ -0,0 +1,${lines.length} @@`,
        ...lines.map((line) => `+${line}`),
      ].join('\n'),
    };
  }
  if (event.name === 'apply_patch' || event.name === 'patch') {
    const patch = textField(args, ['patch', 'input', 'diff']);
    if (!patch || (!patch.includes('@@') && !patch.includes('---'))) return null;
    return { filename: name, diff: patch };
  }
  return null;
}

function isTerminalTool(name: string): boolean {
  return name === 'bash' || name === 'run_command' || name === 'ds_shell' || name === 'job';
}

function ToolOutput({ output, running, empty = 'No output captured.' }: { output: string; running: boolean; empty?: string }) {
  if (!output.trim()) {
    return (
      <div className="activity-tool-empty">
        {running ? <><Loader2 size={12} className="animate-spin" /> Waiting for output…</> : empty}
      </div>
    );
  }
  return <pre className="activity-tool-output">{output}</pre>;
}

function ToolPanel({ event }: { event: LiveToolEvent }) {
  const [copied, setCopied] = useState(false);
  const args = parseToolArgs(event.args);
  const output = stripAnsi(event.preview || '');
  const running = event.status === 'running';
  const diff = editDiff(event, args);
  const command = textField(args, ['command', 'cmd', 'script']);
  const path = textField(args, ['file_path', 'path', 'file']);
  const copy = async () => {
    const text = diff
      ? `${diff.filename}\n\n${diff.diff}`
      : [command, output].filter(Boolean).join('\n\n');
    try {
      await navigator.clipboard.writeText(text || prettyInput(event));
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1400);
    } catch {
      /* clipboard unavailable */
    }
  };

  return (
    <div className="activity-tool-panel">
      <div className="activity-tool-panel-head">
        <span className="activity-tool-panel-name">{event.name}</span>
        <span className={`activity-tool-panel-status ${event.ok === false ? 'is-error' : ''}`}>
          {running ? 'Running' : event.ok === false ? 'Failed' : 'Completed'}
        </span>
        <button type="button" className="activity-tool-copy" onClick={() => { void copy(); }} title="Copy tool details" aria-label="Copy tool details">
          {copied ? <Check size={11} className="text-emerald-400" /> : <Copy size={11} />}
        </button>
      </div>

      {diff ? (
        <div className="activity-tool-diff">
          <DiffView diff={diff.diff} filename={diff.filename} />
          {output.trim() && <div className="activity-tool-diff-note">{output}</div>}
        </div>
      ) : isTerminalTool(event.name) ? (
        <>
          <div className="activity-tool-command">
            <span className="activity-tool-prompt">$</span>
            <code>{command || textField(args, ['description']) || event.name}</code>
          </div>
          {event.args && !command && <pre className="activity-tool-input">{prettyInput(event)}</pre>}
          <ToolOutput output={output} running={running} />
        </>
      ) : (
        <>
          <div className="activity-tool-section">
            <span className="activity-tool-section-label">Input</span>
            <pre className="activity-tool-input">{prettyInput(event) || '—'}</pre>
          </div>
          <div className="activity-tool-section">
            <span className="activity-tool-section-label">
              {path ? `Output · ${path}` : 'Output'}
            </span>
            <ToolOutput output={output} running={running} />
          </div>
        </>
      )}
    </div>
  );
}

function ToolEventDetail({ event, delegations, isActive, onOpenSubagentSession }: {
  event: LiveToolEvent;
  delegations?: TraceStep[];
  isActive?: boolean;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}) {
  const [open, setOpen] = useState(false);
  if (isDelegationToolName(event.name)) {
    // One card per spawned child, inline at the delegation call.
    const children = structuredDelegationSteps(event);
    const childSteps = children.length ? children : delegations;
    if (childSteps?.length) {
      return (
        <>
          {childSteps.map((step) => (
            <DelegationCard key={step.id} event={event} step={step} isActive={isActive} onOpenSubagentSession={onOpenSubagentSession} />
          ))}
        </>
      );
    }
    return <DelegationCard event={event} step={undefined} isActive={isActive} onOpenSubagentSession={onOpenSubagentSession} />;
  }
  const { verb, subject } = toolLabel(event);
  const statusLabel = event.status === 'running' ? 'Running' : event.ok === false ? 'Failed' : 'Completed';
  const hasDetails = !!(event.args || event.preview || event.status === 'running');

  return (
    <div className={`activity-tool-call ${open ? 'is-open' : ''}`} role="listitem">
      <button
        type="button"
        className="activity-tool-call-row"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
        aria-label={`${verb}${subject ? ` ${subject}` : ''} — ${statusLabel}`}
      >
        <span className="activity-tool-detail-icon"><ToolIcon name={event.name} /></span>
        <span className="activity-tool-detail-title">{verb}</span>
        {subject && <code className="activity-event-subject">{subject}</code>}
        <span className={`activity-event-status ${event.ok === false ? 'is-error' : ''}`} aria-label={statusLabel}>
          <ToolStatus event={event} />
        </span>
        {event.status === 'done' && typeof event.ms === 'number' && event.ms > 0 && (
          <span className="activity-event-duration">{event.ms >= 1000 ? `${(event.ms / 1000).toFixed(1)}s` : `${event.ms}ms`}</span>
        )}
        {hasDetails && (
          open ? <ChevronDown size={13} className="activity-tool-chevron" /> : <ChevronRight size={13} className="activity-tool-chevron" />
        )}
      </button>
      {open && <ToolPanel event={event} />}
    </div>
  );
}

function ToolGroupRow({ entry, defaultOpen, isActive, delegations, onOpenSubagentSession }: {
  entry: ToolGroupEntry;
  defaultOpen: boolean;
  isActive?: boolean;
  delegations: Array<TraceStep[]>;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}) {
  const [open, setOpen] = useState(defaultOpen);
  const title = toolGroupTitle(entry.events);

  return (
    <div className="activity-tool-group" role="listitem">
      <button
        type="button"
        className="activity-tool-group-header"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="activity-tool-group-icon"><Wrench size={13} /></span>
        <span className="activity-tool-group-title">{title}</span>
        {entry.events.length > 1 && <span className="activity-tool-group-count">{entry.events.length} actions</span>}
        {open ? <ChevronDown size={14} className="activity-tool-group-chevron" /> : <ChevronRight size={14} className="activity-tool-group-chevron" />}
      </button>
      {open && (
        <div className="activity-tool-group-body" role="list" aria-label={`${title} details`}>
          {entry.events.map((event, index) => (
            <ToolEventDetail
              key={event.id}
              event={event}
              delegations={delegations[index]}
              isActive={isActive}
              onOpenSubagentSession={onOpenSubagentSession}
            />
          ))}
        </div>
      )}
    </div>
  );
}

export default function ActivityTimeline({ segments, isActive = false, delegationSteps, onOpenSubagentSession }: Props) {
  if (!segments.length) return null;
  const entries = buildEntries(segments);
  const lastToolEntry = entries.reduce((last, entry, index) => entry.kind === 'tools' ? index : last, -1);
  const resolvedDelegations = resolveDelegations(entries, delegationSteps);

  return (
    <div className="activity-timeline" role="list" aria-label="Agent activity">
      {entries.map((entry, index) => {
        if (entry.kind === 'tools') {
          const delegations = entry.events.map((event) => resolvedDelegations.get(event.id) || []);
          return (
            <ToolGroupRow
              key={`tools-${entry.events[0]?.id || index}`}
              entry={entry}
              defaultOpen={isActive && index === lastToolEntry}
              isActive={isActive}
              delegations={delegations}
              onOpenSubagentSession={onOpenSubagentSession}
            />
          );
        }

        if (entry.kind === 'reasoning') {
    return (
      <div className="activity-inline-note activity-inline-note--reasoning" role="listitem" aria-label="Reasoning update" key={`reasoning-${index}`}>
        <Markdown content={entry.segment.text} compact />
      </div>
    );
        }

        return (
          <div className="activity-inline-note activity-inline-note--status" role="listitem" aria-label="Agent update" key={`message-${index}`}>
            <Markdown content={entry.segment.text} compact />
          </div>
        );
      })}
    </div>
  );
}
