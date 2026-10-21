import { useEffect, useState } from 'react';
import { ChevronRight, ChevronDown, BrainCircuit, FolderSearch, FilePenLine } from 'lucide-react';
import type { GroupedTraceStep, TraceStep } from '../../utils/logParser';
import TraceStepRow from './TraceStepRow';

interface Props {
  group: GroupedTraceStep;
  isLastActive: boolean;
  onOpenSubagent?: (name: string, task: string) => void;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
  activeSubagent?: string | null;
  allTraceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
}

export default function TraceGroupRow({ group, isLastActive, onOpenSubagent, onOpenSubagentSession, activeSubagent, allTraceSteps, workspaceId, chatId, onCloseSubagent }: Props) {
  const [expanded, setExpanded] = useState(isLastActive);
  const [activeSeconds, setActiveSeconds] = useState(0);

  useEffect(() => {
    if (!isLastActive) return;
    const startedAt = Date.now();
    const tick = () => setActiveSeconds(Math.max(1, Math.round((Date.now() - startedAt) / 1000)));
    tick();
    const timer = window.setInterval(tick, 1000);
    return () => window.clearInterval(timer);
  }, [group.id, isLastActive]);

  const getTitle = () => {
    if (isLastActive) {
      if (group.type === 'thought') {
        const loggedDuration = group.steps[0]?.duration;
        return `Thinking for ${loggedDuration || `${activeSeconds}s`}`;
      }
      if (group.type === 'exploration') return 'Exploring files...';
      if (group.type === 'edit') return 'Editing files...';
      return group.title;
    }
    if (group.type === 'thought') {
      const firstStep = group.steps[0];
      const thoughtDuration = firstStep?.duration;
      if (thoughtDuration) {
        const num = parseInt(thoughtDuration);
        return isNaN(num) ? `Thought for ${thoughtDuration}` : `Thought for ${num}s`;
      }
      // Estimate duration based on reasoning content length if exact timing isn't logged (1s per ~15 words, min 1s)
      const content = firstStep?.content || '';
      const wordCount = content.trim().split(/\s+/).filter(Boolean).length;
      const estimatedSecs = Math.max(1, Math.round(wordCount / 15));
      return `Thought for ${estimatedSecs}s`;
    }
    if (group.type === 'exploration' || group.type === 'edit') {
      const stepCount = group.steps.length;
      const searchCount = group.steps.filter(s => s.toolName === 'search_code').length;
      if (group.type === 'exploration' && searchCount > 0) {
        return `Explored ${stepCount - searchCount} file${(stepCount - searchCount) !== 1 ? 's' : ''}, ${searchCount} search${searchCount !== 1 ? 'es' : ''}`;
      }
      return group.title;
    }
    return group.title;
  };

  const hasCollapsibleContent = group.type === 'thought' || group.type === 'exploration' || group.type === 'edit';

  if (!hasCollapsibleContent) {
    return (
      <div className="trace-system-row">
        {group.steps.map((s) => (
          <TraceStepRow key={s.id} step={s} onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} isActive={isLastActive} activeSubagent={activeSubagent} allTraceSteps={allTraceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} />
        ))}
      </div>
    );
  }

  const thoughtContent = group.steps[0]?.content || '';
  const lines = thoughtContent.split('\n').map(l => l.trim()).filter(Boolean);

  let thoughtTitle = '';
  let thoughtBody;

  if (lines.length > 1 && lines[0].length < 80) {
    thoughtTitle = lines[0];
    thoughtBody = lines.slice(1).join('\n');
  } else {
    thoughtBody = thoughtContent;
  }

  return (
    <div className={`trace-group trace-${group.type} ${isLastActive ? 'is-active' : ''}`}>
      {/* Header Row — whole row clickable, not just the chevron/icon */}
      <div
        role="button"
        tabIndex={0}
        onClick={() => setExpanded(!expanded)}
        onKeyDown={(e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); setExpanded(!expanded); } }}
        style={{ marginBottom: expanded ? '8px' : '0px' }}
        className="trace-group-header w-full cursor-pointer"
      >
        <span className="trace-group-icon">{group.type === 'thought' ? <BrainCircuit size={13}/> : group.type === 'edit' ? <FilePenLine size={13}/> : <FolderSearch size={13}/>}</span>
        <span>{getTitle()}</span>
        {expanded
          ? <ChevronDown size={14} className="text-zinc-500" />
          : <ChevronRight size={14} className="text-zinc-500" />
        }
      </div>

      {/* Expanded Content */}
      {expanded && (
        <div className="trace-group-content">
          {group.type === 'thought' ? (
            <div className="trace-thought-card">
              {thoughtTitle && <div className="trace-thought-title">{thoughtTitle}</div>}
              {thoughtBody && <div className="trace-thought-body">{thoughtBody}</div>}
            </div>
          ) : (
            <div className="trace-tool-stack">
              {group.steps.map((s) => (
                <TraceStepRow key={s.id} step={s} onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} isActive={isLastActive} activeSubagent={activeSubagent} allTraceSteps={allTraceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} />
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
