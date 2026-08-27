import { useState } from 'react';
import { ChevronDown, Loader2, Sparkles } from 'lucide-react';
import ExecutionTrace from './ExecutionTrace';
import type { TraceStep } from '../../utils/logParser';

interface Props {
  steps: TraceStep[];
  isActive?: boolean;
  duration?: string;
  onOpenSubagent?: (name: string, task: string) => void;
  onOpenSubagentSession?: (childId: string, label: string) => void;
  activeSubagent?: string | null;
  allTraceSteps?: TraceStep[];
  workspaceId?: string | null;
  chatId?: string | null;
  onCloseSubagent?: () => void;
}

export default function TraceAccordion({ steps, isActive, duration, onOpenSubagent, onOpenSubagentSession, activeSubagent, allTraceSteps, workspaceId, chatId, onCloseSubagent }: Props) {
  const [isOpen, setIsOpen] = useState(false);

  if (isActive) return <div className="trace-shell trace-live"><div className="trace-live-header"><span className="trace-live-orb"><Loader2 size={13}/></span><div><span className="trace-eyebrow">Agent activity</span><span className="trace-live-label">Working through your request</span></div><span className="trace-live-status"><Sparkles size={11}/> Live</span></div><ExecutionTrace steps={steps} isActive onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} activeSubagent={activeSubagent} allTraceSteps={allTraceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} /></div>;

  const formatDurationText = (dur?: string): string => {
    if (!dur) return "Worked for a few seconds";
    // Handle "active" string during live run — parseLogToSteps can return it
    if (dur === 'active') return "Worked for a few seconds";
    const num = parseInt(dur);
    if (isNaN(num)) return `Worked for ${dur}`;
    return `Worked for ${num}s`;
  };

  const durationText = formatDurationText(duration);

  return (
    <div className={`trace-shell ${isOpen ? 'trace-complete-open' : ''}`}>
      <div
        onClick={() => setIsOpen(!isOpen)}
        style={{ marginBottom: isOpen ? '14px' : '0px' }}
        className="trace-complete-header"
      >
        <span>{durationText}</span>
        <ChevronDown size={14} className={`text-zinc-500 transition-transform duration-200 ${isOpen ? '' : '-rotate-90'}`} />
      </div>

      <div className={`trace-collapsible ${isOpen ? 'is-open' : ''}`}><div><ExecutionTrace steps={steps} isActive={false} onOpenSubagent={onOpenSubagent} onOpenSubagentSession={onOpenSubagentSession} activeSubagent={activeSubagent} allTraceSteps={allTraceSteps} workspaceId={workspaceId} chatId={chatId} onCloseSubagent={onCloseSubagent} /></div></div>
    </div>
  );
}
