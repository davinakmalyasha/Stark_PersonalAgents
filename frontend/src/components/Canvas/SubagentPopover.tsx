import { X, Loader2 } from 'lucide-react';
import type { TraceStep } from '../../utils/logParser';
import type { CanvasNode } from '../../types/canvas.types';
import ExecutionTrace from '../Workspace/ExecutionTrace';

interface Props {
  node: CanvasNode;
  steps: TraceStep[];
  taskDesc: string;
  isRunning: boolean;
  onClose: () => void;
}

export default function SubagentPopover({ node, steps, taskDesc, isRunning, onClose }: Props) {
  const filteredSteps = steps.filter((s) => s.type !== 'thought');
  const thoughtCount = steps.filter((s) => s.type === 'thought').length;

  // Position the popover to the right of the node card.
  // Node card is 240px wide, centred at node.x — so right edge is node.x + 120.
  // Add 16px gap.
  const left = node.x + 157;
  const top = node.y - 88;

  return (
    <div
      className="absolute z-50 animate-fade-in"
      style={{ left: `${left}px`, top: `${top}px` }}
      // Prevent canvas pan when interacting inside popover
      onMouseDown={(e) => e.stopPropagation()}
    >
      {/* Connector line hint */}
      <div
        className="absolute top-[52px] bg-zinc-700/60 rounded-full"
        style={{ left: '-14px', width: '14px', height: '1px' }}
      />

      <div
        className="subagent-popover bg-[#202126]/95 border border-white/[0.13] shadow-2xl flex flex-col overflow-hidden"
        style={{ width: '318px', maxHeight: '360px', borderRadius: '18px' }}
      >
        {/* Header */}
        <div
          className="flex items-center justify-between flex-shrink-0 px-4 py-3 border-b border-zinc-800/60"
        >
          <div className="flex items-center gap-2 min-w-0">
            {isRunning && (
              <Loader2 size={12} className="text-indigo-400 animate-spin flex-shrink-0" />
            )}
            <div className="min-w-0">
              <span className="text-[13px] font-bold text-zinc-100 truncate block">
                {node.label}
              </span>
              <div className="flex items-center gap-2 text-[10px] text-zinc-500 mt-0.5">
                <span>{filteredSteps.length} steps</span>
                {thoughtCount > 0 && <span>· {thoughtCount} thoughts</span>}
              </div>
            </div>
          </div>
          <button
            onClick={onClose}
            className="p-1 hover:bg-zinc-800 rounded-md text-zinc-500 hover:text-zinc-200 cursor-pointer flex-shrink-0 transition-colors"
          >
            <X size={13} />
          </button>
        </div>

        {/* Task description */}
        {taskDesc && (
          <div className="px-4 pt-3 pb-2 border-b border-zinc-800/40">
            <p className="text-[11px] text-zinc-400 leading-relaxed line-clamp-3">{taskDesc}</p>
          </div>
        )}

        {/* Execution steps */}
        <div className="flex-1 overflow-y-auto px-3 py-2" style={{ scrollbarWidth: 'thin' }}>
          {filteredSteps.length === 0 ? (
            <div className="flex items-center gap-2 py-4 text-[12px] text-zinc-600 justify-center">
              {isRunning
                ? <><Loader2 size={12} className="animate-spin text-zinc-600" /> Waiting for activity…</>
                : 'No steps recorded'}
            </div>
          ) : (
            <ExecutionTrace steps={filteredSteps} isActive={isRunning} />
          )}
        </div>
      </div>
    </div>
  );
}
