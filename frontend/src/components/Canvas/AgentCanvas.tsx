import { lazy, Suspense, useEffect, useState } from 'react';
import { useCanvasAgentState } from '../../hooks/useCanvasAgentState';
import { useGroupProjects } from '../../house/useGroupProjects';
import GroupProjectModal from '../../house/GroupProjectModal';
import type { CanvasAddonId } from '../../canvas/canvasTypes';
import { kindForWorkspace, sendCanvasChat } from '../../canvas/canvasSend';
import SubagentSessionView from '../../house/SubagentSessionView';
import ChatView from '../Workspace/ChatView';
import type { AppState, ChatMessage, ChatSummary, ModelOption, WorkspaceInfo } from '../../types';
import { fetchChats } from '../../services/api';

const CanvasStage = lazy(() => import('../../canvas/CanvasStage'));

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
  onModeChange?: (mode: 'plan' | 'build') => void;
  onSend: (message: string, files?: File[]) => void;
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
  onOpenChat?: (workspaceId: string, chatId: string) => void;
  onOpenAddon?: (addonId: CanvasAddonId) => void;
  onRewind?: (message: ChatMessage) => void;
  draft?: string;
  onDraftConsumed?: () => void;
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

  const [subagentView, setSubagentView] = useState<{ childId: string; label: string; workspaceId?: string } | null>(null);
  const [openGroupId, setOpenGroupId] = useState<string | null>(null);
  const [workspaceChats, setWorkspaceChats] = useState<ChatSummary[]>([]);
  const { projects } = useGroupProjects(props.workspaceId || '');

  useEffect(() => {
    if (!props.workspaceId) { setWorkspaceChats([]); return; }
    let cancelled = false;
    const load = () => fetchChats(props.workspaceId!)
      .then((list) => { if (!cancelled) setWorkspaceChats(list); })
      .catch(() => {});
    load();
    const timer = window.setInterval(load, 15000);
    return () => { cancelled = true; window.clearInterval(timer); };
  }, [props.workspaceId, props.chatId]);

  const workspace: WorkspaceInfo | null = props.workspaceId ? {
    id: props.workspaceId,
    name: props.workspaceName || 'Workspace',
    path: props.state?.project_path || '',
    exists: true,
    chatCount: workspaceChats.length,
    addedAt: '',
    isGeneral: false,
    workspaceKind: 'project',
  } : null;

  const boardChats = (() => {
    if (!props.workspaceId) return {};
    const list = [...workspaceChats];
    if (props.chatId && !list.some((c) => c.id === props.chatId)) {
      const last = [...props.messages].reverse().find((m) => m.content?.trim());
      list.unshift({
        id: props.chatId,
        title: props.chatTitle || 'Current chat',
        messageCount: props.messages.length,
        model: props.selectedModel,
        createdAt: new Date().toISOString(),
        updatedAt: new Date().toISOString(),
        lastMessage: last?.content?.slice(0, 180),
        lastRole: last?.role,
      });
    }
    return { [props.workspaceId]: list };
  })();

  return (
    <div className="relative flex flex-1 min-w-0 h-full overflow-hidden border-l border-zinc-800/80 bg-[#0a0a0c]">
      <div className="absolute inset-0 overflow-y-auto house-thread px-3 pt-3">
        <Suspense fallback={null}>
          <CanvasStage
            key={props.workspaceId || 'main'}
            scope={props.workspaceId ? `ws:${props.workspaceId}` : 'main'}
            workspaces={workspace ? [workspace] : []}
            chatsByWs={boardChats}
            activeWorkspaceId={props.workspaceId}
            activeChatId={props.chatId}
            agents={agents}
            isRunning={props.isRunning}
            projects={projects}
            onOpenChat={(wsId, chatId) => props.onOpenChat?.(wsId, chatId)}
            onOpenGroup={setOpenGroupId}
            onOpenAddon={(_wsId, addonId) => props.onOpenAddon?.(addonId)}
            onSendToChat={async (_wsId, chatId, text) => {
              if (!workspace) throw new Error('Workspace not found.');
              if (chatId === props.chatId) {
                props.onSend(text);
                return;
              }
              await sendCanvasChat(workspace, chatId, text, kindForWorkspace(workspace));
              fetchChats(props.workspaceId!).then(setWorkspaceChats).catch(() => {});
            }}
          />
        </Suspense>
      </div>

      <div className="pointer-events-none absolute inset-x-0 bottom-2 z-[60]">
        <div className="pointer-events-auto">
          <ChatView
            {...props}
            variant="docked"
            traceSteps={steps}
            onOpenSubagentSession={(childId, label, childWorkspace) => setSubagentView({ childId, label, workspaceId: childWorkspace })}
          />
        </div>
      </div>

      {subagentView && (
        <div className="absolute top-0 right-0 h-full w-[440px] bg-[#0c0c0f] border-l border-zinc-800 shadow-2xl flex flex-col z-50">
          <div className="flex-1 min-h-0 px-3 pt-1 pb-3">
            <SubagentSessionView
              workspaceId={subagentView.workspaceId || props.workspaceId || ''}
              childId={subagentView.childId}
              label={subagentView.label}
              parentTitle={props.chatTitle ?? 'Main session'}
              onBack={() => setSubagentView(null)}
              onOpenSubagentSession={(childId, label, childWorkspace) => setSubagentView({ childId, label, workspaceId: childWorkspace })}
            />
          </div>
        </div>
      )}

      {openGroupId && (
        <GroupProjectModal
          key={openGroupId}
          projectId={openGroupId}
          onClose={() => setOpenGroupId(null)}
        />
      )}
    </div>
  );
}
