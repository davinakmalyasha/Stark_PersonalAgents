import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { AppState, ChatMessage, ChatAttachment, SessionTab, TokenUsage, StreamDelta } from '../types';
import { addChatMessage, fetchChat, uploadChatAttachments } from '../services/api';
import { startHousePipeline } from './houseChatService';
import { parseLogToSteps } from '../utils/logParser';

interface ThreadState { key: string; messages: ChatMessage[] }
interface UsageState { key: string; value: TokenUsage | null }

/**
 * Runs the agent pipeline against ANY session (project chat or general chat).
 * Used by the coding-tab mini-IDE: sends from the floating prompt dock land in
 * the tab's own project chat; live state feeds the Activity/Diff/Terminal
 * panels; the finished run is persisted as one unified trace message.
 *
 * The hook instance survives session switches (it lives in House), so every
 * stream callback is guarded by the session key it was started with — a
 * finished run never writes another tab's messages. Thread/usage state is
 * keyed by session, so a stale view never shows the wrong conversation.
 */
export function useSessionRunner(session: SessionTab | null, onFinished?: () => void) {
  const [thread, setThread] = useState<ThreadState>({ key: '', messages: [] });
  const [usageState, setUsageState] = useState<UsageState>({ key: '', value: null });
  const [isRunning, setIsRunning] = useState(false);
  const [liveState, setLiveState] = useState<AppState | null>(null);
  const [notification, setNotification] = useState<string | null>(null);

  const abortRef = useRef<AbortController | null>(null);
  const liveStateRef = useRef<AppState | null>(null);
  const startedAtRef = useRef<number | null>(null);
  const runningRef = useRef(false);
  const currentKeyRef = useRef('');
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

  const finishedRef = useRef(onFinished);
  useEffect(() => { finishedRef.current = onFinished; }, [onFinished]);

  const ws = session?.workspaceId ?? '';
  const chatId = session?.chatId ?? '';
  const sessionKey = ws && chatId ? `${ws}:${chatId}` : '';
  const lastChatJsonRef = useRef('');

  // Thread/usage are keyed — a stale key renders as empty instead of
  // flashing the previous session's conversation.
  const persistedMessages = useMemo<ChatMessage[]>(
    () => (thread.key === sessionKey ? thread.messages : []),
    [thread, sessionKey],
  );
  const persistedUsage = usageState.key === sessionKey ? usageState.value : null;

  // Live message while running: carries the parsed steps so subagent spawn
  // blocks appear in the thread the moment the delegation happens.
  const liveParsed = useMemo(
    () => parseLogToSteps(liveState?.live_terminal_log || ''),
    [liveState],
  );
  const displayMessages = useMemo<ChatMessage[]>(() => {
    if (!isRunning || !sessionKey) return persistedMessages;
    const liveSteps = liveParsed.steps.length > 0 ? liveParsed.steps : [];
    const liveText = streaming.active && streaming.text ? streaming.text : (liveParsed.assistantResponse || '');
    return [
      ...persistedMessages,
      {
        id: 'session-live',
        role: 'agent',
        agentName: streaming.agent || undefined,
        content: liveText,
        timestamp: new Date().toISOString(),
        duration: 'active',
        metadata: {
          isTrace: true, duration: 'active', steps: liveSteps,
          isStreaming: streaming.active,
          streamingReasoning: streaming.reasoning,
          streamRate: streaming.rate,
        },
      },
    ];
  }, [isRunning, sessionKey, persistedMessages, liveParsed, streaming]);

  // ── Load (and lightly poll) the session's chat history ──
  // Diff-gated: avoid the 5s tick that was recreating House + the whole
  // IDE (Prism re-tokenize + Markdown re-parse) even when the chat hadn't
  // changed. Compares a compact fingerprint (length + last id + updatedAt).
  useEffect(() => {
    currentKeyRef.current = sessionKey;
    lastChatJsonRef.current = '';
    if (!sessionKey) return;
    let cancelled = false;
    const fingerprint = (chat: { messages?: { id: string }[]; updatedAt?: string; token_usage?: unknown }) =>
      `${chat.messages?.length ?? 0}:${chat.messages?.[chat.messages.length - 1]?.id ?? ''}:${chat.updatedAt ?? ''}:${JSON.stringify(chat.token_usage ?? null)}`;
    const load = () => {
      fetchChat(ws, chatId)
        .then((chat) => {
          if (cancelled || currentKeyRef.current !== sessionKey) return;
          const fp = fingerprint(chat as unknown as { messages: { id: string }[]; updatedAt: string; token_usage: unknown });
          if (fp === lastChatJsonRef.current) return;
          lastChatJsonRef.current = fp;
          setThread({ key: sessionKey, messages: chat.messages || [] });
          setUsageState({ key: sessionKey, value: chat.token_usage ?? null });
        })
        .catch(() => {});
    };
    load();
    // Covers replies written by the voice overlay or the coding workspace
    // while this tab is open. Skipped while our own run is streaming.
    const id = window.setInterval(() => { if (!runningRef.current && !document.hidden) load(); }, 5000);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [sessionKey, ws, chatId]);

  const send = useCallback(async (target: SessionTab, prompt: string, files: File[]) => {
    if (runningRef.current) return;
    if (!prompt.trim() && !files.length) return;
    const s = target;
    const runKey = `${s.workspaceId}:${s.chatId}`;
    setNotification(null);

    const wsInfo = {
      id: s.workspaceId, name: s.workspaceName, path: s.workspacePath,
      exists: true, chatCount: 0, addedAt: '',
    };

    let attachments: ChatAttachment[] = [];
    if (files.length) {
      try {
        attachments = await uploadChatAttachments(s.workspaceId, s.chatId, files);
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
    setThread((prev) => ({
      key: runKey,
      messages: [...(prev.key === runKey ? prev.messages : []), userMsg],
    }));
    runningRef.current = true;
    startedAtRef.current = Date.now();
    setIsRunning(true);
    setLiveState(null);
    resetStream();

    const stillCurrent = () => currentKeyRef.current === runKey;

    abortRef.current = startHousePipeline(
      prompt.trim(),
      wsInfo,
      s.chatId,
      attachments,
      (st) => {
        if (!stillCurrent()) return;
        liveStateRef.current = st;
        setLiveState(st);
      },
      async () => {
        try {
          const state = liveStateRef.current;
          const log = state?.live_terminal_log || '';
          const { steps, assistantResponse } = parseLogToSteps(log);
          const finalResponse = assistantResponse || state?.outputs?.agent_report || '';
          const duration = startedAtRef.current
            ? Math.round((Date.now() - startedAtRef.current) / 1000) + 's'
            : undefined;
          await addChatMessage(s.workspaceId, s.chatId, 'agent', finalResponse, {
            isTrace: true,
            // Full step list (incl. subagent tool calls) — the coding tab's
            // Activity panel replays the whole run from this metadata.
            steps,
            duration,
            artifacts: state?.artifacts,
          });
          const chat = await fetchChat(s.workspaceId, s.chatId);
          if (stillCurrent()) {
            setThread({ key: runKey, messages: chat.messages || [] });
            setUsageState({ key: runKey, value: chat.token_usage ?? null });
          }
        } catch { /* keep the live message visible if persistence fails */ }
        resetStream();
        runningRef.current = false;
        setIsRunning(false);
        if (stillCurrent()) setLiveState(null);
        finishedRef.current?.();
      },
      (err) => {
        if (stillCurrent()) {
          setNotification(typeof err === 'string' ? err : 'Error occurred.');
          setLiveState(null);
        }
        resetStream();
        runningRef.current = false;
        setIsRunning(false);
        finishedRef.current?.();
      },
      undefined,
      applyDelta,
    );
  }, [applyDelta, resetStream]);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    runningRef.current = false;
    setIsRunning(false);
    setLiveState(null);
  }, []);

  // ── Combined usage: persisted chat usage + live run usage ──
  const liveUsage = liveState?.token_usage;
  const usage: TokenUsage = {
    total_input_tokens: (persistedUsage?.total_input_tokens || 0) + (liveUsage?.total_input_tokens || 0),
    total_output_tokens: (persistedUsage?.total_output_tokens || 0) + (liveUsage?.total_output_tokens || 0),
    total_cache_hit_tokens: (persistedUsage?.total_cache_hit_tokens || 0) + (liveUsage?.total_cache_hit_tokens || 0),
    total_cache_miss_tokens: (persistedUsage?.total_cache_miss_tokens || 0) + (liveUsage?.total_cache_miss_tokens || 0),
    total_cost: (persistedUsage?.total_cost || 0) + (liveUsage?.total_cost || 0),
    calls: [...(persistedUsage?.calls || []), ...(liveUsage?.calls || [])],
  };
  const lastCall = usage.calls[usage.calls.length - 1];
  const activeContextTokens = lastCall?.input || 0;

  return {
    messages: displayMessages, isRunning, liveState, notification, usage, activeContextTokens,
    send, cancel, clearNotification: () => setNotification(null),
  };
}
