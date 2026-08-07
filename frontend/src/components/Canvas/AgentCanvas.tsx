import { useEffect, useState } from 'react';
import { useCanvas } from '../../hooks/useCanvas';
import { useCanvasAgentState } from '../../hooks/useCanvasAgentState';
import CanvasWorkspace from './CanvasWorkspace';
import ChatView from '../Workspace/ChatView';
import type { AppState, ChatMessage, ModelOption } from '../../types';
import type { AgentTask } from '../../types';
import { deleteTask, fetchTasks, startTask, updateTaskStatus } from '../../services/api';

interface Props {
  messages: ChatMessage[];
  isRunning: boolean;
  workspaceName: string | null;
  chatTitle: string | null;
  selectedModel: string;
  models: ModelOption[];
  state: AppState | null;
  hasActiveChat: boolean;
  notification: string | null;
  onSend: (message: string, files?: File[]) => void;
  onRunTask: (task: AgentTask) => boolean;
  onCancel: () => void;
  onModelChange: (model: string) => void;
  onClearNotification: () => void;
  workspaceId: string | null;
  chatId: string | null;
  isVoiceMode: boolean;
  onToggleVoice: () => void;
  voiceInputText?: string;
  onOpenStats: () => void;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  totalCacheHitTokens: number;
  activeContextTokens: number;
}

export default function AgentCanvas(props: Props) {
  const { pan, setPan, zoom, setZoom, selectedNodeId, setSelectedNodeId, getInitialNodes, getInitialEdges } = useCanvas();
  const { steps, nodes, edges, activeAgent } = useCanvasAgentState({
    isRunning: props.isRunning,
    liveLog: props.state?.live_terminal_log,
    getInitialNodes,
    getInitialEdges,
    tokenUsage: props.state?.token_usage,
  });

  // Whether the user has manually dismissed the popover mid-run
  const [popoverDismissed, setPopoverDismissed] = useState(false);
  const [tasks, setTasks] = useState<AgentTask[]>([]);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [editingTask, setEditingTask] = useState<AgentTask | null>(null);

  useEffect(() => {
    if (!props.workspaceId || !props.chatId) { setTasks([]); return; }
    let cancelled = false;
    const refresh = () => fetchTasks(props.workspaceId!, props.chatId!).then((next) => { if (!cancelled) setTasks(next); }).catch(() => {});
    refresh();
    const timer = window.setInterval(refresh, 5000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [props.workspaceId, props.chatId]);

  const refreshTasks = () => {
    if (props.workspaceId && props.chatId) fetchTasks(props.workspaceId, props.chatId).then(setTasks).catch(() => {});
  };

  const handleStartTask = (task: AgentTask) => {
    if (!props.workspaceId || !props.chatId) return;
    startTask(props.workspaceId, props.chatId, task.id).then(async (started) => {
      setTasks((current) => current.map((item) => item.id === task.id ? started : item));
      const accepted = props.onRunTask(started);
      if (!accepted) {
        const reverted = await updateTaskStatus(props.workspaceId!, props.chatId!, task.id, 'ready').catch(() => task);
        setTasks((current) => current.map((item) => item.id === task.id ? reverted : item));
      }
    }).catch(() => {});
  };

  const handleDeleteTask = (task: AgentTask) => {
    if (props.workspaceId && props.chatId) deleteTask(props.workspaceId, props.chatId, task.id).then(refreshTasks).catch(() => {});
  };
  // Reset dismissal whenever a new agent becomes active
  const prevAgentRef = { current: activeAgent };
  if (prevAgentRef.current !== activeAgent) {
    setPopoverDismissed(false);
  }

  // The floating popover only appears when a non-Supervisor agent is actively working
  const isSubagentWorking = props.isRunning && !!activeAgent && activeAgent !== 'Supervisor';
  const activeAgentNode = isSubagentWorking && !popoverDismissed
    ? (nodes.find((n) => n.id === activeAgent) ?? null)
    : null;

  const activeAgentSteps = activeAgentNode
    ? steps.filter((s) => s.agent === activeAgent)
    : [];

  // Pull the task description from the delegation call for the active agent
  const activeAgentTask = steps
    .filter((s) => s.toolName === 'task' || s.toolName === 'start_async_task')
    .map((s) => {
      try {
        const parsed = JSON.parse(s.args || '{}');
        if (parsed.name === activeAgent) return parsed.task as string;
      } catch {}
      return null;
    })
    .filter(Boolean)[0] ?? '';

  return (
    <div className="relative flex flex-1 min-w-0 h-full overflow-hidden border-l border-zinc-800/80 bg-[#0a0a0c]">
      <CanvasWorkspace
        nodes={nodes}
        edges={edges}
        pan={pan}
        zoom={zoom}
        steps={steps}
        selectedId={selectedNodeId}
        setPan={setPan}
        setZoom={setZoom}
        onSelectNode={setSelectedNodeId}
        activeAgentNode={activeAgentNode}
        activeAgentSteps={activeAgentSteps}
        activeAgentTask={activeAgentTask}
        isRunning={props.isRunning}
        onClosePopover={() => setPopoverDismissed(true)}
        isVoiceMode={props.isVoiceMode}
        activeAgent={activeAgentNode}
        tasks={tasks}
        taskModalOpen={taskModalOpen}
        onOpenTaskModal={() => { setEditingTask(null); setTaskModalOpen(true); }}
        onCloseTaskModal={() => { setEditingTask(null); setTaskModalOpen(false); }}
        onTaskCreated={(task) => setTasks((current) => [task, ...current])}
        editingTask={editingTask}
        onEditTask={(task) => { setEditingTask(task); setTaskModalOpen(true); }}
        onTaskUpdated={(updated) => { setTasks((current) => current.map((item) => item.id === updated.id ? updated : item)); setEditingTask(null); }}
        onStartTask={handleStartTask}
        onDeleteTask={handleDeleteTask}
        onStartNow={(prompt, files) => props.onSend(prompt, files)}
        workspaceId={props.workspaceId}
        chatId={props.chatId}
      />

      <ChatView {...props} traceSteps={steps} />
    </div>
  );
}
