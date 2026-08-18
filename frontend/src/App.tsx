import { useState, useCallback, useEffect, useRef } from 'react';
import { X, FolderGit2 } from 'lucide-react';
import TitleBar from './components/Layout/TitleBar';
import Sidebar from './components/Layout/Sidebar';
import AgentCanvas from './components/Canvas/AgentCanvas';
import TokenStatsModal from './components/Layout/TokenStatsModal';
import QuestionModal from './components/Workspace/QuestionModal';
import FileExplorer from './components/Workspace/FileExplorer';
import { usePipeline } from './hooks/usePipeline';
import { fetchModels, addChatMessage, createChat, fetchChat, fetchWorkspaces, fetchChats, updateChatTitle, uploadChatAttachments, updateTaskStatus } from './services/api';
import type { ChatMessage, WorkspaceInfo, ChatData, ModelOption, AppState, ChatAttachment, AgentTask } from './types';
import { parseLogToSteps, filterOrchestratorSteps } from './utils/logParser';

const WELCOME_MESSAGES: ChatMessage[] = [
  {
    id: 'welcome-1',
    role: 'system',
    content: 'Connected to VinCode v3 — Supervisor + Developer agent online. Add a workspace folder to get started.',
    timestamp: new Date().toISOString(),
  },
];

export default function App() {
  const { run, cancel, state: pipeState, isRunning, error, streaming } = usePipeline();
  const state: AppState | null = pipeState;

  const [messages, setMessages] = useState<ChatMessage[]>(WELCOME_MESSAGES);
  const [voiceLiveRun, setVoiceLiveRun] = useState<{
    state: AppState | null;
    active: boolean;
    workspaceId: string;
    chatId: string;
    startedAt: number;
  } | null>(null);
  const [isPersisting, setIsPersisting] = useState(false);
  const [activeWorkspace, setActiveWorkspace] = useState<WorkspaceInfo | null>(null);
  const [activeChat, setActiveChat] = useState<ChatData | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [isTokenStatsOpen, setIsTokenStatsOpen] = useState(false);

  const [model, setModel] = useState('Automatic Fallback');
  const [temperature] = useState(0.7);
  const [models, setModels] = useState<ModelOption[]>([]);
  const [notification, setNotification] = useState<string | null>(null);
  const [sidebarRefreshTick, setSidebarRefreshTick] = useState(0);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [showFiles, setShowFiles] = useState(false);

  const wasRunning = useRef(false);
  const runStartTime = useRef<number | null>(null);
  const activeTaskRef = useRef<string | null>(null);

  // Init: fetch models + workspaces on mount
  useEffect(() => {
    fetchModels().then((m) => { if (m.length) setModels(m); }).catch(() => {});
    fetchWorkspaces().then(async (ws) => {
      if (ws.length > 0) {
        const general = ws.find((w) => w.isGeneral);
        const initialWs = general || ws[0];
        setActiveWorkspace(initialWs);
        try {
          const chats = await fetchChats(initialWs.id);
          if (chats.length > 0) {
            const chat = await fetchChat(initialWs.id, chats[0].id);
            setActiveChat(chat);
            setMessages(chat.messages.length > 0 ? chat.messages : [{
              id: `sys-${Date.now()}`,
              role: 'system',
              content: initialWs.isGeneral ? 'Connected. How can I help you?' : `Chat ready in ${initialWs.name}. What would you like to build?`,
              timestamp: new Date().toISOString(),
            }]);
          }
        } catch (err) {
          console.error('[App] Failed to auto-load chats:', err);
        }
      }
    }).catch(() => {});
  }, []);

  useEffect(() => {
    if (isRunning) {
      wasRunning.current = true;
    } else if (wasRunning.current) {
      wasRunning.current = false;
      // Backend/provider failures also end the stream, but they must not be
      // persisted as completed runs or empty assistant traces.
      if (state?.error) {
        return;
      }
      if (activeTaskRef.current && activeWorkspace && activeChat) {
        updateTaskStatus(activeWorkspace.id, activeChat.id, activeTaskRef.current, 'completed').catch(() => {});
        activeTaskRef.current = null;
      }
      if (activeWorkspace && activeChat) {
        console.log('[App] Pipeline run completed. Saving trace and final response...');
        setIsPersisting(true);
        
        const saveAndRefresh = async () => {
          try {
            const log = state?.live_terminal_log;
            if (log) {
              const { steps, assistantResponse } = parseLogToSteps(log);
              // General mode stores the final text in outputs rather than the
              // terminal log. Use it as the same unified trace response.
              const finalResponse = assistantResponse || state?.outputs?.agent_report || '';
              const duration = runStartTime.current
                ? Math.round((Date.now() - runStartTime.current) / 1000) + 's'
                : undefined;

              // Main chat only shows orchestrator-level actions (delegation,
              // supervisor thoughts, final summary). Subagent tool calls
              // belong in the canvas subagent drawer, not here.
              const chatSteps = filterOrchestratorSteps(steps);

              // Save a single unified agent message with trace metadata
              await addChatMessage(
                activeWorkspace.id,
                activeChat.id,
                'agent',
                finalResponse,
                { isTrace: true, steps: chatSteps, duration, artifacts: state?.artifacts, project_ref: state?.project_ref ?? undefined, usage: state?.token_usage ?? undefined }
              );
            }
            // Now re-fetch to sync messages and token stats
            const updatedChat = await fetchChat(activeWorkspace.id, activeChat.id);
            setActiveChat(updatedChat);
            setMessages(updatedChat.messages);
            // P9: a new project workspace was created this run → refresh sidebar
            if (state?.workspace_created) {
              setSidebarRefreshTick((t) => t + 1);
            }
          } catch (err) {
            console.error('[App] Failed to complete run persistence:', err);
          } finally {
            setIsPersisting(false);
          }
        };

        saveAndRefresh();
      }
    }
  }, [isRunning, activeWorkspace, activeChat, state?.live_terminal_log, state?.project_ref, state?.workspace_created, state?.token_usage]);

  useEffect(() => {
    if (!error) return;
    if (activeTaskRef.current && activeWorkspace && activeChat) {
      updateTaskStatus(activeWorkspace.id, activeChat.id, activeTaskRef.current, 'failed').catch(() => {});
      activeTaskRef.current = null;
    }
    const errMsg: ChatMessage = { id: `error-${Date.now()}`, role: 'error', content: error, timestamp: new Date().toISOString() };
    setMessages((prev) => [...prev, errMsg]);
    if (activeWorkspace && activeChat) {
      addChatMessage(activeWorkspace.id, activeChat.id, 'error', error).catch(() => {});
    }
  }, [error]);

  // ── Handlers ──

  const handleSend = useCallback(async (prompt: string, files: File[] = []) => {
    console.log('[App.handleSend] called with prompt=%o activeWorkspace=%o activeChat=%o', prompt, activeWorkspace, activeChat);
    setNotification(null);

    // Resolve workspace — auto-select General or first available
    let ws = activeWorkspace;

    if (!ws) {
      console.log('[App.handleSend] no active workspace, fetching...');
      try {
        const workspaces = await fetchWorkspaces();
        console.log('[App.handleSend] workspaces fetched:', workspaces);
        if (workspaces.length === 0) {
          setNotification('No chat available. Try restarting the server.');
          return;
        }
        // Prefer General workspace for non-folder chat
        const general = workspaces.find((w) => w.isGeneral);
        ws = general || workspaces[0];
        setActiveWorkspace(ws);
        console.log('[App.handleSend] auto-selected workspace:', ws);
      } catch (err) {
        console.error('[App.handleSend] fetchWorkspaces failed:', err);
        setNotification('Unable to reach backend. Is the API server running on port 8000?');
        return;
      }
    }

    // Resolve chat — auto-select first existing or create new one
    let chat = activeChat;

    if (!chat) {
      console.log('[App.handleSend] no active chat, resolving...');
      try {
        const chats = await fetchChats(ws.id);
        console.log('[App.handleSend] chats fetched:', chats);
        if (chats.length > 0) {
          chat = await fetchChat(ws.id, chats[0].id);
          setActiveChat(chat);
          setMessages(chat.messages.length > 0 ? chat.messages : [{
            id: `sys-${Date.now()}`,
            role: 'system',
            content: ws.isGeneral
              ? 'Connected. How can I help you?'
              : `Chat ready in ${ws.name}. What would you like to build?`,
            timestamp: new Date().toISOString(),
          }]);
          console.log('[App.handleSend] loaded existing chat:', chat);
        } else {
          const newChat = await createChat(ws.id, 'New Conversation');
          chat = await fetchChat(ws.id, newChat.id);
          setActiveChat(chat);
          setMessages([{
            id: `sys-${Date.now()}`,
            role: 'system',
            content: ws.isGeneral
              ? 'Connected. How can I help you?'
              : `Chat ready in ${ws.name}. What would you like to build?`,
            timestamp: new Date().toISOString(),
          }]);
          console.log('[App.handleSend] created new chat:', chat);
        }
      } catch (err) {
        console.error('[App.handleSend] chat resolution failed:', err);
        setNotification(`Failed to load conversation: ${err}`);
        return;
      }
    }

    // ── Send the message ──
    console.log('[App.handleSend] sending message, ws=%o chat=%o', ws, chat);
    let attachments: ChatAttachment[] = [];
    if (files.length) {
      try {
        attachments = await uploadChatAttachments(ws.id, chat.id, files);
      } catch (err) {
        console.error('[App] attachment upload failed:', err);
        setNotification('Your files could not be uploaded. Please try again.');
        return;
      }
    }
    const userMsg: ChatMessage = {
      id: `user-${Date.now()}`,
      role: 'user',
      content: prompt,
      timestamp: new Date().toISOString(),
      metadata: attachments.length ? { attachments } : undefined,
    };
    setMessages((prev) => [...prev, userMsg]);

    // Auto-name if the chat title is 'New Conversation' or empty
    if (chat && (chat.title === 'New Conversation' || !chat.title)) {
      let cleanTitle = prompt.trim();
      if (cleanTitle.startsWith('/')) {
        // Remove slash commands (e.g. "/goal build app" -> "build app")
        cleanTitle = cleanTitle.replace(/^\/\w+\s*/, '');
      }
      cleanTitle = cleanTitle.slice(0, 32).trim();
      if (prompt.length > 32) cleanTitle += '...';
      if (cleanTitle) {
        updateChatTitle(ws.id, chat.id, cleanTitle).catch(console.error);
        chat.title = cleanTitle;
        setActiveChat({ ...chat });
      }
    }

    console.log('[App.handleSend] calling run...');
    runStartTime.current = Date.now();
    run(prompt, model, temperature, ws.path, chat.id, attachments);
  }, [run, model, temperature, activeWorkspace, activeChat]);

  const handleRunTask = useCallback((task: AgentTask) => {
    if (!activeWorkspace || !activeChat || isRunning) return false;
    activeTaskRef.current = task.id;
    const userMsg: ChatMessage = {
      id: `task-${task.id}-${Date.now()}`,
      role: 'user',
      content: task.prompt,
      timestamp: new Date().toISOString(),
      metadata: { taskId: task.id, attachments: task.attachments },
    };
    setMessages((prev) => [...prev, userMsg]);
    runStartTime.current = Date.now();
    run(task.prompt, model, temperature, activeWorkspace.path, activeChat.id, task.attachments);
    return true;
  }, [activeWorkspace, activeChat, isRunning, run, model, temperature]);
  // ── Voice mode: driven by the floating overlay window (voice.html) ──
  const [isVoiceMode, setIsVoiceMode] = useState(false);
  const [voiceInputText] = useState('');

  const toggleVoice = useCallback(() => {
    (window as any).antigravity?.voiceToggle?.();
  }, []);

  // Re-fetch the active chat so conversations written by the voice overlay
  // (which runs outside this window) show up without switching chats.
  const refreshActiveChat = useCallback(() => {
    if (!activeWorkspace || !activeChat || isRunning || document.hidden) return;
    fetchChat(activeWorkspace.id, activeChat.id)
      .then((chat) => {
        if (chat?.messages?.length) {
          setMessages((prev) => {
            const lastPrev = prev[prev.length - 1];
            const lastNew = chat.messages[chat.messages.length - 1];
            return lastPrev?.id !== lastNew?.id ? chat.messages : prev;
          });
        }
      })
      .catch(() => {});
  }, [activeWorkspace, activeChat, isRunning]);

  // Keep the overlay in sync with the chat currently open here, so live mode
  // writes into the active chat (project or general) instead of a fixed one.
  const syncVoiceChat = useCallback(() => {
    if (activeWorkspace && activeChat) {
      (window as any).antigravity?.setVoiceChat?.({
        wsId: activeWorkspace.id,
        wsPath: activeWorkspace.path ?? '',
        chatId: activeChat.id,
      });
    }
  }, [activeWorkspace, activeChat]);

  useEffect(() => { syncVoiceChat(); }, [syncVoiceChat]);

  // Instant refresh when the overlay finishes a reply
  useEffect(() => {
    const ag = window.antigravity;
    if (!ag) return;
    const offSession = ag.onVoiceSessionEvent?.((ev) => {
      if (ev?.type && ev.type !== 'idle' && ev.type !== 'closed') {
        setIsVoiceMode(true);
      } else {
        setIsVoiceMode(false);
      }
      if (ev?.type === 'pipeline-start') {
        setVoiceLiveRun({
          state: null,
          active: true,
          workspaceId: ev.workspaceId || '',
          chatId: ev.chatId || '',
          startedAt: Date.now(),
        });
      } else if (ev?.type === 'pipeline-state') {
        setVoiceLiveRun((current) => ({
          state: ev.state as AppState,
          active: true,
          workspaceId: ev.workspaceId || current?.workspaceId || '',
          chatId: ev.chatId || current?.chatId || '',
          startedAt: current?.startedAt || Date.now(),
        }));
      } else if (ev?.type === 'pipeline-done') {
        setVoiceLiveRun((current) => current ? {
          ...current,
          state: (ev.state as AppState | null) || current.state,
          active: false,
        } : null);
      }
      if (ev?.type === 'opened') syncVoiceChat();
      if (ev?.type === 'reply') {
        refreshActiveChat();
        setVoiceLiveRun(null);
      }
    });
    const offClosed = ag.onVoiceWindowClosed?.(() => {
      setIsVoiceMode(false);
      setVoiceLiveRun(null);
    });
    return () => {
      offSession?.();
      offClosed?.();
    };
  }, [refreshActiveChat, syncVoiceChat]);

  // Lightweight polling fallback (covers runs finished while window was hidden)
  useEffect(() => {
    const id = setInterval(refreshActiveChat, 4000);
    return () => clearInterval(id);
  }, [refreshActiveChat]);

  // ── Keyboard shortcuts: Esc cancels a running pipeline, "?" opens help ──
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing = !!target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);
      if (e.key === 'Escape' && isRunning && !typing) {
        cancel();
      } else if (e.key === '?' && !isRunning && !typing) {
        e.preventDefault();
        setShowShortcuts((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [isRunning, cancel]);

  const handleSelectWorkspace = useCallback((ws: WorkspaceInfo) => {
    setActiveWorkspace(ws);
  }, []);

  const handleSelectChat = useCallback((ws: WorkspaceInfo, chat: ChatData) => {
    setActiveWorkspace(ws);
    setActiveChat(chat);


    // Load existing messages from chat
    if (chat.messages && chat.messages.length > 0) {
      setMessages(chat.messages);
    } else {
      setMessages([{
        id: `sys-${Date.now()}`,
        role: 'system',
        content: ws.isGeneral
          ? 'Connected. How can I help you?'
          : `Chat ready in ${ws.name}. What would you like to build?`,
        timestamp: new Date().toISOString(),
      }]);
    }

    // Sync project_path to the workspace folder
    fetch('/api/session/reset', { method: 'POST' }).catch(() => {});
  }, []);

  const handleChatTitleUpdated = useCallback((chatId: string, newTitle: string) => {
    if (activeChat && activeChat.id === chatId) {
      setActiveChat((c) => c ? { ...c, title: newTitle } : null);
    }
  }, [activeChat]);

  // P9: "Open in <project>" — jump from a routed reply card to the project's
  // own chat (where the full Dev reply lives).
  const handleOpenProject = useCallback((ref: { workspace_id: string; chat_id: string }) => {
    fetchWorkspaces().then(async (wsList) => {
      const ws = wsList.find((w) => w.id === ref.workspace_id);
      if (!ws) return;
      const chat = await fetchChat(ws.id, ref.chat_id);
      setActiveWorkspace(ws);
      setActiveChat(chat);
      setMessages(chat.messages.length > 0 ? chat.messages : [{
        id: `sys-${Date.now()}`,
        role: 'system',
        content: `Chat ready in ${ws.name}.`,
        timestamp: new Date().toISOString(),
      }]);
      fetch('/api/session/reset', { method: 'POST' }).catch(() => {});
    }).catch(() => {});
  }, []);

  const voiceTraceMatchesActiveChat = !!voiceLiveRun
    && (!voiceLiveRun.workspaceId || voiceLiveRun.workspaceId === activeWorkspace?.id)
    && (!voiceLiveRun.chatId || voiceLiveRun.chatId === activeChat?.id);
  const showVoiceTrace = !isRunning && voiceTraceMatchesActiveChat && !!voiceLiveRun?.state;
  const renderState = showVoiceTrace ? voiceLiveRun?.state : state;
  const renderIsRunning = isRunning || (showVoiceTrace && !!voiceLiveRun?.active);

  const displayMessages = [...messages];
  if ((isRunning || isPersisting) && state?.live_terminal_log) {
    const { steps, assistantResponse } = parseLogToSteps(state.live_terminal_log);
    const elapsed = runStartTime.current
      ? Math.round((Date.now() - runStartTime.current) / 1000) + 's'
      : 'active';

    // P10: live token streaming — show the generated text as it arrives, with
    // a caret; the final [ASSISTANT] line takes over when the run completes.
    const streamText = streaming.active && streaming.text ? streaming.text : '';
    const liveContent = streamText || assistantResponse || '';
    displayMessages.push({
      id: 'live-agent-run',
      role: 'agent',
      agentName: streaming.active && streaming.agent ? streaming.agent : 'Assistant',
      content: liveContent,
      timestamp: new Date().toISOString(),
      duration: isRunning ? 'active' : (elapsed === 'active' ? 'completed' : elapsed),
      metadata: {
        isTrace: true,
        steps: filterOrchestratorSteps(steps),
        duration: elapsed === 'active' ? undefined : elapsed,
        artifacts: state?.artifacts,
        project_ref: state?.project_ref ?? undefined,
        isStreaming: isRunning && streaming.active,
        streamingReasoning: streaming.active ? streaming.reasoning : '',
        streamRate: isRunning ? streaming.rate : 0,
      },
    });
  }
  if (showVoiceTrace && voiceLiveRun?.state) {
    const { steps, assistantResponse } = parseLogToSteps(voiceLiveRun.state.live_terminal_log || '');
    const elapsed = voiceLiveRun.active
      ? 'active'
      : Math.round((Date.now() - voiceLiveRun.startedAt) / 1000) + 's';
    displayMessages.push({
      id: 'voice-live-agent-run',
      role: 'agent',
      agentName: 'Assistant',
      content: assistantResponse || voiceLiveRun.state.outputs?.agent_report || '',
      timestamp: new Date().toISOString(),
      duration: elapsed,
      metadata: {
        isTrace: true,
        steps: filterOrchestratorSteps(steps),
        duration: elapsed === 'active' ? undefined : elapsed,
      },
    });
  }

  // TTS playback moved to the voice overlay (voice.html) — speaks replies
  // with natural neural voices via the backend /api/tts endpoint.

  const liveUsageState = isRunning ? state : (showVoiceTrace && voiceLiveRun?.active ? voiceLiveRun.state : null);
  const combinedTokenUsage = {
    total_input_tokens: (activeChat?.token_usage?.total_input_tokens || 0) + (liveUsageState?.token_usage?.total_input_tokens || 0),
    total_output_tokens: (activeChat?.token_usage?.total_output_tokens || 0) + (liveUsageState?.token_usage?.total_output_tokens || 0),
    total_cache_hit_tokens: (activeChat?.token_usage?.total_cache_hit_tokens || 0) + (liveUsageState?.token_usage?.total_cache_hit_tokens || 0),
    total_cache_miss_tokens: (activeChat?.token_usage?.total_cache_miss_tokens || 0) + (liveUsageState?.token_usage?.total_cache_miss_tokens || 0),
    total_cost: (activeChat?.token_usage?.total_cost || 0) + (liveUsageState?.token_usage?.total_cost || 0),
    calls: [
      ...(activeChat?.token_usage?.calls || []),
      ...(liveUsageState?.token_usage?.calls || [])
    ]
  };
  const lastCall = combinedTokenUsage.calls[combinedTokenUsage.calls.length - 1];
  const activeContextTokens = lastCall ? lastCall.input : 0;
  const lastModelName = lastCall ? lastCall.model : (model || 'Automatic Fallback');

  return (
    <div className="flex flex-col h-screen w-screen bg-[#0c0c0e] text-zinc-100 overflow-hidden font-sans">
      <TitleBar
      />
      <div className="app-shell flex flex-1 min-h-0" style={{ backgroundColor: '#0c0c0e' }}>
        <Sidebar
          activeWorkspaceId={activeWorkspace?.id ?? null}
          activeChatId={activeChat?.id ?? null}
          activeChatTitle={activeChat?.title}
          collapsed={sidebarCollapsed}
          onSelectWorkspace={handleSelectWorkspace}
          onSelectChat={handleSelectChat}
          onToggleCollapse={() => setSidebarCollapsed((v) => !v)}
          onChatTitleUpdated={handleChatTitleUpdated}
          refreshSignal={sidebarRefreshTick}
        />
        <AgentCanvas
          messages={displayMessages}
          isRunning={renderIsRunning}
          workspaceName={activeWorkspace?.name ?? null}
          chatTitle={activeChat?.title ?? null}
          selectedModel={model}
          models={models}
          state={renderState || null}
          hasActiveChat={!!activeWorkspace && !!activeChat}
          notification={notification}
          onSend={handleSend}
          onRunTask={handleRunTask}
          onCancel={cancel}
          onModelChange={setModel}
          onClearNotification={() => setNotification(null)}
          workspaceId={activeWorkspace?.id ?? null}
          chatId={activeChat?.id ?? null}
          isVoiceMode={isVoiceMode}
          onToggleVoice={toggleVoice}
          voiceInputText={voiceInputText}
          onOpenStats={() => setIsTokenStatsOpen(true)}
          onOpenProject={handleOpenProject}
          onRetry={handleSend}
          totalCost={combinedTokenUsage.total_cost}
          totalInput={combinedTokenUsage.total_input_tokens}
          totalOutput={combinedTokenUsage.total_output_tokens}
          totalCacheHitTokens={combinedTokenUsage.total_cache_hit_tokens}
          activeContextTokens={activeContextTokens}
        />
      </div>
      <TokenStatsModal
        isOpen={isTokenStatsOpen}
        onClose={() => setIsTokenStatsOpen(false)}
        tokenUsage={combinedTokenUsage}
        lastModelName={lastModelName}
      />
      {showShortcuts && (
        <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4" onClick={() => setShowShortcuts(false)}>          <div className="modal-panel-enter w-full max-w-sm rounded-2xl border border-zinc-800 bg-[#141417] shadow-2xl overflow-hidden" onClick={(e) => e.stopPropagation()}>
            <div className="flex items-center justify-between px-5 py-3.5 border-b border-zinc-800">
              <span className="text-[13px] font-bold text-zinc-100">Keyboard shortcuts</span>
              <button type="button" onClick={() => setShowShortcuts(false)} className="p-1 text-zinc-500 hover:text-zinc-300" aria-label="Close"><X size={15} /></button>
            </div>
            <div className="px-5 py-4 space-y-2.5 text-[12.5px]">
              {[
                ['Enter', 'Send message'],
                ['Shift + Enter', 'New line'],
                ['Ctrl + G', 'Toggle voice mode'],
                ['Esc', 'Stop the running agent'],
                ['?', 'Show this help'],
                ['Ctrl + Enter', 'Send (when typing)'],
              ].map(([key, desc]) => (
                <div key={key} className="flex items-center justify-between gap-4">
                  <span className="text-zinc-400">{desc}</span>
                  <kbd className="px-2 py-0.5 rounded bg-zinc-800 border border-zinc-700 text-[11px] font-mono text-zinc-300">{key}</kbd>
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
      <QuestionModal visible={isRunning || isPersisting} />

      {/* P10/Tier-3: file explorer toggle + panel */}
      <button
        type="button"
        onClick={() => setShowFiles((v) => !v)}
        className="fixed right-2 top-1/2 -translate-y-1/2 z-[85] flex flex-col items-center gap-1 px-1.5 py-3 rounded-xl bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shadow-xl"
        title={showFiles ? 'Close files' : 'Browse project files'}
        style={{ writingMode: 'vertical-rl' }}
      >
        <FolderGit2 size={15} className={showFiles ? 'text-indigo-400' : ''} />
        <span className="text-[9.5px] font-bold tracking-widest uppercase">Files</span>
      </button>
      {showFiles && <FileExplorer workspaceId={activeWorkspace?.id ?? null} onClose={() => setShowFiles(false)} />}
    </div>
  );
}
