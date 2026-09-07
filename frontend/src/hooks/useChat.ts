/**
 * Unified chat hook — Lane B merge of house/useHouseChat.ts + house/useSessionRunner.ts
 * Generic for any workspace kind (general/house/personal/project) — scales with routing/workspaceKind enum.
 * New code: `import { useChat } from '../hooks/useChat'`
 * Legacy: `house/useHouseChat.ts` re-exports this.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChatMessage, WorkspaceInfo, AppState, ChatSummary, ChatAttachment, TokenUsage, AgentTask, StreamDelta } from '../types';
import { fetchWorkspaces, createChat, fetchChats, fetchChat, deleteChat, updateChatTitle, uploadChatAttachments, fetchTasks, startTask, deleteTask } from '../services/api';
import { initHouseWorkspace, startHousePipeline, finishHouseMessage, resolvePinnedChat } from '../house/houseChatService';
import { getAssistantName } from '../house/assistant';
import { parseLogToSteps } from '../utils/logParser';

export interface UseChatProps {
  onActive?: (payload: { wsId: string; chatId: string }) => void;
  pinnedChatTitle?: string;
  workspaceKind?: 'general' | 'house' | 'personal' | 'project';
}

export function useChat({ onActive, pinnedChatTitle, workspaceKind: _workspaceKind = 'general' }: UseChatProps = {}) {
  const workspaceKind = _workspaceKind === 'house' || _workspaceKind === 'personal' ? _workspaceKind : 'general';
  const [generalWs, setGeneralWs] = useState<WorkspaceInfo | null>(null);
  const [chats, setChats] = useState<ChatSummary[]>([]);
  const [activeChatId, setActiveChatId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [tokenUsage, setTokenUsage] = useState<TokenUsage | null>(null);
  const [tasks, setTasks] = useState<AgentTask[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [liveState, setLiveState] = useState<AppState | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [notification, setNotification] = useState<string | null>(null);
  const [isVoiceMode, setIsVoiceMode] = useState(false);
  // Phase 8-V2 fix: voice runs execute in the overlay — track their live
  // activity here so the house chat streams the reply in real time.
  const [voiceActive, setVoiceActive] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  const threadRef = useRef<HTMLDivElement | null>(null);
  const liveStateRef = useRef<AppState | null>(null);
  const startedAtRef = useRef<number | null>(null);
  const [streaming, setStreaming] = useState<{ text: string; reasoning: string; agent: string; active: boolean; rate: number }>({ text: '', reasoning: '', agent: '', active: false, rate: 0 });
  const streamRef = useRef({ text: '', reasoning: '', agent: '' });
  const rateRef = useRef({ lastTs: 0, lastChars: 0, ema: 0 });

  const applyDelta = useCallback((delta: StreamDelta) => {
    const now = performance.now();
    if (delta.kind === 'reasoning') {
      streamRef.current.reasoning += delta.text;
    } else if (delta.kind === 'text') {
      const chars = delta.text.length;
      const r = rateRef.current;
      if (r.lastTs > 0) {
        const dt = Math.max(16, now - r.lastTs) / 1000;
        const rate = (chars / 4) / dt;
        r.ema = r.ema === 0 ? rate : r.ema * 0.8 + rate * 0.2;
      }
      r.lastTs = now;
      r.lastChars = chars;
      streamRef.current.text += delta.text;
    } else if (delta.kind === 'end') {
      rateRef.current.ema = 0;
    }
    if (delta.agent) streamRef.current.agent = delta.agent;
    setStreaming({
      text: streamRef.current.text,
      reasoning: streamRef.current.reasoning,
      agent: streamRef.current.agent,
      active: true,
      rate: Math.round(rateRef.current.ema),
    });
  }, []);

  const resetStream = useCallback(() => {
    streamRef.current = { text: '', reasoning: '', agent: '' };
    rateRef.current = { lastTs: 0, lastChars: 0, ema: 0 };
    setStreaming({ text: '', reasoning: '', agent: '', active: false, rate: 0 });
  }, []);

  const refreshMessages = useCallback(async () => {
    if (!generalWs || !activeChatId || isRunning) return;
    try {
      const chat = await fetchChat(generalWs.id, activeChatId);
      if (chat?.messages?.length) setMessages(chat.messages);
      setTokenUsage(chat.token_usage ?? null);
    } catch { /* keep current view */ }
  }, [generalWs, activeChatId, isRunning]);

  const refreshList = useCallback(async (ws: WorkspaceInfo) => {
    setChats(await fetchChats(ws.id).catch(() => []));
  }, []);

  const refreshTasks = useCallback(async () => {
    if (!generalWs || !activeChatId) { setTasks([]); return; }
    setTasks(await fetchTasks(generalWs.id, activeChatId).catch(() => []));
  }, [generalWs, activeChatId]);

  useEffect(() => {
    let cancelled = false;
    initHouseWorkspace(pinnedChatTitle, workspaceKind).then(({ ws, chats: c, initialMessages, initialChatId }) => {
      if (cancelled) return;
      setGeneralWs(ws);
      setChats(c);
      setActiveChatId(initialChatId);
      setMessages(initialMessages);
      if (ws && initialChatId) {
        fetchChat(ws.id, initialChatId)
          .then((chat) => { if (!cancelled) setTokenUsage(chat.token_usage ?? null); })
          .catch(() => {});
      }
    }).catch(() => { if (!cancelled) setNotification('Backend unreachable.'); })
      .finally(() => { if (!cancelled) setIsLoading(false); });
    return () => { cancelled = true; };
  }, [pinnedChatTitle]);

  useEffect(() => {
    const el = threadRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, liveState, isRunning]);

  useEffect(() => {
    const id = window.setTimeout(() => { refreshTasks(); }, 0);
    return () => window.clearTimeout(id);
  }, [refreshTasks]);

  const syncVoiceChat = useCallback(() => {
    if (generalWs && activeChatId) {
      window.antigravity?.setVoiceChat?.({ wsId: generalWs.id, wsPath: generalWs.path ?? '', chatId: activeChatId });
    }
  }, [generalWs, activeChatId]);

  useEffect(() => { syncVoiceChat(); }, [syncVoiceChat]);

  useEffect(() => {
    const ag = window.antigravity;
    if (!ag) return;
    const offSession = ag.onVoiceSessionEvent?.((ev) => {
      if (ev?.type && ev.type !== 'idle' && ev.type !== 'closed') {
        setIsVoiceMode(true);
      } else {
        setIsVoiceMode(false);
      }
      if (ev?.type === 'opened') syncVoiceChat();
      if (ev?.type === 'reply') {
        setIsVoiceMode(false);
        setVoiceActive(false);
        streamRef.current = { text: '', reasoning: '', agent: '' };
        setStreaming((s) => ({ ...s, active: false, text: '', reasoning: '' }));
        refreshMessages();
      }
      // Phase 8-V2 fix: a voice run started — begin the live bubble.
      if (ev?.type === 'pipeline-start') {
        setVoiceActive(true);
        streamRef.current = { text: '', reasoning: '', agent: '' };
        rateRef.current = { lastTs: 0, lastChars: 0, ema: 0 };
        setStreaming({ text: '', reasoning: '', agent: '', active: false, rate: 0 });
      }
      // Phase 8-V2: live tokens from a voice run stream into the house chat —
      // merge them into the same streaming bubble as local runs.
      if (ev?.type === 'delta' && ev.text) {
        const kind = ev.kind === 'reasoning' ? 'reasoning' : 'text';
        if (kind === 'reasoning') streamRef.current.reasoning += String(ev.text);
        else streamRef.current.text += String(ev.text);
        if (ev.agent) streamRef.current.agent = String(ev.agent);
        setStreaming((s) => ({
          text: streamRef.current.text,
          reasoning: streamRef.current.reasoning,
          agent: streamRef.current.agent,
          active: true,
          rate: s.rate,
        }));
      }
      // Live input streaming: words appear in chat as you speak (ChatGPT live)
      if (ev?.type === 'transcript' && ev.text) {
        const t = String(ev.text);
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last && last.role === 'user' && last.id === 'voice-live-input') {
            return [...prev.slice(0, -1), { ...last, content: t }];
          }
          return [...prev, { id: 'voice-live-input', role: 'user', content: t, timestamp: new Date().toISOString() }];
        });
      }
      // When run starts, replace live input bubble with persisted user message (avoid duplicate)
      if (ev?.type === 'pipeline-start' && (ev as any).prompt) {
        setMessages((prev) => {
          const hasLive = prev.some(m => m.id === 'voice-live-input');
          if (!hasLive) return prev;
          // Keep live bubble until run completes — will be replaced on refresh
          return prev;
        });
      }
    });
    const offClosed = ag.onVoiceWindowClosed?.(() => { setIsVoiceMode(false); });
    return () => {
      offSession?.();
      offClosed?.();
    };
  }, [syncVoiceChat, refreshMessages]);

  const toggleVoice = useCallback(() => {
    window.antigravity?.voiceToggle?.();
  }, []);

  const handleNewChat = useCallback(async () => {
    if (!generalWs || isRunning) return;
    try {
      const chat = await createChat(generalWs.id, 'New Conversation');
      setActiveChatId(chat.id);
      setMessages([]);
      setTokenUsage(null);
      setChats(await fetchChats(generalWs.id));
    } catch { setNotification('Could not create chat.'); }
  }, [generalWs, isRunning]);

  const handleSelectChat = useCallback(async (chatId: string, opts?: { silent?: boolean }) => {
    if (!generalWs || isRunning) return;
    try {
      const chat = await fetchChat(generalWs.id, chatId);
      setActiveChatId(chat.id);
      setMessages(chat.messages || []);
      setTokenUsage(chat.token_usage ?? null);
    } catch {
      if (!opts?.silent) setNotification('Could not load chat.');
    }
  }, [generalWs, isRunning]);

  const handleRenameChat = useCallback(async (chatId: string, title: string) => {
    if (!generalWs || isRunning || !title.trim()) return;
    try {
      await updateChatTitle(generalWs.id, chatId, title.trim());
      setChats(await fetchChats(generalWs.id));
    } catch { setNotification('Could not rename chat.'); }
  }, [generalWs, isRunning]);

  const handleDeleteChat = useCallback(async (chatId: string) => {
    if (!generalWs || isRunning) return;
    try {
      await deleteChat(generalWs.id, chatId);
      const list = await fetchChats(generalWs.id);
      setChats(list);
      if (activeChatId === chatId) {
        if (list.length > 0) {
          const chat = await fetchChat(generalWs.id, list[0].id);
          setActiveChatId(chat.id);
          setMessages(chat.messages || []);
          setTokenUsage(chat.token_usage ?? null);
        } else {
          setActiveChatId(null);
          setMessages([]);
          setTokenUsage(null);
        }
      }
    } catch { setNotification('Could not delete chat.'); }
  }, [generalWs, isRunning, activeChatId]);

  const handleCancel = useCallback(() => {
    abortRef.current?.abort();
    setIsRunning(false);
    setLiveState(null);
  }, []);

  const handleSend = useCallback(async (prompt: string, files: File[]) => {
    if (isRunning) return;
    if (!prompt.trim() && !files.length) return;
    setNotification(null);

    let ws = generalWs;
    if (!ws) {
      try {
        const wsList = await fetchWorkspaces();
        ws = wsList.find((w) => w.isGeneral) || wsList[0] || null;
        setGeneralWs(ws);
      } catch {
        setNotification('VIN could not reach the backend.');
        return;
      }
      if (!ws) { setNotification('No general workspace available yet.'); return; }
    }

    let chatId = activeChatId;
    if (!chatId && pinnedChatTitle) {
      try {
        const pinned = await resolvePinnedChat(ws, pinnedChatTitle);
        if (!pinned) throw new Error('unavailable');
        chatId = pinned.id;
        setActiveChatId(chatId);
        await refreshList(ws);
      } catch {
        setNotification('Could not open the House conversation.');
        return;
      }
    } else if (!chatId) {
      try {
        const trimmed = prompt.trim();
        const title = (trimmed.slice(0, 32) || 'New Conversation') + (trimmed.length > 32 ? '...' : '');
        const chat = await createChat(ws.id, title);
        chatId = chat.id;
        setActiveChatId(chatId);
        await refreshList(ws);
      } catch {
        setNotification('Could not create a conversation.');
        return;
      }
    }

    let attachments: ChatAttachment[] = [];
    if (files.length) {
      try {
        attachments = await uploadChatAttachments(ws.id, chatId, files);
      } catch {
        setNotification('Your files could not be uploaded.');
        return;
      }
    }

    const userMsg: ChatMessage = {
      id: `usr-${Date.now()}`,
      role: 'user',
      content: prompt.trim() || `Attached ${attachments.length} file${attachments.length > 1 ? 's' : ''}`,
      timestamp: new Date().toISOString(),
      metadata: attachments.length ? { attachments } : undefined,
    };
    setMessages((prev) => [...prev, userMsg]);
    setIsRunning(true);
    setLiveState(null);
    resetStream();
    startedAtRef.current = Date.now();

    abortRef.current = startHousePipeline(
      prompt.trim(),
      ws,
      chatId,
      attachments,
      (st) => { liveStateRef.current = st; setLiveState(st); },
      async () => {
        try {
          const duration = startedAtRef.current ? Math.round((Date.now() - startedAtRef.current) / 1000) : undefined;
          const updated = await finishHouseMessage(ws, chatId!, liveStateRef.current, duration);
          setMessages(updated);
          setChats(await fetchChats(ws.id));
          fetchChat(ws.id, chatId!).then((chat) => setTokenUsage(chat.token_usage ?? null)).catch(() => {});
          await refreshTasks();
        } catch { /* keep the live message visible if persistence fails */ }
        resetStream();
        setIsRunning(false);
        setLiveState(null);
        onActive?.({ wsId: ws.id, chatId: chatId! });
      },
      (err) => {
        setNotification(typeof err === 'string' ? err : 'Error occurred.');
        resetStream();
        setIsRunning(false);
        setLiveState(null);
      },
      workspaceKind,
      applyDelta,
    );
  }, [isRunning, generalWs, activeChatId, pinnedChatTitle, refreshList, refreshTasks, onActive, applyDelta, resetStream]);

  const handleTaskCreated = useCallback((task: AgentTask) => {
    setTasks((prev) => [task, ...prev.filter((t) => t.id !== task.id)]);
  }, []);

  const handleTaskDelete = useCallback(async (taskId: string) => {
    if (!generalWs || !activeChatId) return;
    try {
      await deleteTask(generalWs.id, activeChatId, taskId);
      await refreshTasks();
    } catch { setNotification('Could not delete task.'); }
  }, [generalWs, activeChatId, refreshTasks]);

  const handleTaskStart = useCallback(async (taskId: string) => {
    if (!generalWs || !activeChatId || isRunning) return;
    try {
      await startTask(generalWs.id, activeChatId, taskId);
      await refreshTasks();
    } catch { setNotification('Could not start task.'); }
  }, [generalWs, activeChatId, isRunning, refreshTasks]);

  const handleTaskRunNow = useCallback(async (prompt: string, files: File[]) => {
    await handleSend(prompt, files);
  }, [handleSend]);

  const liveLog = liveState?.live_terminal_log || '';
  const liveParsed = parseLogToSteps(liveLog);
  const liveText = streaming.active && streaming.text ? streaming.text : (liveParsed.assistantResponse || liveState?.outputs?.agent_report || '');
  const displayMessages = [...messages];
  if ((isRunning || voiceActive) && (liveText.trim() || streaming.active || messages.length === 0 || liveParsed.steps.length > 0)) {
    displayMessages.push({
      id: 'house-live', role: 'agent', agentName: streaming.agent || getAssistantName(), content: liveText,
      timestamp: new Date().toISOString(), duration: 'active',
      metadata: {
        isTrace: true, duration: 'active', steps: liveParsed.steps,
        isStreaming: streaming.active,
        streamingReasoning: streaming.reasoning,
        streamRate: streaming.rate,
      },
    });
  }

  const liveUsage = liveState?.token_usage;
  const combinedTokenUsage = useMemo<TokenUsage>(() => ({
    total_input_tokens: (tokenUsage?.total_input_tokens || 0) + (liveUsage?.total_input_tokens || 0),
    total_output_tokens: (tokenUsage?.total_output_tokens || 0) + (liveUsage?.total_output_tokens || 0),
    total_cache_hit_tokens: (tokenUsage?.total_cache_hit_tokens || 0) + (liveUsage?.total_cache_hit_tokens || 0),
    total_cache_miss_tokens: (tokenUsage?.total_cache_miss_tokens || 0) + (liveUsage?.total_cache_miss_tokens || 0),
    total_cost: (tokenUsage?.total_cost || 0) + (liveUsage?.total_cost || 0),
    calls: [...(tokenUsage?.calls || []), ...(liveUsage?.calls || [])],
  }), [tokenUsage, liveUsage]);
  const lastCall = combinedTokenUsage.calls[combinedTokenUsage.calls.length - 1];
  const lastModelName = lastCall?.model || 'Automatic Fallback';
  const activeContextTokens = lastCall?.input || 0;

  return {
    workspaceId: generalWs?.id ?? null,
    chats, activeChatId, displayMessages, isRunning, isLoading, notification, threadRef,
    isVoiceMode, toggleVoice,
    totalCost: combinedTokenUsage.total_cost,
    totalInput: combinedTokenUsage.total_input_tokens,
    totalOutput: combinedTokenUsage.total_output_tokens,
    totalCacheHitTokens: combinedTokenUsage.total_cache_hit_tokens || 0,
    activeContextTokens,
    combinedTokenUsage, lastModelName,
    tasks,
    handleNewChat, handleSelectChat, handleRenameChat, handleDeleteChat,
    handleSend, handleCancel,
    handleTaskCreated, handleTaskDelete, handleTaskStart, handleTaskRunNow,
  };
}

// Legacy alias for house
export const useHouseChat = useChat;
