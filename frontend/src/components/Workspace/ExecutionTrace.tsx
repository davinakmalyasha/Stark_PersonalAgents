import { groupTraceSteps, type TraceStep } from '../../utils/logParser';
import TraceGroupRow from './TraceGroupRow';

interface Props {
  steps: TraceStep[];
  isActive?: boolean;
  onOpenSubagent?: (name: string, task: string) => void;
  activeSubagent?: string | null;
  allTraceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
}

export default function ExecutionTrace({ steps, isActive, onOpenSubagent, activeSubagent, allTraceSteps, workspaceId, chatId, onCloseSubagent }: Props) {
  const grouped = groupTraceSteps(steps);
  if (grouped.length === 0) return null;

  return (
    <div className="trace-timeline">
      {grouped.map((group, idx) => (
        <TraceGroupRow 
          key={group.id} 
          group={group} 
          isLastActive={!!isActive && idx === grouped.length - 1} 
          onOpenSubagent={onOpenSubagent}
          activeSubagent={activeSubagent}
          allTraceSteps={allTraceSteps}
          workspaceId={workspaceId}
          chatId={chatId}
          onCloseSubagent={onCloseSubagent}
        />
      ))}
    </div>
  );
}
