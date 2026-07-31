import { useRef, useEffect, useCallback } from 'react';
import type { MouseEvent, WheelEvent } from 'react';
import type { CanvasNode, CanvasEdge } from '../../types/canvas.types';
import type { TraceStep } from '../../utils/logParser';
import type { AgentTask } from '../../types';
import { Pencil, Play, Trash2 } from 'lucide-react';
import CanvasNodeComponent from './CanvasNode';
import SubagentPopover from './SubagentPopover';
import AddTaskModal from './AddTaskModal';
import { CANVAS_LIMITS, CANVAS_SIZE } from '../../hooks/useCanvas';

interface Props {
  nodes: CanvasNode[];
  edges: CanvasEdge[];
  pan: { x: number; y: number };
  zoom: number;
  selectedId: string | null;
  setPan: (pan: { x: number; y: number } | ((prev: { x: number; y: number }) => { x: number; y: number })) => void;
  setZoom: (zoom: number | ((prev: number) => number)) => void;
  onSelectNode: (id: string) => void;
  steps: any[];
  activeAgentNode: CanvasNode | null;
  activeAgentSteps: TraceStep[];
  activeAgentTask: string;
  activeAgent: CanvasNode | null;
  tasks: AgentTask[];
  taskModalOpen: boolean;
  onOpenTaskModal: () => void;
  onCloseTaskModal: () => void;
  onTaskCreated: (task: AgentTask) => void;
  editingTask: AgentTask | null;
  onEditTask: (task: AgentTask) => void;
  onTaskUpdated: (task: AgentTask) => void;
  onStartTask: (task: AgentTask) => void;
  onDeleteTask: (task: AgentTask) => void;
  onStartNow: (prompt: string, files: File[]) => Promise<void> | void;
  workspaceId: string | null;
  chatId: string | null;
  isRunning: boolean;
  isVoiceMode: boolean;
  onClosePopover: () => void;
}

