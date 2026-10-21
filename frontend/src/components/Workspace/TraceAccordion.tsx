import { useState } from 'react';
import { ChevronDown } from 'lucide-react';
import ExecutionTrace from './ExecutionTrace';
import ActivityTimeline, { DelegationCards } from './ActivityTimeline';
import { unmatchedDelegationSteps } from '../../utils/delegationMatch';
import type { TraceStep } from '../../utils/logParser';
import { isDelegationToolName } from '../../utils/logParser';
import type { StreamSegment } from '../../utils/streamAccum';

interface Props {
  steps: TraceStep[];
  isActive?: boolean;
  duration?: string;
  timeline?: StreamSegment[];
  onOpenSubagent?: (name: string, task: string) => void;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
  activeSubagent?: string | null;
  allTraceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
}

export default function TraceAccordion({ steps, isActive, duration, timeline, onOpenSubagent, onOpenSubagentSession, activeSubagent, allTraceSteps, workspaceId, chatId, onCloseSubagent }: Props) {
  const [isOpen, setIsOpen] = useState(false);

  const toggleOpen = () => setIsOpen((open) => !open);

  // Live: render only delegation cards as an inline overlay (no purple
  // "Agent activity" shell). The reasoning timeline above already shows
  // thinking progress; this keeps the "open delegated chat" action visible
  // without reintroducing the purple block the user flagged everywhere.
  if (isActive) {
    // Children claimed by a timeline tool event render inline at their call
    // site — the overlay covers only unclaimed (legacy/orphan) children.
    const hasDelegationEvent = timeline?.some((segment) => segment.type === 'tool' && isDelegationToolName(segment.ev.name));
    // Structured live activity renders delegation rows in their original
    // position. The legacy cards remain only when an older stream has no
    // corresponding tool event to attach them to.
    if (hasDelegationEvent) return null;
    const delegateSteps = unmatchedDelegationSteps(timeline || [], steps);
    if (!delegateSteps.length) return null;
    return (
      <div className="animate-[thinkingFadeIn_180ms_ease-out]">
        <DelegationCards
          delegationSteps={delegateSteps}
          isActive
          onOpenSubagentSession={onOpenSubagentSession}
        />
      </div>
    );
  }

  const formatDurationText = (dur?: string): string => {
    if (!dur) return "Worked for a few seconds";
    // Handle "active" string during live run — parseLogToSteps can return it
    if (dur === 'active') return "Worked for a few seconds";
    const num = parseInt(dur);
    if (isNaN(num)) return `Worked for ${dur}`;
    return `Worked for ${num}s`;
  };

  const durationText = formatDurationText(duration);
  const hasTimeline = !!timeline?.length;
  const hasDelegationEvents = !!timeline?.some((segment) => segment.type === 'tool' && isDelegationToolName(segment.ev.name));
  // If a legacy child step is already represented by a fallback card, remove
  // it from the old trace renderer so the same delegation is not shown twice.
  const executionSteps = hasTimeline ? steps.filter((step) => !step.childId) : steps;
  const hasLegacyActions = executionSteps.some((step) => step.type === 'tool' || step.type === 'command' || step.type === 'system');
  // Children claimed by a timeline delegation event already render inline at
  // their call site. The fallback below covers only unclaimed children
  // (legacy traces whose stream has no matching tool event) — and always
  // renders AFTER the chronological timeline, never hoisted above it.
  const fallbackDelegations = hasTimeline ? unmatchedDelegationSteps(timeline || [], steps) : [];

  return (
    <div className={`trace-shell ${isOpen ? 'trace-complete-open' : ''}`}>
      <div
        role="button"
        tabIndex={0}
        aria-expanded={isOpen}
        aria-label={`${durationText} activity details`}
        onClick={toggleOpen}
        onKeyDown={(event) => {
          if (event.key === 'Enter' || event.key === ' ') {
            event.preventDefault();
            toggleOpen();
          }
        }}
        style={{ marginBottom: isOpen ? '14px' : '0px' }}
        className="trace-complete-header"
      >
        <span>{durationText}</span>
        <ChevronDown size={14} className={`text-zinc-500 transition-transform duration-200 ${isOpen ? '' : '-rotate-90'}`} />
      </div>

      <div className={`trace-collapsible ${isOpen ? 'is-open' : ''}`}><div>
        {hasTimeline && (
          <ActivityTimeline
            segments={timeline || []}
            delegationSteps={steps}
            onOpenSubagentSession={onOpenSubagentSession}
          />
        )}
        {(!hasTimeline || (!hasDelegationEvents && hasLegacyActions)) && (
          <ExecutionTrace steps={executionSteps} isActive={false} onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} activeSubagent={activeSubagent} allTraceSteps={allTraceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} />
        )}
        {fallbackDelegations.length > 0 && (
          <DelegationCards
            delegationSteps={fallbackDelegations}
            onOpenSubagentSession={onOpenSubagentSession}
          />
        )}
      </div></div>
    </div>
  );
}
