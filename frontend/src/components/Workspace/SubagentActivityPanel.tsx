import { useEffect, useMemo, useState } from 'react';
import { Check, ChevronDown, ChevronUp, CircleDot, Loader2, X } from 'lucide-react';
import type { ChatMessage } from '../../types';
import { apiUrl } from '../../services/api';
import type { TraceStep } from '../../utils/logParser';

interface Props {
  name: string;
  taskDesc: string;
  steps: TraceStep[];
  isActive: boolean;
  workspaceId: string | null;
  chatId: string | null;
  onClose: () => void;
}

function isAgentStep(step: TraceStep, name: string) {
  return step.agent.toLowerCase() === name.toLowerCase();
}

export default function SubagentActivityPanel({ name, taskDesc, steps, isActive, workspaceId, chatId, onClose }: Props) {
  const [history, setHistory] = useState<ChatMessage[]>([]);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [showResponse, setShowResponse] = useState(true);

  const agentSteps = useMemo(() => steps.filter((step) => isAgentStep(step, name)), [steps, name]);
  const thoughts = agentSteps.filter((step) => step.type === 'thought');
  const tools = agentSteps.filter((step) => step.type === 'tool' || step.type === 'command');
  const response = [...history].reverse().find((message) => (message.role as string) === 'assistant' && message.content.trim());

  useEffect(() => {
    if (!workspaceId || !chatId || !name) return;
    setHistoryLoading(true);
    fetch(apiUrl(`/api/workspaces/${encodeURIComponent(workspaceId)}/chats/${encodeURIComponent(chatId)}/subagents/${encodeURIComponent(name)}`))
      .then((res) => (res.ok ? res.json() : []))
      .then((data) => setHistory(Array.isArray(data) ? data : []))
      .catch(() => setHistory([]))
      .finally(() => setHistoryLoading(false));
  }, [workspaceId, chatId, name]);

  return (
    <div className="subagent-activity-panel" role="region" aria-label={`${name} activity`}>
      <div className="subagent-activity-header">
        <div className="flex min-w-0 items-center gap-2">
          <span className={`subagent-activity-status ${isActive ? 'is-live' : 'is-done'}`}>
            {isActive ? <Loader2 size={11} /> : <Check size={11} />}
          </span>
          <div className="min-w-0">
            <div className="subagent-activity-title">{name} activity</div>
            <div className="subagent-activity-meta">
              {thoughts.length} thought{thoughts.length === 1 ? '' : 's'} · {tools.length} tool{tools.length === 1 ? '' : 's'}
            </div>
          </div>
        </div>
        <button type="button" className="subagent-activity-close" onClick={onClose} aria-label="Close activity">
          <X size={13} />
        </button>
      </div>

      {taskDesc && <div className="subagent-activity-task">{taskDesc}</div>}

      <div className="subagent-activity-feed">
        {agentSteps.length > 0 ? agentSteps.map((step) => (
          <div className="subagent-activity-step" key={step.id}>
            <CircleDot size={10} className={step.type === 'thought' ? 'text-violet-300' : 'text-zinc-500'} />
            <div className="min-w-0">
              <div className="subagent-step-title">{step.title}</div>
              {step.content && <div className="subagent-step-content">{step.content}</div>}
            </div>
          </div>
        )) : (
          <div className="subagent-activity-empty">
            {isActive ? <><Loader2 size={12} /> Waiting for the first update…</> : 'No execution steps were recorded.'}
          </div>
        )}
      </div>

      {(historyLoading || response) && (
        <div className="subagent-response-section">
          <button type="button" className="subagent-response-toggle" onClick={() => setShowResponse((open) => !open)}>
            <span>Subagent response</span>
            {showResponse ? <ChevronUp size={13} /> : <ChevronDown size={13} />}
          </button>
          {showResponse && (historyLoading ? (
            <div className="subagent-response-copy"><Loader2 size={12} /> Loading response…</div>
          ) : (
            <div className="subagent-response-copy">{response?.content}</div>
          ))}
        </div>
      )}
    </div>
  );
}