export default function CanvasWorkspace({ nodes, edges, pan, zoom, selectedId, setPan, setZoom, onSelectNode, steps, activeAgentNode, activeAgentSteps, activeAgentTask, activeAgent, tasks, taskModalOpen, onOpenTaskModal, onCloseTaskModal, onTaskCreated, editingTask, onEditTask, onTaskUpdated, onStartTask, onDeleteTask, onStartNow, workspaceId, chatId, isRunning, isVoiceMode, onClosePopover }: Props) {
  const containerRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef({ isDragging: false, startX: 0, startY: 0, panX: 0, panY: 0, hasMoved: false });
  const wheelFrameRef = useRef<number | null>(null);
  const wheelEventRef = useRef<WheelEvent | null>(null);
  const motionTimeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setCanvasMotion = (active: boolean) => {
    const canvas = containerRef.current;
    if (!canvas) return;
    canvas.classList.toggle('canvas-is-moving', active);
    if (motionTimeoutRef.current) clearTimeout(motionTimeoutRef.current);
    if (active) motionTimeoutRef.current = setTimeout(() => canvas.classList.remove('canvas-is-moving'), 120);
  };

  const clampPan = useCallback((nextPan: { x: number; y: number }, nextZoom = zoom) => {
    const container = containerRef.current;
    if (!container) return nextPan;
    const rect = container.getBoundingClientRect();
    const gutter = 56;
    const minX = Math.min(gutter, rect.width - CANVAS_SIZE.width * nextZoom - gutter);
    const minY = Math.min(gutter, rect.height - CANVAS_SIZE.height * nextZoom - gutter);
    return { x: Math.max(minX, Math.min(gutter, nextPan.x)), y: Math.max(minY, Math.min(gutter, nextPan.y)) };
  }, [zoom]);

  const handleMouseDown = (e: MouseEvent) => {
    const target = e.target as HTMLElement;
    if (target.closest('button, input, textarea, select, a, [data-interactive="true"]')) return;
    dragRef.current = { isDragging: true, startX: e.clientX, startY: e.clientY, panX: pan.x, panY: pan.y, hasMoved: false };
  };

  useEffect(() => {
    const onMouseMove = (e: globalThis.MouseEvent) => {
      if (!dragRef.current.isDragging) return;
      const dx = e.clientX - dragRef.current.startX;
      const dy = e.clientY - dragRef.current.startY;
      if (Math.abs(dx) > 3 || Math.abs(dy) > 3) dragRef.current.hasMoved = true;
      setCanvasMotion(true);
      setPan(clampPan({ x: dragRef.current.panX + dx, y: dragRef.current.panY + dy }));
    };
    const onMouseUp = () => { dragRef.current.isDragging = false; setCanvasMotion(false); };
    const onClickCapture = (e: globalThis.MouseEvent) => {
      if (dragRef.current.hasMoved) {
        e.stopPropagation();
        e.preventDefault();
        dragRef.current.hasMoved = false;
      }
    };
    window.addEventListener('mousemove', onMouseMove);
    window.addEventListener('mouseup', onMouseUp);
    window.addEventListener('click', onClickCapture, true);
    return () => {
      window.removeEventListener('mousemove', onMouseMove);
      window.removeEventListener('mouseup', onMouseUp);
      window.removeEventListener('click', onClickCapture, true);
    };
  }, [setPan]);

  const handleWheel = (e: WheelEvent) => {
    e.preventDefault();
    wheelEventRef.current = e;
    setCanvasMotion(true);
    if (wheelFrameRef.current !== null) return;
    wheelFrameRef.current = requestAnimationFrame(() => {
      wheelFrameRef.current = null;
      const event = wheelEventRef.current;
      const container = containerRef.current;
      if (!event || !container) return;
      const rect = container.getBoundingClientRect();
      const mouseX = event.clientX - rect.left;
      const mouseY = event.clientY - rect.top;
      const factor = event.deltaY < 0 ? 1.05 : 0.95;
      const nextZoom = Math.max(CANVAS_LIMITS.minZoom, Math.min(CANVAS_LIMITS.maxZoom, zoom * factor));
      const ratio = nextZoom / zoom;
      const nextPanX = mouseX - (mouseX - pan.x) * ratio;
      const nextPanY = mouseY - (mouseY - pan.y) * ratio;
      setZoom(nextZoom);
      setPan(clampPan({ x: nextPanX, y: nextPanY }, nextZoom));
    });
  };

  useEffect(() => () => {
    if (wheelFrameRef.current !== null) cancelAnimationFrame(wheelFrameRef.current);
    if (motionTimeoutRef.current) clearTimeout(motionTimeoutRef.current);
  }, []);

  return (
    <div
      ref={containerRef}
      onMouseDown={handleMouseDown}
      onWheel={handleWheel}
      className="agent-canvas relative flex-1 h-full overflow-hidden cursor-grab active:cursor-grabbing select-none"
      style={{
        backgroundImage: 'radial-gradient(rgba(187,190,205,0.16) 1px, transparent 1px)',
        backgroundSize: '128px 128px',
        backgroundPosition: `${pan.x}px ${pan.y}px`,
      }}
    >
      <div
        className="absolute origin-top-left"
        style={{ transform: `translate3d(${pan.x}px, ${pan.y}px, 0) scale(${zoom})`, width: `${CANVAS_SIZE.width}px`, height: `${CANVAS_SIZE.height}px`, willChange: 'transform' }}
      >
        <svg className="absolute inset-0 w-full h-full pointer-events-none overflow-visible">
          <style>{`
            @keyframes vibrate {
              0% { transform: translate(0, 0); }
              25% { transform: translate(-1.5px, 0.5px); }
              50% { transform: translate(1px, -1px); }
              75% { transform: translate(-0.5px, 1.5px); }
              100% { transform: translate(0, 0); }
            }
            @keyframes voicePulse {
              0%, 100% { box-shadow: 0 0 4px rgba(59, 130, 246, 0.3); }
              50% { box-shadow: 0 0 16px rgba(59, 130, 246, 0.7); }
            }
            @keyframes flowAnimation {
              from {
                stroke-dashoffset: 16;
              }
              to {
                stroke-dashoffset: 0;
              }
            }
          `}</style>
          {edges.map((edge) => {
            const fromNode = nodes.find((n) => n.id === edge.from);
            const toNode = nodes.find((n) => n.id === edge.to);
            if (!fromNode || !toNode) return null;
            
            const isFlowEdge = edge.id.startsWith('edge-flow-');
            const strokeColor = edge.active 
              ? (isFlowEdge ? 'stroke-amber-500/30' : 'stroke-indigo-500/30') 
              : 'stroke-zinc-900/60';
            const activeStrokeColor = isFlowEdge ? 'stroke-amber-400' : 'stroke-indigo-400';
            const glowFilter = isFlowEdge 
              ? 'drop-shadow(0 0 3px rgba(245, 158, 11, 0.5))' 
              : 'drop-shadow(0 0 3px rgba(99, 102, 241, 0.5))';

            return (
              <g key={edge.id}>
                {/* Background line */}
                <line
                  x1={fromNode.x}
                  y1={fromNode.y}
                  x2={toNode.x}
                  y2={toNode.y}
                  className={`stroke-[1.5px] transition-colors duration-500 ${strokeColor}`}
                />
                {/* Flowing animated overlay */}
                {edge.active && (
                  <line
                    x1={fromNode.x}
                    y1={fromNode.y}
                    x2={toNode.x}
                    y2={toNode.y}
                    className={`stroke-[2px] ${activeStrokeColor}`}
                    style={{
                      strokeDasharray: '6 6',
                      animation: 'flowAnimation 1.2s linear infinite',
                      filter: glowFilter,
                    }}
                  />
                )}
              </g>
            );
          })}
        </svg>
        {nodes.map((node) => (
          <CanvasNodeComponent
            key={node.id}
            node={node}
            steps={steps}
            isActive={selectedId === node.id}
            onClick={() => onSelectNode(node.id)}
            isVoiceMode={isVoiceMode}
          />
        ))}

        {/* Floating subagent popover — only shown while that agent is working */}
        {activeAgentNode && (
          <SubagentPopover
            node={activeAgentNode}
            steps={activeAgentSteps}
            taskDesc={activeAgentTask}
            isRunning={isRunning}
            onClose={onClosePopover}
          />
        )}
      </div>
      <div className="canvas-preview-cluster"><div className="canvas-preview-status"><span className={`canvas-preview-pulse ${activeAgent ? 'is-working' : ''}`} /><div className="canvas-preview-copy"><span className="canvas-preview-eyebrow">LIVE PREVIEW</span><span className="canvas-preview-title">{activeAgent ? activeAgent.label : 'Supervisor standing by'}</span></div><span className={`canvas-preview-state ${activeAgent ? 'is-working' : ''}`}>{activeAgent ? 'WORKING' : 'READY'}</span></div><button type="button" className="canvas-add-task-button" onMouseDown={(event) => event.stopPropagation()} onClick={onOpenTaskModal} aria-label="Add a task" title="Add a task">+</button></div>
      {tasks.length > 0 && <div className="canvas-task-strip">{tasks.slice(0, 5).map((task) => <div className="canvas-task-chip" key={task.id}><span className={`canvas-task-dot is-${task.status}`} /><span className="canvas-task-chip-copy"><b>{task.prompt || 'Attachment task'}</b><small>{task.status === 'scheduled' && task.schedule?.nextRunAt ? `Next · ${new Date(task.schedule.nextRunAt).toLocaleString()}` : task.status === 'ready' ? 'Saved · ready to run' : task.status === 'completed' ? 'Completed · ready to run again' : task.status === 'running' ? (isRunning ? 'Running now' : 'Run interrupted · ready again') : task.status}</small></span>{task.status === 'running' && isRunning ? <span className="canvas-task-running-label">Running</span> : <><button type="button" data-interactive="true" className="canvas-task-action" onClick={() => onStartTask(task)} aria-label="Run task" title="Run task"><Play size={11} /></button><button type="button" data-interactive="true" className="canvas-task-action" onClick={() => onEditTask(task)} aria-label="Edit task" title="Edit task"><Pencil size={11} /></button><button type="button" data-interactive="true" className="canvas-task-action canvas-task-delete" onClick={() => onDeleteTask(task)} aria-label="Delete task" title="Delete task"><Trash2 size={11} /></button></>}</div>)}</div>}
      <div className="canvas-hud canvas-hud-bottom"><span>{Math.round(zoom * 100)}%</span></div>
      {taskModalOpen && <AddTaskModal workspaceId={workspaceId} chatId={chatId} isRunning={isRunning} task={editingTask} onClose={onCloseTaskModal} onCreated={onTaskCreated} onUpdated={onTaskUpdated} onStartNow={onStartNow} />}
    </div>
  );
}
