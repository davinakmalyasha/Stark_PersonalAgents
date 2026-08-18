import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChatMessage, WorkspaceInfo, AppState, ChatSummary, ChatAttachment, TokenUsage, AgentTask } from '../types';
import { fetchWorkspaces, createChat, fetchChats, fetchChat, deleteChat, updateChatTitle, uploadChatAttachments, fetchTasks, startTask, deleteTask } from '../services/api';
import { initHouseWorkspace, startHousePipeline, finishHouseMessage } from './houseChatService';
import { getAssistantName } from './assistant';
import { parseLogToSteps } from '../utils/logParser';

interface UseHouseChatProps {
  onActive?: (payload: { wsId: string; chatId: string }) => void;
}

export function useHouseChat({ onActive }: UseHouseChatProps = {}) {
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
  const abortRef = useRef<AbortController | null>(null);
  const threadRef = useRef<HTMLDivElement | null>(null);
  const liveStateRef = useRef<AppState | null>(null);

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

  // ── Boot: resolve General workspace + conversations ──
  useEffect(() => {
    let cancelled = false;
    initHouseWorkspace().then(({ ws, chats: c, initialMessages, initialChatId }) => {
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
  }, []);

  // Auto-scroll to the newest message
  useEffect(() => {
    const el = threadRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages, liveState, isRunning]);

  // Tasks follow the active chat (deferred so state settles before fetching)
  useEffect(() => {
    const id = window.setTimeout(() => { refreshTasks(); }, 0);
    return () => window.clearTimeout(id);
  }, [refreshTasks]);

  // ── Voice overlay: sync the active chat + refresh when it replies ──
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
        refreshMessages();
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

  // ── Chat handlers ──
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

  const handleSelectChat = useCallback(async (chatId: string) => {
    if (!generalWs || isRunning) return;
    try {
      const chat = await fetchChat(generalWs.id, chatId);
      setActiveChatId(chat.id);
      setMessages(chat.messages || []);
      setTokenUsage(chat.token_usage ?? null);
    } catch { setNotification('Could not load chat.'); }
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

  // ── Send (prompt + optional attachments) ──
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
    if (!chatId) {
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

    abortRef.current = startHousePipeline(
      prompt.trim(),
      ws,
      chatId,
      attachments,
      (st) => { liveStateRef.current = st; setLiveState(st); },
      async () => {
        try {
          const updated = await finishHouseMessage(ws, chatId!, liveStateRef.current);
          setMessages(updated);
          setChats(await fetchChats(ws.id));
          fetchChat(ws.id, chatId!).then((chat) => setTokenUsage(chat.token_usage ?? null)).catch(() => {});
          await refreshTasks();
        } catch { /* keep the live message visible if persistence fails */ }
        setIsRunning(false);
        setLiveState(null);
        onActive?.({ wsId: ws.id, chatId: chatId! });
      },
      (err) => {
        setNotification(typeof err === 'string' ? err : 'Error occurred.');
        setIsRunning(false);
        setLiveState(null);
      },
    );
  }, [isRunning, generalWs, activeChatId, refreshList, refreshTasks, onActive]);

  // ── Task handlers ──
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

  // ── Live streaming message ──
  const liveLog = liveState?.live_terminal_log || '';
  const liveParsed = parseLogToSteps(liveLog);
  const liveText = liveParsed.assistantResponse || liveState?.outputs?.agent_report || '';
  const displayMessages = [...messages];
  if (isRunning && (liveText.trim() || messages.length === 0)) {
    displayMessages.push({
      id: 'house-live', role: 'agent', agentName: getAssistantName(), content: liveText,
      timestamp: new Date().toISOString(), duration: 'active',
    });
  }

  // ── Combined token usage (persisted + live) ──
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
