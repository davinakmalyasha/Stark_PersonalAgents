import { useState } from 'react';
import { Bot, CheckCircle2, ChevronDown, ChevronRight, FileText, Loader2, Search, Terminal, Wrench, X } from 'lucide-react';
import { delegationTarget, isDelegationToolName, type TraceStep } from '../../utils/logParser';
import { buildEntries, resolveDelegations, type ToolGroupEntry } from '../../utils/delegationMatch';
import type { LiveToolEvent, StreamSegment } from '../../utils/streamAccum';

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

function toolLabel(event: LiveToolEvent): { verb: string; subject: string } {
  const subject = primaryArg(event.args);
  const verb = {
    read: 'Read',
    read_file: 'Read',
    list_files: 'Listed files',
    write: 'Wrote',
    write_file: 'Wrote',
    edit: 'Edited',
    edit_file: 'Edited',
    read_image: 'Read image',
    bash: 'Ran command',
    run_command: 'Ran command',
    grep: 'Searched',
    glob: 'Listed files',
    search_code: 'Searched',
    web_search: 'Searched web',
    web_fetch: 'Fetched',
    task: 'Delegated',
    call_subagent: 'Delegated',
    delegate: 'Delegated',
    spawn_agent: 'Spawned agent',
    message_agent: 'Messaged agent',
    send_message: 'Messaged agent',
    followup_task: 'Followed up',
    wait_agent: 'Waited for agent',
    list_agents: 'Listed agents',
    interrupt_agent: 'Interrupted agent',
    close_agent: 'Closed agent',
    job: 'Managed job',
    job_list: 'Listed jobs',
    job_output: 'Read job',
    job_kill: 'Killed job',
    todo_write: 'Updated todo list',
    update_plan: 'Updated plan',
    exit_plan_mode: 'Proposed plan',
    ask_user: 'Asked',
    ask_user_question: 'Asked',
    skill: 'Loaded skill',
    working_directory: 'Changed directory',
    memory: 'Recalled memory',
  }[event.name] || event.name;
  return { verb, subject };
}

function toolSummaryPart(event: LiveToolEvent): string {
  return {
    read: 'read files',
    read_file: 'read files',
    list_files: 'explored files',
    write: 'wrote files',
    write_file: 'wrote files',
    edit: 'edited files',
    edit_file: 'edited files',
    bash: 'ran commands',
    run_command: 'ran commands',
    grep: 'searched code',
    glob: 'explored files',
    search_code: 'searched code',
    web_search: 'searched the web',
    web_fetch: 'fetched pages',
    task: 'created an agent',
    call_subagent: 'created an agent',
    delegate: 'created an agent',
    spawn_agent: 'created an agent',
    message_agent: 'messaged an agent',
    send_message: 'messaged an agent',
    followup_task: 'sent a follow-up',
    wait_agent: 'waited for an agent',
    list_agents: 'listed agents',
    interrupt_agent: 'interrupted an agent',
    close_agent: 'closed an agent',
    job: 'managed a job',
    job_list: 'listed jobs',
    job_output: 'read a job',
    job_kill: 'killed a job',
    todo_write: 'updated the todo list',
    update_plan: 'updated the plan',
    exit_plan_mode: 'proposed a plan',
    ask_user: 'asked a question',
    ask_user_question: 'asked a question',
    skill: 'loaded a skill',
    working_directory: 'changed directory',
    memory: 'recalled memory',
  }[event.name] || 'used tools';
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

function ToolEventDetail({ event, delegations, isActive, onOpenSubagentSession }: {
  event: LiveToolEvent;
  delegations?: TraceStep[];
  isActive?: boolean;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}) {
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

  return (
    <div className="activity-tool-detail" role="listitem" aria-label={`${verb}${subject ? ` ${subject}` : ''}`}>
      <span className="activity-tool-detail-icon"><ToolIcon name={event.name} /></span>
      <span className="activity-tool-detail-title">{verb}</span>
      {subject && <code className="activity-event-subject">{subject}</code>}
      <span className={`activity-event-status ${event.ok === false ? 'is-error' : ''}`} aria-label={statusLabel}>
        <ToolStatus event={event} />
      </span>
      {event.status === 'done' && typeof event.ms === 'number' && event.ms > 0 && (
        <span className="activity-event-duration">{event.ms >= 1000 ? `${(event.ms / 1000).toFixed(1)}s` : `${event.ms}ms`}</span>
      )}
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
        {entry.segment.text}
      </div>
    );
        }

        return (
          <div className="activity-inline-note activity-inline-note--status" role="listitem" aria-label="Agent update" key={`message-${index}`}>
            {entry.segment.text}
          </div>
        );
      })}
    </div>
  );
}
