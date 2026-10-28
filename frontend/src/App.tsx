import { lazy, Suspense, useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { X, FolderGit2, Palette } from 'lucide-react';
import TitleBar from './components/Layout/TitleBar';
import Sidebar from './components/Layout/Sidebar';
import AgentCanvas from './components/Canvas/AgentCanvas';
import TokenStatsModal from './components/Layout/TokenStatsModal';
import QuestionModal from './components/Workspace/QuestionModal';
import FileExplorer from './components/Workspace/FileExplorer';
const DesignWorkspace = lazy(() => import('./modules/design-workspace/DesignWorkspace'));
import { usePipeline } from './hooks/usePipeline';
import { fetchModels, fetchActiveRun, addChatMessage, createChat, fetchChat, fetchWorkspaces, fetchChats, updateChatTitle, uploadChatAttachments, steerRun, setChatMode, getChatSnapshots, restoreRun, undoRun, abortRun, rewindChat, API_BASE, apiUrl } from './services/api';
import type { ChatMessage, WorkspaceInfo, ChatData, ModelOption, AppState, ChatAttachment } from './types';
import { parseLogToSteps, filterOrchestratorSteps } from './utils/logParser';

const isProjectWorkspace = (w: WorkspaceInfo): boolean =>
  !w.isGeneral && !w.isHouse && !w.isPersonal && !(w as { isDfd?: boolean }).isDfd;

const WELCOME_MESSAGES: ChatMessage[] = [
  {
    id: 'welcome-1',
    role: 'system',
    content: 'Connected to Stark v3 — orchestrator online, workspace agents ready. Add a workspace folder to get started.',
    timestamp: new Date().toISOString(),
  },
];

export default function App() {
  const { run, cancel, state: pipeState, isRunning, error, streaming, runMode, attachMirror } = usePipeline();
  const state: AppState | null = pipeState;

  const [messages, setMessages] = useState<ChatMessage[]>(WELCOME_MESSAGES);
  const [voiceLiveRun, setVoiceLiveRun] = useState<{
    state: AppState | null;
    active: boolean;
    workspaceId: string;
    chatId: string;
    startedAt: number;
    runSeq?: number;
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
  const [draft, setDraft] = useState('');
  const [chatModeState, setChatModeState] = useState<'plan' | 'build'>('build');
  const [sidebarRefreshTick, setSidebarRefreshTick] = useState(0);
  const [showShortcuts, setShowShortcuts] = useState(false);
  const [showFiles, setShowFiles] = useState(false);
  const [designOpen, setDesignOpen] = useState(false);

  // ── Top-bar tabs (opencode-style) for the legacy coding workspace ──
  // Persisted separately from the main-workspace ui_state (cw_ prefix avoids collision).
  const CW_TABS_KEY = 'cw_open_tabs';
  const CW_ACTIVE_KEY = 'cw_active_tab';
  type CwTab = { workspaceId: string; chatId: string; title: string; workspaceName: string };
  const tabKey = (wsId: string, cId: string) => `${wsId}:${cId}`;
  const [cwTabs, setCwTabs] = useState<CwTab[]>(() => {
    try { const raw = localStorage.getItem(CW_TABS_KEY); return raw ? JSON.parse(raw) : []; } catch { return []; }
  });
  const [cwActiveKey, setCwActiveKey] = useState<string | null>(() => {
    try { return localStorage.getItem(CW_ACTIVE_KEY); } catch { return null; }
  });
  useEffect(() => { try { localStorage.setItem(CW_TABS_KEY, JSON.stringify(cwTabs)); } catch { void 0; } }, [cwTabs]);
  useEffect(() => { try { if (cwActiveKey) localStorage.setItem(CW_ACTIVE_KEY, cwActiveKey); else localStorage.removeItem(CW_ACTIVE_KEY); } catch { void 0; } }, [cwActiveKey]);

  const upsertCwTab = useCallback((wsId: string, chatId: string, title: string, wsName: string) => {
    const key = tabKey(wsId, chatId);
    setCwTabs((prev) => {
      const exists = prev.find((t) => t.workspaceId === wsId && t.chatId === chatId);
      if (exists) {
        return prev.map((t) => (t.workspaceId === wsId && t.chatId === chatId ? { ...t, title, workspaceName: wsName } : t));
      }
      return [...prev, { workspaceId: wsId, chatId, title, workspaceName: wsName }];
    });
    setCwActiveKey(key);
  }, []);

  const wasRunning = useRef(false);
  const runStartTime = useRef<number | null>(null);
  // Stop bookkeeping: the persist effect below runs after cancel() already
  // cleared the stream, so capture "was this a Stop" + partial thinking here.
  const cancelledRef = useRef(false);
  const cancelThinkingRef = useRef('');
  const cancelTextRef = useRef('');
  const lastStreamingRef = useRef(streaming);
  useEffect(() => {
    if (streaming.reasoning || streaming.segments.length || streaming.text) {
      lastStreamingRef.current = streaming;
    }
  }, [streaming]);
  // Live mirroring (mobile/remote): remember which run instance we already
  // attached to so the poll doesn't re-attach to a finished run.
  const mirroredRunStart = useRef('');

  // Init: fetch models + workspaces on mount.
  // `#/coding?ws=<id>&chat=<id>` (from the main workspace's "open in Coding
  // Workspace" button) selects that exact workspace + chat after boot.
  useEffect(() => {
    const hashQuery = window.location.hash.split('?')[1] ?? '';
    const params = new URLSearchParams(hashQuery);
    let targetWsId: string | null = params.get('ws');
    let targetChatId: string | null = params.get('chat');
    if (window.location.hash.startsWith('#/design') || params.get('addon') === 'design') {
      setDesignOpen(true);
    }
    // Restore last tab if no deep-link is present
    if (!targetWsId && !targetChatId && cwActiveKey) {
      const sep = cwActiveKey.indexOf(':');
      if (sep > 0) {
        targetWsId = cwActiveKey.slice(0, sep);
        targetChatId = cwActiveKey.slice(sep + 1);
      }
    }

    fetchModels().then((m) => { if (m.length) setModels(m); }).catch(() => {});
    fetchWorkspaces().then(async (ws) => {
      if (ws.length === 0) return;
      let initialWs = ws.find(isProjectWorkspace) || ws.find((w) => w.isGeneral) || ws[0];
      if (targetWsId) {
        const requested = ws.find((w) => w.id === targetWsId);
        if (requested && isProjectWorkspace(requested)) initialWs = requested;
      }
      const nonProjectIds = new Set(ws.filter((w) => !isProjectWorkspace(w)).map((w) => w.id));
      setCwTabs((prev) => prev.filter((t) => !nonProjectIds.has(t.workspaceId)));
      setActiveWorkspace(initialWs);
      try {
        const chats = await fetchChats(initialWs.id);
        const targetChat = chats.find((c) => c.id === targetChatId) ?? null;
        if (!targetChat && targetChatId) {
          // The requested chat may exist but be missing from the summary list
          // (e.g. brand-new) — try fetching it directly before falling back.
          const direct = await fetchChat(initialWs.id, targetChatId).catch(() => null);
          if (direct) {
            setActiveChat(direct);
            setMessages(direct.messages.length > 0 ? direct.messages : [{
              id: `sys-${Date.now()}`,
              role: 'system',
              content: initialWs.isGeneral ? 'Connected. How can I help you?' : `Chat ready in ${initialWs.name}. What would you like to build?`,
              timestamp: new Date().toISOString(),
            }]);
            upsertCwTab(initialWs.id, direct.id, direct.title || 'Untitled', initialWs.name);
            return;
          }
        }
        const fallback = targetChat ?? chats[0] ?? null;
        if (fallback) {
          const chat = await fetchChat(initialWs.id, fallback.id);
          setActiveChat(chat);
          setMessages(chat.messages.length > 0 ? chat.messages : [{
            id: `sys-${Date.now()}`,
            role: 'system',
            content: initialWs.isGeneral ? 'Connected. How can I help you?' : `Chat ready in ${initialWs.name}. What would you like to build?`,
            timestamp: new Date().toISOString(),
          }]);
          upsertCwTab(initialWs.id, chat.id, chat.title || fallback.title || 'Untitled', initialWs.name);
        }
      } catch (err) {
        console.error('[App] Failed to auto-load chats:', err);
      }
    }).catch(() => {});
  // eslint-disable-next-line react-hooks/exhaustive-deps -- mount-only restore from cwActiveKey + upsert
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
      // Stop (not done): keep the partial, marked stopped. Capture + reset
      // here — cancel() already cleared the live stream by the time we run.
      const wasStopped = cancelledRef.current;
      const stoppedThinking = cancelThinkingRef.current;
      const stoppedText = cancelTextRef.current;
      cancelledRef.current = false;
      cancelThinkingRef.current = '';
      cancelTextRef.current = '';
      if (activeWorkspace && activeChat) {
        // Mirrored run: the initiating client persists the trace — just reload.
        if (runMode === 'mirror') {
          fetchChat(activeWorkspace.id, activeChat.id).then((c) => {
            setActiveChat(c);
            setMessages(c.messages);
          }).catch(() => {});
          return;
        }
        console.log('[App] Pipeline run completed. Saving trace and final response...');
        setIsPersisting(true);
        
        const saveAndRefresh = async () => {
          try {
            const log = state?.live_terminal_log;
            if (log) {
              const { steps, assistantResponse } = parseLogToSteps(log);
              // General mode stores the final text in outputs rather than the
              // terminal log. Use it as the same unified trace response.
              // On Stop, the visible streamed partial wins (it never lands in
              // the log).
              const finalResponse = stoppedText.trim() || assistantResponse || state?.outputs?.agent_report || '';
              const duration = runStartTime.current
                ? Math.round((Date.now() - runStartTime.current) / 1000) + 's'
                : undefined;

              // Main chat only shows orchestrator-level actions (delegation,
              // supervisor thoughts, final summary). Subagent tool calls
              // belong in the canvas subagent drawer, not here.
              const chatSteps = filterOrchestratorSteps(steps);

              // Save a single unified agent message with trace metadata
              // Wire streaming reasoning on success too (was only on Stop) — otherwise past reasoning vanishes.
              // Use lastStreamingRef because usePipeline resets streaming before this effect fires.
              const live = lastStreamingRef.current;
              const successThinking = (live.reasoning || streaming.reasoning || '').trim();
              const successTimeline = live.segments?.length ? [...live.segments] : streaming.segments?.length ? [...streaming.segments] : [];
              await addChatMessage(
                activeWorkspace.id,
                activeChat.id,
                'agent',
                finalResponse,
                { isTrace: true, steps: chatSteps, duration, artifacts: state?.artifacts, project_ref: state?.project_ref ?? undefined, usage: state?.token_usage ?? undefined, thinking: (stoppedThinking || successThinking) || undefined, liveTimeline: successTimeline.length ? successTimeline : undefined, ...(wasStopped ? { stopped: true } : {}) }
              );
            }
            // Now re-fetch to sync messages and token stats
            const updatedChat = await fetchChat(activeWorkspace.id, activeChat.id);
            setActiveChat(updatedChat);
            setMessages(updatedChat.messages);
            // Delegations may have created chats in ANY workspace this run —
            // refresh the sidebar so they show up without a manual reload.
            setSidebarRefreshTick((t) => t + 1);
          } catch (err) {
            console.error('[App] Failed to complete run persistence:', err);
          } finally {
            setIsPersisting(false);
          }
        };

        saveAndRefresh();
      }
    }
  }, [isRunning, activeWorkspace, activeChat, runMode, state?.live_terminal_log, state?.project_ref, state?.workspace_created, state?.token_usage]);

  // ── Live mirroring (mobile/remote feature) ──
  // While idle, watch /api/runs/active: when a run starts for THIS chat from
  // another client (phone or second browser), attach to its event stream so
  // both screens show the identical live session.
  // Suppressed while a voice session owns this chat — the overlay's SSE is
  // the working stream; attaching a second one is the stream-then-gone bug.
  useEffect(() => {
    if (isRunning || !activeChat) return;
    let cancelled = false;
    const tick = async () => {
      if (cancelled || document.hidden) return;
      try {
        if (Date.now() - lastVoiceEventAt.current < 30000) return;
        const run = await fetchActiveRun();
        if (cancelled || !run.running) return;
        if (run.chat_id === activeChat.id && run.started_at && run.started_at !== mirroredRunStart.current) {
          mirroredRunStart.current = run.started_at;
          attachMirror(run.chat_id);
        }
      } catch { /* backend briefly unavailable */ }
    };
    tick();
    const id = window.setInterval(tick, 2500);
    return () => { cancelled = true; window.clearInterval(id); };
  }, [isRunning, activeChat?.id, attachMirror]);

  useEffect(() => {
    if (!error) return;
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

    // Stage 4: slash-command pre-dispatch. /plan, /build, /undo, /rewind
    // are intercepted before the prompt reaches the LLM.
    const trimmed = (prompt || '').trim();
    if (trimmed.startsWith('/')) {
      const ws = activeWorkspace;
      const chat = activeChat;
      const [cmd, ...rest] = trimmed.slice(1).split(/\s+/);
      const arg = rest.join(' ');
      try {
        if (cmd === 'plan') {
          if (!ws || !chat) { setNotification('No active chat to put in plan mode.'); return; }
          await setChatMode(chat.id, 'plan');
          setChatModeState('plan');
          setNotification('Plan mode on. The agent can research and propose, but cannot edit. Type /build to leave plan mode.');
          return;
        }
        if (cmd === 'build') {
          if (!ws || !chat) { setNotification('No active chat to switch to build mode.'); return; }
          await setChatMode(chat.id, 'build');
          setChatModeState('build');
          setNotification('Build mode on. The agent can write, edit, and run commands again.');
          return;
        }
        if (cmd === 'undo') {
          if (!chat) { setNotification('No active chat to undo.'); return; }
          const r = await fetch(`${API_BASE}/runs/active`);
          const active = await r.json().catch(() => null);
          if (!active || !active.run_id) { setNotification('No active run to undo.'); return; }
          await undoRun(active.run_id);
          setNotification('Undo complete.');
          return;
        }
        if (cmd === 'rewind') {
          if (!ws || !chat) { setNotification('No active chat to rewind.'); return; }
          const idx = parseInt(arg || '1', 10);
          if (Number.isNaN(idx) || idx < 1) { setNotification('Usage: /rewind <index>  (1 = most recent snapshot run)'); return; }
          const groups = await getChatSnapshots(chat.id, ws.id);
          if (!groups.length) { setNotification('No snapshots available for this chat yet.'); return; }
          const target = groups[idx - 1];
          if (!target) { setNotification(`No snapshot at index ${idx} (have ${groups.length}).`); return; }
          const ok = window.confirm(`Rewind to ${new Date(target.created_at * 1000).toLocaleString()}? This will restore ${target.files.length} file(s).`);
          if (!ok) return;
          const result = await restoreRun(chat.id, target.run_id, ws.id);
          setNotification(result.ok ? 'Rewind complete.' : `Rewind partial: ${result.messages.filter(m => !m.startsWith('restored')).join('; ') || 'see console'}`);
          return;
        }
      } catch (e) {
        setNotification(`Slash command failed: ${e}`);
        return;
      }
    }

    // Resolve workspace — auto-select General or first available
    let ws = activeWorkspace;

    if (!ws || !isProjectWorkspace(ws)) {
      console.log('[App.handleSend] no active workspace, fetching...');
      try {
        const workspaces = await fetchWorkspaces();
        console.log('[App.handleSend] workspaces fetched:', workspaces);
        if (workspaces.length === 0) {
          setNotification('No chat available. Try restarting the server.');
          return;
        }
        // The coding workspace is project-first; fall back to General only
        // when no project folder exists.
        ws = workspaces.find(isProjectWorkspace) || workspaces.find((w) => w.isGeneral) || workspaces[0];
        setActiveWorkspace(ws);
        console.log('[App.handleSend] auto-selected workspace:', ws);
      } catch (err) {
        console.error('[App.handleSend] fetchWorkspaces failed:', err);
        setNotification('Unable to reach backend. Is the API server running on port 8123?');
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

    // ── Mid-run message → steering queue ──
    // The orchestrator is an event-driven coordinator: while it (or its
    // delegations) are still working, a new message is delivered as steering
    // at the next step boundary instead of starting a second pipeline.
    if (isRunning) {
      addChatMessage(ws.id, chat.id, 'user', prompt).catch(() => {});
      const queued = await steerRun(chat.id, prompt).catch(() => false);
      setNotification(queued
        ? 'Queued — the agent will pick this up at its next step.'
        : 'Could not queue the message. Try again when the run finishes.');
      return;
    }

    console.log('[App.handleSend] calling run...');
    runStartTime.current = Date.now();
    // Pass workspace_kind so the router picks the right agent: project/coding →
    // orchestrator (or project_dev), general → orchestrator, house → house.
    const kind = ((ws as any).workspaceKind || '') as string;
    run(prompt, model, temperature, ws.path, chat.id, attachments, kind);
  }, [run, model, temperature, activeWorkspace, activeChat, isRunning]);

  // ── Voice mode: driven by the floating overlay window (voice.html) ──
  const [isVoiceMode, setIsVoiceMode] = useState(false);
  // Phase 8-V2: live STT from the overlay shows in the main window's dock,
  // and token deltas stream into the voice-live agent bubble.
  const [voiceInputText, setVoiceInputText] = useState('');
  const voiceStreamRef = useRef({ text: '', reasoning: '' });
  // Single voice-trace owner: the overlay's run is canonical. runSeq from
  // pipeline-start lets stale done/state/delta frames fall through instead of
  // wiping the live run. lastVoiceEventAt suppresses the mirror poll while a
  // voice session owns this chat (the mirror is the broken duplicate stream).
  const voiceRunSeqRef = useRef(0);
  const lastVoiceEventAt = useRef(0);

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
        model,
        temperature,
      });
    }
  }, [activeWorkspace, activeChat, model, temperature]);

  useEffect(() => { syncVoiceChat(); }, [syncVoiceChat]);

  // Instant refresh when the overlay finishes a reply
  useEffect(() => {
    const ag = window.antigravity;
    if (!ag) return;
    const offSession = ag.onVoiceSessionEvent?.((ev) => {
      // Phase 8-V2: `mic` events carry the real mic state (not "voice on").
      if (ev?.type === 'mic') {
        setIsVoiceMode(!!ev.mic);
      } else if (ev?.type && ev.type !== 'idle' && ev.type !== 'closed') {
        setIsVoiceMode(true);
      } else {
        setIsVoiceMode(false);
      }
      if (ev?.type === 'pipeline-start') {
        lastVoiceEventAt.current = Date.now();
        voiceStreamRef.current = { text: '', reasoning: '' };
        const seq = typeof (ev as any)?.runSeq === 'number' ? (ev as any).runSeq : voiceRunSeqRef.current + 1;
        voiceRunSeqRef.current = seq;
        const prompt = typeof (ev as any)?.prompt === 'string' ? (ev as any).prompt : '';
        // Replace the live-input ghost with the single canonical user bubble
        // so one utterance never stacks Hello x N.
        if (prompt) {
          setMessages((prev) => {
            const withoutLive = prev.filter((m) => m.id !== 'voice-live-input');
            const last = withoutLive[withoutLive.length - 1];
            if (last && last.role === 'user' && last.content === prompt) return withoutLive;
            return [...withoutLive, { id: `voice-user-${Date.now()}`, role: 'user', content: prompt, timestamp: new Date().toISOString() }];
          });
        }
        setVoiceLiveRun({
          state: null,
          active: true,
          workspaceId: ev.workspaceId || '',
          chatId: ev.chatId || '',
          startedAt: Date.now(),
          runSeq: seq,
        });
      } else if (ev?.type === 'pipeline-state') {
        lastVoiceEventAt.current = Date.now();
        const seq = typeof (ev as any)?.runSeq === 'number' ? (ev as any).runSeq : undefined;
        if (seq === undefined || seq === voiceRunSeqRef.current) {
          const incoming = (ev.state || null) as AppState | null;
          const terminal = incoming?.active_node === 'done' || incoming?.active_node === 'error';
          setVoiceLiveRun((current) => ({
            state: incoming,
            active: terminal ? false : true,
            workspaceId: ev.workspaceId || current?.workspaceId || '',
            chatId: ev.chatId || current?.chatId || '',
            startedAt: current?.startedAt || Date.now(),
            runSeq: current?.runSeq ?? voiceRunSeqRef.current,
          }));
        }
      } else if (ev?.type === 'pipeline-done') {
        lastVoiceEventAt.current = Date.now();
        const seq = typeof (ev as any)?.runSeq === 'number' ? (ev as any).runSeq : undefined;
        // Stale/superseded runs (mic-off abort, watchdog of an old run) must
        // not wipe the live run's trace. Aborted frames never clear either —
        // the run continues server-side and its later frames still arrive.
        const fresh = seq === undefined || seq === voiceRunSeqRef.current;
        const completed = (ev as any)?.status !== 'aborted';
        if (fresh && completed) {
          const error = String((ev as any)?.error || '').trim();
          if (error) {
            setMessages((prev) => {
              const last = prev[prev.length - 1];
              if (last && last.role === 'agent' && last.content === `Error: ${error}`) return prev;
              return [...prev, {
                id: `voice-error-${Date.now()}`,
                role: 'agent',
                content: `Error: ${error}`,
                timestamp: new Date().toISOString(),
              }];
            });
          }
          setVoiceLiveRun((current) => current ? {
            ...current,
            state: (ev.state as AppState | null) || current.state,
            active: false,
          } : null);
        }
      } else if (ev?.type === 'delta' && ev.text) {
        lastVoiceEventAt.current = Date.now();
        // Phase 8-V2: live tokens from the voice run stream into the overlay.
        const kind = ev.kind === 'reasoning' ? 'reasoning' : 'text';
        voiceStreamRef.current[kind] += String(ev.text);
        setVoiceLiveRun((current) => current ? { ...current, active: true } : current);
      }
      if (ev?.type === 'opened') syncVoiceChat();
      if (ev?.type === 'reply') {
        lastVoiceEventAt.current = Date.now();
        voiceStreamRef.current = { text: '', reasoning: '' };
        // Ordered clear: refresh first, THEN drop the trace. Clearing before
        // the fetch resolves was the answer-flash-then-Thinking bug.
        refreshActiveChat();
        window.setTimeout(() => setVoiceLiveRun(null), 800);
      }
      // Live input streaming: the overlay streams STT words as they are heard —
      // append them to the chat in real time (one live user bubble).
      if (ev?.type === 'transcript' && ev.text) {
        const t = String(ev.text);
        setVoiceInputText(t);
        setMessages((prev) => {
          const last = prev[prev.length - 1];
          if (last && last.role === 'user' && last.id === 'voice-live-input') {
            return [...prev.slice(0, -1), { ...last, content: t }];
          }
          return [...prev, { id: 'voice-live-input', role: 'user', content: t, timestamp: new Date().toISOString() }];
        });
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

  // Lightweight polling fallback (covers runs finished while window was hidden).
  // Phase 8: only polls while a run is in progress (SSE pushes live frames;
  // this is purely a hidden-window catch-up) and skips when the tab is hidden.
  useEffect(() => {
    const id = setInterval(() => {
      if (isRunning && !document.hidden) refreshActiveChat();
    }, 4000);
    return () => clearInterval(id);
  }, [isRunning, refreshActiveChat]);

  // ── Keyboard shortcuts: Esc cancels a running pipeline, "?" opens help ──
  const handleCancelPipeline = useCallback(() => {
    cancelledRef.current = true;
    cancelThinkingRef.current = streaming.reasoning || '';
    cancelTextRef.current = streaming.text || '';
    if (activeChat) abortRun(activeChat.id).catch(() => {});
    cancel();
  }, [cancel, streaming.reasoning, streaming.text, activeChat]);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null;
      const typing = !!target && (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA' || target.isContentEditable);
      if (e.key === 'Escape' && isRunning && !typing) {
        handleCancelPipeline();
      } else if (e.key === '?' && !isRunning && !typing) {
        e.preventDefault();
        setShowShortcuts((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [isRunning, handleCancelPipeline]);

  const handleSelectWorkspace = useCallback((ws: WorkspaceInfo) => {
    setActiveWorkspace(ws);
  }, []);

  const handleSelectChat = useCallback((ws: WorkspaceInfo, chat: ChatData) => {
    setActiveWorkspace(ws);
    setActiveChat(chat);
    upsertCwTab(ws.id, chat.id, chat.title || 'Untitled', ws.name);

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
    fetch(apiUrl('/api/session/reset'), { method: 'POST' }).catch(() => {});
  }, [upsertCwTab]);

  const handleChatTitleUpdated = useCallback((chatId: string, newTitle: string) => {
    if (activeChat && activeChat.id === chatId) {
      setActiveChat((c) => c ? { ...c, title: newTitle } : null);
    }
    // Keep the tab title in sync
    setCwTabs((prev) => prev.map((t) => t.chatId === chatId ? { ...t, title: newTitle } : t));
  }, [activeChat]);

  const handleCwActivate = useCallback(async (wsId: string, chatId: string) => {
    try {
      const wsList = await fetchWorkspaces();
      const ws = wsList.find((w) => w.id === wsId);
      if (!ws) return;
      const chat = await fetchChat(wsId, chatId);
      handleSelectChat(ws, chat);
    } catch {}
  }, [handleSelectChat]);

  const handleCwClose = useCallback((wsId: string, chatId: string) => {
    const key = tabKey(wsId, chatId);
    const idx = cwTabs.findIndex((t) => t.workspaceId === wsId && t.chatId === chatId);
    const next = cwTabs.filter((t) => !(t.workspaceId === wsId && t.chatId === chatId));
    setCwTabs(next);
    if (cwActiveKey === key) {
      const neighbor = next[Math.min(idx, next.length - 1)] ?? next[0] ?? null;
      if (neighbor) {
        handleCwActivate(neighbor.workspaceId, neighbor.chatId);
      } else {
        setCwActiveKey(null);
      }
    }
  }, [cwTabs, cwActiveKey, handleCwActivate]);

  const handleCwDrop = useCallback(async (data: string) => {
    // data is "wsId:chatId" from the sidebar drag
    const sep = data.indexOf(':');
    if (sep <= 0) return;
    const wsId = data.slice(0, sep);
    const chatId = data.slice(sep + 1);
    await handleCwActivate(wsId, chatId);
  }, [handleCwActivate]);

  // Keep cwActiveKey in sync when the active chat changes via other paths
  useEffect(() => {
    if (activeWorkspace && activeChat) {
      const key = tabKey(activeWorkspace.id, activeChat.id);
      // eslint-disable-next-line react-hooks/set-state-in-effect -- sync persisted active key from external selection
      if (cwActiveKey !== key) setCwActiveKey(key);
    }
  }, [activeWorkspace?.id, activeChat?.id, cwActiveKey]);

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
      fetch(apiUrl('/api/session/reset'), { method: 'POST' }).catch(() => {});
    }).catch(() => {});
  }, []);

  const handleRewind = useCallback(async (message: ChatMessage) => {
    if (!activeWorkspace || !activeChat || isRunning) return;
    try {
      const result = await rewindChat(activeWorkspace.id, activeChat.id, message.id);
      const chat = await fetchChat(activeWorkspace.id, activeChat.id);
      setMessages(chat.messages || []);
      setDraft(result.prompt || '');
      setNotification(
        result.restore?.conflicts?.length
          ? `Rewound, but ${result.restore.conflicts.length} file(s) could not be restored.`
          : 'Rewound. The message is back in the prompt.',
      );
      setSidebarRefreshTick((t) => t + 1);
    } catch (e) {
      setNotification(e instanceof Error ? e.message : 'Rewind failed.');
    }
  }, [activeWorkspace, activeChat, isRunning]);

  const voiceTraceMatchesActiveChat = !!voiceLiveRun
    && (!voiceLiveRun.workspaceId || voiceLiveRun.workspaceId === activeWorkspace?.id)
    && (!voiceLiveRun.chatId || voiceLiveRun.chatId === activeChat?.id);
  const showVoiceTrace = !isRunning && voiceTraceMatchesActiveChat && !!voiceLiveRun?.state;
  const renderState = showVoiceTrace ? voiceLiveRun?.state : state;
  const renderIsRunning = isRunning || (showVoiceTrace && !!voiceLiveRun?.active);

  const liveParsed = useMemo(() => parseLogToSteps(state?.live_terminal_log ?? ''), [state?.live_terminal_log]);
  const voiceLiveParsed = useMemo(() => parseLogToSteps(voiceLiveRun?.state?.live_terminal_log ?? ''), [voiceLiveRun?.state?.live_terminal_log]);

  const displayMessages = [...messages];
  if (isRunning || isPersisting) {
    const { steps, assistantResponse } = liveParsed;
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
        liveTimeline: streaming.segments,
      },
    });
  }
  if (showVoiceTrace && voiceLiveRun?.state) {
    const { steps, assistantResponse } = voiceLiveParsed;
    const elapsed = voiceLiveRun.active
      ? 'active'
      : Math.round((Date.now() - voiceLiveRun.startedAt) / 1000) + 's';
    // Phase 8-V2: token deltas from the overlay fill the bubble live (the log
    // only has a final [ASSISTANT] line — streaming text shows earlier).
    const vText = voiceStreamRef.current.text || assistantResponse || voiceLiveRun.state.outputs?.agent_report || '';
    displayMessages.push({
      id: 'voice-live-agent-run',
      role: 'agent',
      agentName: 'Assistant',
      content: vText,
      timestamp: new Date().toISOString(),
      duration: elapsed,
      metadata: {
        isTrace: true,
        steps: filterOrchestratorSteps(steps),
        duration: elapsed === 'active' ? undefined : elapsed,
        isStreaming: voiceLiveRun.active && !!voiceStreamRef.current.text,
        streamingReasoning: voiceStreamRef.current.reasoning,
        streamRate: 0,
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
        tabs={cwTabs}
        activeKey={cwActiveKey}
        onActivateTab={handleCwActivate}
        onCloseTab={handleCwClose}
        onDropTab={handleCwDrop}
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
          chatMode={chatModeState}
          onSend={handleSend}
          onCancel={handleCancelPipeline}
          onModelChange={setModel}
          onClearNotification={() => setNotification(null)}
          workspaceId={activeWorkspace?.id ?? null}
          chatId={activeChat?.id ?? null}
          isVoiceMode={isVoiceMode}
          onToggleVoice={toggleVoice}
          voiceInputText={voiceInputText}
          onOpenStats={() => setIsTokenStatsOpen(true)}
          onOpenProject={handleOpenProject}
          onOpenChat={handleCwActivate}
          onOpenAddon={(addonId) => { if (addonId === 'design') setDesignOpen(true); }}
          onRetry={handleSend}
          onRewind={handleRewind}
          draft={draft}
          onDraftConsumed={() => setDraft('')}
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

      {/* OpenDesign module launcher — opens the design room bound to this workspace/chat */}
      <button
        type="button"
        onClick={() => setDesignOpen(true)}
        className="fixed left-2 top-1/2 -translate-y-1/2 z-[85] flex flex-col items-center gap-1 px-1.5 py-3 rounded-xl bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 text-zinc-400 hover:text-zinc-200 transition-colors cursor-pointer shadow-xl"
        title="OpenDesign addon — agent-led UI design iteration for this workspace (straight to the UI subagent loop)"
        style={{ writingMode: 'vertical-rl' }}
      >
        <Palette size={15} />
        <span className="text-[9.5px] font-bold tracking-widest uppercase">Design</span>
      </button>
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
      {designOpen && (
        <Suspense fallback={null}>
          <DesignWorkspace
            embedded
            initialWorkspaceId={activeWorkspace?.id}
            initialChatId={activeChat?.id}
            onClose={() => setDesignOpen(false)}
          />
        </Suspense>
      )}
    </div>
  );
}
