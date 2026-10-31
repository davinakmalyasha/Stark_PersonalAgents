/**
 * Unified chat hook — Lane B merge of house/useHouseChat.ts + house/useSessionRunner.ts
 * Generic for any workspace kind (general/house/personal/project) — scales with routing/workspaceKind enum.
 * New code: `import { useChat } from '../hooks/useChat'`
 * Legacy: `house/useHouseChat.ts` re-exports this.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { ChatMessage, WorkspaceInfo, AppState, ChatSummary, ChatAttachment, TokenUsage, StreamDelta } from '../types';
import { fetchWorkspaces, createChat, fetchChats, fetchChat, deleteChat, updateChatTitle, uploadChatAttachments, abortRun, rewindChat, addChatMessage } from '../services/api';
import { initHouseWorkspace, startHousePipeline, finishHouseMessage, resolvePinnedChat } from '../house/houseChatService';
import { getAssistantName } from '../house/assistant';
import { parseLogToSteps } from '../utils/logParser';
import { applyDeltaToAccum, emptyRateAccum, emptyStreamAccum, type StreamSegment } from '../utils/streamAccum';

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
  const [isRunning, setIsRunning] = useState(false);
  const [liveState, setLiveState] = useState<AppState | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const [notification, setNotification] = useState<string | null>(null);
  const [draft, setDraft] = useState('');
  const [isVoiceMode, setIsVoiceMode] = useState(false);
  // Phase 8-V2 fix: voice runs execute in the overlay — track their live
  // activity here so the house chat streams the reply in real time.
  const [voiceActive, setVoiceActive] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  // Synchronous guard: React state can still be stale for a second Enter
  // press, which used to start two runs on the same chat.
  const sendInFlightRef = useRef(false);
  const threadRef = useRef<HTMLDivElement | null>(null);
  const liveStateRef = useRef<AppState | null>(null);
  const startedAtRef = useRef<number | null>(null);
  // Owning run target for Stop: persist the partial into the same chat.
  const runWsRef = useRef<WorkspaceInfo | null>(null);
  const runChatIdRef = useRef<string | null>(null);
  const [streaming, setStreaming] = useState<{ text: string; reasoning: string; agent: string; segments: StreamSegment[]; active: boolean; rate: number }>({ text: '', reasoning: '', agent: '', segments: [], active: false, rate: 0 });
  const streamRef = useRef(emptyStreamAccum());
  const rateRef = useRef(emptyRateAccum());

  const applyDelta = useCallback((delta: StreamDelta) => {
    const rate = applyDeltaToAccum(streamRef.current, rateRef.current, delta, performance.now());
    setStreaming({
      text: streamRef.current.text,
      reasoning: streamRef.current.reasoning,
      agent: streamRef.current.agent,
      segments: [...streamRef.current.segments],
      active: true,
      rate,
    });
  }, []);

  const resetStream = useCallback(() => {
    streamRef.current = emptyStreamAccum();
    rateRef.current = emptyRateAccum();
    setStreaming({ text: '', reasoning: '', agent: '', segments: [], active: false, rate: 0 });
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
        streamRef.current = emptyStreamAccum();
        setStreaming((s) => ({ ...s, active: false, text: '', reasoning: '', segments: [] }));        refreshMessages();
      }
      // Phase 8-V2 fix: a voice run started — begin the live bubble.
      if (ev?.type === 'pipeline-start') {
        setVoiceActive(true);
        streamRef.current = emptyStreamAccum();
        rateRef.current = emptyRateAccum();
        setStreaming({ text: '', reasoning: '', agent: '', segments: [], active: false, rate: 0 });
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
          segments: [...streamRef.current.segments],
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
      // When run starts, replace live input bubble with the single canonical
      // user message (avoid duplicate Hello x N stacking).
      if (ev?.type === 'pipeline-start' && (ev as any).prompt) {
        const prompt = String((ev as any).prompt);
        setMessages((prev) => {
          const withoutLive = prev.filter((m) => m.id !== 'voice-live-input');
          const last = withoutLive[withoutLive.length - 1];
          if (last && last.role === 'user' && last.content === prompt) return withoutLive;
          return [...withoutLive, { id: `voice-user-${Date.now()}`, role: 'user', content: prompt, timestamp: new Date().toISOString() }];
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
    sendInFlightRef.current = false;
    abortRef.current?.abort();
    const ws = runWsRef.current;
    const chatId = runChatIdRef.current;
    // Actually cancel server-side too — detaching the fetch alone leaves the
    // run burning tokens (the orphan-run bug).
    if (chatId) abortRun(chatId).catch(() => {});
    const partialText = streamRef.current.text;
    const partialReasoning = streamRef.current.reasoning;
    const log = liveStateRef.current?.live_terminal_log || '';
    const hasPartial = !!(partialText.trim() || partialReasoning.trim() || log.trim());
    setIsRunning(false);
    setLiveState(null);
    if (ws && chatId && hasPartial) {
      // Keep the partial: same persist path as completion, marked stopped.
      const duration = startedAtRef.current ? Math.round((Date.now() - startedAtRef.current) / 1000) : undefined;
      const snap = liveStateRef.current;
      finishHouseMessage(ws, chatId, snap, duration, {
        thinking: partialReasoning,
        liveTimeline: [...streamRef.current.segments],
        stopped: true,
        textOverride: partialText,
      }).then(setMessages).catch(() => {});
    }
    resetStream();
  }, []);

  const handleRewind = useCallback(async (message: ChatMessage) => {
    if (!generalWs || !activeChatId || isRunning) return;
    try {
      const result = await rewindChat(generalWs.id, activeChatId, message.id);
      const chat = await fetchChat(generalWs.id, activeChatId);
      setMessages(chat.messages || []);
      setTokenUsage(chat.token_usage ?? null);
      setDraft(result.prompt || '');
      if (result.restore?.conflicts?.length) {
        setNotification(`Rewound, but ${result.restore.conflicts.length} file(s) could not be restored.`);
      } else {
        setNotification('Rewound. The message is back in the prompt.');
      }
      await refreshList(generalWs);
    } catch (e) {
      setNotification(e instanceof Error ? e.message : 'Rewind failed.');
    }
  }, [generalWs, activeChatId, isRunning, refreshList]);

  const handleSend = useCallback(async (prompt: string, files: File[]) => {
    if (isRunning || sendInFlightRef.current) return;
    if (!prompt.trim() && !files.length) return;
    sendInFlightRef.current = true;
    setNotification(null);

    let ws = generalWs;
    if (!ws) {
      try {
        const wsList = await fetchWorkspaces();
        ws = wsList.find((w) => w.isGeneral) || wsList[0] || null;
        setGeneralWs(ws);
      } catch {
        setNotification('VIN could not reach the backend.');
        sendInFlightRef.current = false;
        return;
      }
      if (!ws) {
        setNotification('No general workspace available yet.');
        sendInFlightRef.current = false;
        return;
      }
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
        sendInFlightRef.current = false;
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
        sendInFlightRef.current = false;
        return;
      }
    }

    let attachments: ChatAttachment[] = [];
    if (files.length) {
      try {
        attachments = await uploadChatAttachments(ws.id, chatId, files);
      } catch {
        setNotification('Your files could not be uploaded.');
        sendInFlightRef.current = false;
        return;
      }
    }

    const userMsg: ChatMessage = {
      id: `usr-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`,
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
    runWsRef.current = ws;
    runChatIdRef.current = chatId;

    abortRef.current = startHousePipeline(
      prompt.trim(),
      ws,
      chatId,
      attachments,
      (st) => { liveStateRef.current = st; setLiveState(st); },
      async () => {
        try {
          const duration = startedAtRef.current ? Math.round((Date.now() - startedAtRef.current) / 1000) : undefined;
          const updated = await finishHouseMessage(ws, chatId!, liveStateRef.current, duration, { thinking: streamRef.current.reasoning, liveTimeline: [...streamRef.current.segments] });
          setMessages(updated);
          setChats(await fetchChats(ws.id));
          fetchChat(ws.id, chatId!).then((chat) => setTokenUsage(chat.token_usage ?? null)).catch(() => {});
            } catch { /* keep the live message visible if persistence fails */ }
        resetStream();
        setIsRunning(false);
        setLiveState(null);
        sendInFlightRef.current = false;
        onActive?.({ wsId: ws.id, chatId: chatId! });
      },
      (err) => {
        const errText = typeof err === 'string' ? err : 'Error occurred.';
        // Failure keeps the same persist path as completion/cancel: the
        // worked steps must survive as an expandable trace, and the error is
        // appended inline after them — never a banner that replaces history.
        const partialText = streamRef.current.text;
        const partialReasoning = streamRef.current.reasoning;
        const segments = [...streamRef.current.segments];
        const log = liveStateRef.current?.live_terminal_log || '';
        const hasPartial = !!(partialText.trim() || partialReasoning.trim() || log.trim() || segments.length);
        const duration = startedAtRef.current ? Math.round((Date.now() - startedAtRef.current) / 1000) : undefined;
        const ws = runWsRef.current;
        const chatId = runChatIdRef.current;
        setIsRunning(false);
        setLiveState(null);
        if (ws && chatId) {
          (async () => {
            if (hasPartial) {
              const updated = await finishHouseMessage(ws, chatId, liveStateRef.current, duration, {
                thinking: partialReasoning,
                liveTimeline: segments,
                stopped: true,
                textOverride: partialText,
              });
              setMessages(updated);
            }
            await addChatMessage(ws.id, chatId, 'error', errText).catch(() => {});
            const chat = await fetchChat(ws.id, chatId).catch(() => null);
            if (chat) setMessages(chat.messages || []);
          })().catch(() => {});
        } else {
          setNotification(errText);
        }
        resetStream();
        sendInFlightRef.current = false;
      },
      workspaceKind,
      applyDelta,
    );
  }, [isRunning, generalWs, activeChatId, pinnedChatTitle, refreshList, onActive, applyDelta, resetStream]);

  const liveLog = liveState?.live_terminal_log || '';
  const liveParsed = useMemo(() => parseLogToSteps(liveLog), [liveLog]);
  const liveText = streaming.active && streaming.text ? streaming.text : (liveParsed.assistantResponse || liveState?.outputs?.agent_report || '');
  const displayMessages = [...messages];
  // Show the live bubble immediately on send so the "Thinking" placeholder
  // appears before the first reasoning delta (the blank-wait bug).
  if (isRunning || voiceActive) {
    displayMessages.push({
      id: 'house-live', role: 'agent', agentName: streaming.agent || getAssistantName(), content: liveText,
      timestamp: new Date().toISOString(), duration: 'active',
      metadata: {
        isTrace: true, duration: 'active', steps: liveParsed.steps,
        isStreaming: streaming.active,
        streamingReasoning: streaming.reasoning,
        streamRate: streaming.rate,
        liveTimeline: streaming.segments,
      },
    });
  }

  const liveUsage = liveState?.token_usage;
  // Fallback for chats created before token_usage_json existed — sum per-message usage
  const messagesUsage = useMemo<TokenUsage | null>(() => {
    if (tokenUsage) return null;
    let agg: TokenUsage | null = null;
    for (const m of messages) {
      const u = (m.metadata as any)?.usage as TokenUsage | undefined;
      if (!u) continue;
      if (!agg) agg = { total_input_tokens: 0, total_output_tokens: 0, total_cache_hit_tokens: 0, total_cache_miss_tokens: 0, total_cost: 0, calls: [] };
      agg.total_input_tokens += u.total_input_tokens || 0;
      agg.total_output_tokens += u.total_output_tokens || 0;
      agg.total_cache_hit_tokens += (u as any).total_cache_hit_tokens || 0;
      agg.total_cache_miss_tokens += (u as any).total_cache_miss_tokens || 0;
      agg.total_cost += u.total_cost || 0;
      agg.calls.push(...(u.calls || []));
    }
    return agg;
  }, [messages, tokenUsage]);
  const effectiveTokenUsage = tokenUsage || messagesUsage;
  const combinedTokenUsage = useMemo<TokenUsage>(() => ({
    total_input_tokens: (effectiveTokenUsage?.total_input_tokens || 0) + (liveUsage?.total_input_tokens || 0),
    total_output_tokens: (effectiveTokenUsage?.total_output_tokens || 0) + (liveUsage?.total_output_tokens || 0),
    total_cache_hit_tokens: (effectiveTokenUsage?.total_cache_hit_tokens || 0) + (liveUsage?.total_cache_hit_tokens || 0),
    total_cache_miss_tokens: (effectiveTokenUsage?.total_cache_miss_tokens || 0) + (liveUsage?.total_cache_miss_tokens || 0),
    total_cost: (effectiveTokenUsage?.total_cost || 0) + (liveUsage?.total_cost || 0),
    calls: [...(effectiveTokenUsage?.calls || []), ...(liveUsage?.calls || [])],
  }), [effectiveTokenUsage, liveUsage]);
  const lastCall = combinedTokenUsage.calls[combinedTokenUsage.calls.length - 1];
  const lastModelName = lastCall?.model || 'Automatic Fallback';
  const activeContextTokens = lastCall?.input || 0;

  const seedDraft = useCallback((prompt: string) => setDraft(prompt), []);

  return {
    workspaceId: generalWs?.id ?? null,
    chats, activeChatId, displayMessages, liveState, isRunning, isLoading, notification, threadRef,
    isVoiceMode, toggleVoice,
    totalCost: combinedTokenUsage.total_cost,
    totalInput: combinedTokenUsage.total_input_tokens,
    totalOutput: combinedTokenUsage.total_output_tokens,
    totalCacheHitTokens: combinedTokenUsage.total_cache_hit_tokens || 0,
    activeContextTokens,
    combinedTokenUsage, lastModelName,
    handleNewChat, handleSelectChat, handleRenameChat, handleDeleteChat,
    handleSend, handleCancel, handleRewind,
    draft, seedDraft, clearDraft: () => setDraft(''),
  };
}

// Legacy alias for house
export const useHouseChat = useChat;
