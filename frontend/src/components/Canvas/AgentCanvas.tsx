import { useEffect, useRef, useState } from 'react';
import { useCanvasAgentState } from '../../hooks/useCanvasAgentState';
import CanvasWorkspace from './CanvasWorkspace';
import SubagentSessionView from '../../house/SubagentSessionView';
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
  chatMode?: 'plan' | 'build';
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
  onOpenProject?: (ref: { workspace_id: string; chat_id: string }) => void;
  onRetry?: (prompt: string) => void;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  totalCacheHitTokens: number;
  activeContextTokens: number;
}

export default function AgentCanvas(props: Props) {
  const { steps, agents } = useCanvasAgentState({
    isRunning: props.isRunning,
    liveLog: props.state?.live_terminal_log,
    tokenUsage: props.state?.token_usage,
  });

  const [tasks, setTasks] = useState<AgentTask[]>([]);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [editingTask, setEditingTask] = useState<AgentTask | null>(null);
  const [selectedAgent, setSelectedAgent] = useState<string | null>(null);
  const [subagentView, setSubagentView] = useState<{ childId: string; label: string } | null>(null);

  // Track when the current run started so running cards show a live elapsed clock.
  const [runStartedAt, setRunStartedAt] = useState<number | null>(null);
  const prevRunningRef = useRef(false);
  useEffect(() => {
    if (props.isRunning && !prevRunningRef.current) setRunStartedAt(Date.now());
    prevRunningRef.current = props.isRunning;
  }, [props.isRunning]);

  const chatKey = props.workspaceId && props.chatId ? `${props.workspaceId}:${props.chatId}` : '';
  const [loadedChatKey, setLoadedChatKey] = useState('');
  useEffect(() => {
    if (!chatKey) return;
    let cancelled = false;
    const refresh = () => fetchTasks(props.workspaceId!, props.chatId!)
      .then((next) => { if (!cancelled) { setTasks(next); setLoadedChatKey(chatKey); } })
      .catch(() => {});
    refresh();
    const timer = window.setInterval(refresh, 5000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [chatKey]);

  const visibleTasks = chatKey && loadedChatKey === chatKey ? tasks : [];

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

  const handleStatusChange = (task: AgentTask, status: AgentTask['status']) => {
    if (!props.workspaceId || !props.chatId) return;
    const prev = task.status;
    setTasks((current) => current.map((item) => item.id === task.id ? { ...item, status } : item));
    updateTaskStatus(props.workspaceId, props.chatId, task.id, status)
      .then((updated) => setTasks((current) => current.map((item) => item.id === updated.id ? updated : item)))
      .catch(() => setTasks((current) => current.map((item) => item.id === task.id ? { ...item, status: prev } : item)));
  };

  return (
    <div className="relative flex flex-1 min-w-0 h-full overflow-hidden border-l border-zinc-800/80 bg-[#0a0a0c]">
      <CanvasWorkspace
        tasks={visibleTasks}
        agents={agents}
        isRunning={props.isRunning}
        runStartedAt={runStartedAt}
        selectedAgent={selectedAgent}
        onSelectAgent={setSelectedAgent}
        onStartTask={handleStartTask}
        onEditTask={(task) => { setEditingTask(task); setTaskModalOpen(true); }}
        onDeleteTask={handleDeleteTask}
        onStatusChange={handleStatusChange}
        onOpenTaskModal={() => { setEditingTask(null); setTaskModalOpen(true); }}
        taskModalOpen={taskModalOpen}
        editingTask={editingTask}
        onCloseTaskModal={() => { setEditingTask(null); setTaskModalOpen(false); }}
        onTaskCreated={(task) => setTasks((current) => [task, ...current])}
        onTaskUpdated={(updated) => { setTasks((current) => current.map((item) => item.id === updated.id ? updated : item)); setEditingTask(null); }}
        onStartNow={(prompt, files) => props.onSend(prompt, files)}
        workspaceId={props.workspaceId}
        chatId={props.chatId}
      />

      <ChatView {...props} traceSteps={steps} onOpenSubagentSession={(childId, label) => setSubagentView({ childId, label })} />

      {/* Subagent session drawer — child-keyed, read-only transcript */}
      {subagentView && (
        <div className="absolute top-0 right-0 h-full w-[440px] bg-[#0c0c0f] border-l border-zinc-800 shadow-2xl flex flex-col z-50">
          <div className="flex-1 min-h-0 px-3 pt-1 pb-3">
            <SubagentSessionView
              workspaceId={props.workspaceId ?? ''}
              childId={subagentView.childId}
              label={subagentView.label}
              parentTitle={props.chatTitle ?? 'Main session'}
              onBack={() => setSubagentView(null)}
            />
          </div>
        </div>
      )}
    </div>
  );
}