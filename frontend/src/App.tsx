import { useState, useCallback, useEffect, useRef } from 'react';
import { Zap } from 'lucide-react';
import TitleBar from './components/Layout/TitleBar';
import Sidebar from './components/Layout/Sidebar';
import AgentCanvas from './components/Canvas/AgentCanvas';
import TokenStatsModal from './components/Layout/TokenStatsModal';
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
  const { run, cancel, state: pipeState, isRunning, error } = usePipeline();
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
  const [isInitialLoading, setIsInitialLoading] = useState(true);

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
    }).catch(() => {})
      .finally(() => {
        setTimeout(() => setIsInitialLoading(false), 600);
      });
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
                { isTrace: true, steps: chatSteps, duration, artifacts: state?.artifacts }
              );
            }
            // Now re-fetch to sync messages and token stats
            const updatedChat = await fetchChat(activeWorkspace.id, activeChat.id);
            setActiveChat(updatedChat);
            setMessages(updatedChat.messages);
          } catch (err) {
            console.error('[App] Failed to complete run persistence:', err);
          } finally {
            setIsPersisting(false);
          }
        };

        saveAndRefresh();
      }
    }
  }, [isRunning, activeWorkspace, activeChat, state?.live_terminal_log]);

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
    const ag = (window as any).antigravity;
    if (!ag) return;
    ag.onVoiceSessionEvent?.((ev: any) => {
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
    ag.onVoiceWindowClosed?.(() => {
      setIsVoiceMode(false);
      setVoiceLiveRun(null);
    });
  }, [refreshActiveChat, syncVoiceChat]);

  // Lightweight polling fallback (covers runs finished while window was hidden)
  useEffect(() => {
    const id = setInterval(refreshActiveChat, 4000);
    return () => clearInterval(id);
  }, [refreshActiveChat]);

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

    displayMessages.push({
      id: 'live-agent-run',
      role: 'agent',
      agentName: 'Assistant',
      content: assistantResponse || '',
      timestamp: new Date().toISOString(),
      duration: isRunning ? 'active' : (elapsed === 'active' ? 'completed' : elapsed),
      metadata: { isTrace: true, steps: filterOrchestratorSteps(steps), duration: elapsed === 'active' ? undefined : elapsed, artifacts: state?.artifacts },
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

  if (isInitialLoading) {
    return (
      <div className="flex flex-col items-center justify-center h-screen w-screen bg-[#0c0c0e] text-zinc-300 font-sans select-none">
        <div className="flex flex-col items-center gap-5 max-w-sm w-full px-8 py-10 rounded-2xl border border-zinc-900 bg-zinc-950/60 shadow-2xl relative overflow-hidden backdrop-blur-xl">
          {/* Subtle top ambient glow */}
          <div className="absolute -top-12 left-1/2 -translate-x-1/2 w-32 h-32 bg-indigo-500/10 rounded-full blur-2xl pointer-events-none" />
          
          <div className="relative flex items-center justify-center w-16 h-16 rounded-2xl bg-zinc-900 border border-zinc-800 shadow-2xl">
            <Zap size={28} className="text-indigo-400 animate-pulse" />
            <div className="absolute inset-0 rounded-2xl border border-indigo-500/20 animate-ping opacity-25" style={{ animationDuration: '2s' }} />
          </div>
          
          <div className="flex flex-col items-center gap-1.5 text-center">
            <h1 className="text-[15px] font-bold text-zinc-100 tracking-wide">Initializing VinCode</h1>
            <p className="text-[12px] text-zinc-500 font-mono">Loading workspaces and pipelines...</p>
          </div>

          {/* Simple animated loading bar */}
          <div className="w-full h-1 bg-zinc-900 rounded-full overflow-hidden relative">
            <div className="absolute top-0 bottom-0 left-0 w-1/2 h-full bg-gradient-to-r from-indigo-500 to-purple-500 rounded-full animate-pulse-slow" style={{
              animation: 'loadingBar 1.5s infinite ease-in-out'
            }} />
          </div>
        </div>
        
        {/* CSS Keyframes injected inline */}
        <style dangerouslySetInnerHTML={{__html: `
          @keyframes loadingBar {
            0% { left: -50%; }
            50% { left: 100%; }
            100% { left: 100%; }
          }
        `}} />
      </div>
    );
  }

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
    </div>
  );
}
