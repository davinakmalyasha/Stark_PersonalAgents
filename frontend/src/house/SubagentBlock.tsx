import { Check, LayoutGrid } from 'lucide-react';
import type { TraceStep } from '../utils/logParser';

interface SubagentBlockProps {
  step: TraceStep;
  /** Live state of the spawned child — drives the running/done visuals. */
  status?: 'running' | 'done';
  onOpen: (childId: string, label: string) => void;
}

/**
 * Inline pill for a spawned subagent (opencode-style): grid icon + bold agent
 * type + muted task label. Running = breathing glow + shimmer sweep; done =
 * check pop, settled. Clicking drills into the child's read-only session.
 */
export default function SubagentBlock({ step, status = 'done', onOpen }: SubagentBlockProps) {
  if (!step.childId) return null;
  const name = step.childName || step.agent || step.title || 'Agent';
  const label = step.label && step.label !== name ? step.label : '';
  const running = status === 'running';
  return (
    <button
      type="button"
      onClick={() => onOpen(step.childId!, step.label || name)}
      title={`Open ${name} session`}
      className={`subagent-block group flex items-center gap-2.5 self-start max-w-full rounded-lg border px-3 py-2 text-left transition-colors cursor-pointer ${running ? 'is-running' : 'is-done'}`}
    >
      <span className="subagent-block-icon grid place-items-center w-5 h-5 rounded-[5px] shrink-0">
        {running ? <LayoutGrid size={11} className="animate-spin [animation-duration:2.4s]" /> : <Check size={12} />}
      </span>
      <span className="text-[12.5px] font-semibold text-zinc-200 shrink-0">{name}</span>
      {label && (
        <span className="text-[12px] text-zinc-500 truncate">
          {label}
          {running && <span className="subagent-block-ellipsis" />}
        </span>
      )}
      {running ? (
        <span className="subagent-block-dot shrink-0" title="Running" />
      ) : (
        <span className="text-[10px] text-zinc-600 group-hover:text-zinc-400 shrink-0 transition-colors">open →</span>
      )}
    </button>
  );
}
