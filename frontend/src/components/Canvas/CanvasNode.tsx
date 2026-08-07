import type { CanvasNode } from '../../types/canvas.types';
import { statusConfig } from './canvasNode.styles';

interface Props {
  node: CanvasNode;
  steps: any[];
  isActive: boolean;
  isVoiceMode: boolean;
  onClick: () => void;
}

export default function CanvasNode({ node, steps, isActive, isVoiceMode, onClick }: Props) {
  const isSupervisor = node.id === 'Supervisor';
  const config = (statusConfig as any)[node.status] || statusConfig.idle;
  const voiceActive = isVoiceMode && node.id === 'FreeAgent';
  const agentSteps = steps.filter((s) => s.agent === node.id || (node.id === 'Supervisor' && s.agent === 'Supervisor'));
  const toolsCount = agentSteps.filter((s) => s.type === 'tool' || s.type === 'command').length;
  const thoughtsCount = agentSteps.filter((s) => s.type === 'thought').length;

  return (
    <div onClick={onClick} className={`canvas-node relative absolute w-[280px] h-[144px] flex flex-col overflow-hidden rounded-[16px] border backdrop-blur-xl cursor-pointer transition-all duration-300 select-none ${config.border} ${config.bg} ${config.glow} ${isActive ? 'ring-2 ring-indigo-300/70 ring-offset-2 ring-offset-[#101115] scale-[1.035]' : 'hover:scale-[1.02] hover:-translate-y-0.5'} ${voiceActive ? 'voice-active' : ''}`} style={{ left: `${node.x - 140}px`, top: `${node.y - 72}px`, padding: '18px' }}>
      <div className="flex items-center justify-between flex-shrink-0">
        <div className="flex items-center gap-2 min-w-0">
          <div className="min-w-0">
            <span className="text-[14px] font-semibold text-zinc-100 truncate tracking-wide block leading-5">{node.label}</span>
            <span className="mt-1 block text-[8px] uppercase tracking-[0.16em] text-zinc-500">{isSupervisor ? 'Orchestrator' : 'Subagent'}</span>
          </div>
        </div>
        <div className="relative flex h-2 w-2 flex-shrink-0">
          {node.status === 'working' && <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-indigo-400 opacity-75" />}
          <span className={`relative inline-flex rounded-full h-2 w-2 ${config.dot}`} />
        </div>
      </div>
      <div className="w-full border-t border-white/[0.07] mt-4 mb-3 flex-shrink-0" />
      <div className="flex-1 min-h-0 flex items-stretch">
        <div className="flex-1 min-w-0 flex flex-col justify-center pr-3">
          <div className="flex items-center gap-2 mb-2"><span className={`px-1.5 py-0.5 text-[8px] font-bold font-mono uppercase rounded border ${config.badge}`}>{node.status}</span><span className="text-[8px] uppercase tracking-[0.14em] text-zinc-600">Preview</span></div>
          {node.lastStepText ? <div className="text-[9px] font-mono text-zinc-300 bg-white/[0.035] px-2 py-1 rounded-md border border-white/[0.06] truncate"><span className="text-zinc-500 mr-1">›</span>{node.lastStepText}</div> : <span className="text-[9px] text-zinc-500 italic pl-0.5">{isSupervisor ? 'Monitoring...' : 'Awaiting task...'}</span>}
        </div>
      </div>
      <div className="flex items-center gap-4 mt-3 pt-3 border-t border-white/[0.06] text-[8px] font-mono text-zinc-500"><span>TH <b className="text-zinc-200 font-semibold">{thoughtsCount}</b></span><span>TL <b className="text-zinc-200 font-semibold">{toolsCount}</b></span><span>TK <b className="text-zinc-200 font-semibold">{node.tokenUsage || 0}</b></span></div>
    </div>
  );
}
