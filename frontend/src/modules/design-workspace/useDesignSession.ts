import { useCallback, useEffect, useRef, useState } from 'react';
import type { AppState, ChatAttachment, ChatMessage, TokenUsage, WorkspaceInfo } from '../../types';
import type { WorkspaceFileEntry } from '../../services/api';
import {
  addChatMessage,
  createChat,
  fetchChat,
  fetchChats,
  fetchWorkspaceFile,
  fetchWorkspaceFiles,
  fetchWorkspaces,
  uploadChatAttachments,
} from '../../services/api';
import { startHousePipeline } from '../../house/houseChatService';
import { parseLogToSteps } from '../../utils/logParser';

export interface DesignFile {
  path: string;
  name: string;
}

function parseDesignHash(): { wsId: string; chatId: string } {
  try {
    const h = window.location.hash || '';
    const qi = h.indexOf('?');
    if (!h.startsWith('#/design') || qi < 0) return { wsId: '', chatId: '' };
    const params = new URLSearchParams(h.slice(qi + 1));
    return { wsId: params.get('ws') || '', chatId: params.get('chat') || '' };
  } catch {
    return { wsId: '', chatId: '' };
  }
}

export interface DesignSessionOptions {
  /** When set, bind directly to this workspace (addon mode). */
  workspaceId?: string;
  chatId?: string;
  /** Addon mode: do not rewrite the URL hash while binding. */
  embedded?: boolean;
}

/**
 * Session state for the OpenDesign addon.
 *
 * Binds to a REAL project workspace (picked, deep-linked, or passed by the
 * workspace addon host), runs prompts with workspaceKind='design' (straight to
 * the UI design loop — never orchestrator/Dev), and tracks the workspace's
 * design files for preview.
 */
export function useDesignSession(opts: DesignSessionOptions = {}) {
  const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [workspace, setWorkspace] = useState<WorkspaceInfo | null>(null);
  const [chatId, setChatId] = useState<string | null>(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [isRunning, setIsRunning] = useState(false);
  const [liveState, setLiveState] = useState<AppState | null>(null);
  const [notification, setNotification] = useState<string | null>(null);
  const [files, setFiles] = useState<WorkspaceFileEntry[]>([]);
  const [usage, setUsage] = useState<TokenUsage | null>(null);
  const [booted, setBooted] = useState(false);
  const abortRef = useRef<AbortController | null>(null);
  const runningRef = useRef(false);
  const startedAtRef = useRef<number | null>(null);
  const liveStateRef = useRef<AppState | null>(null);

  const refreshFiles = useCallback(async (wsId: string) => {
    try {
      setFiles(await fetchWorkspaceFiles(wsId));
    } catch {
      setFiles([]);
    }
  }, []);

  const loadChat = useCallback(async (wsId: string, cId: string) => {
    try {
      const chat = await fetchChat(wsId, cId);
      setMessages(chat.messages || []);
      setUsage(chat.token_usage ?? null);
    } catch {
      setMessages([]);
    }
  }, []);

  // ── Boot: workspaces → deep-link or picker ──
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const wsList = await fetchWorkspaces();
        if (cancelled) return;
        setWorkspaces(wsList);
        const linked = opts.workspaceId
          ? { wsId: opts.workspaceId, chatId: opts.chatId || '' }
          : parseDesignHash();
        const { wsId, chatId: deepChatId } = linked;
        const found = wsList.find((w) => w.id === wsId) || null;
        // Design is an addon for real project folders. General/House/Personal/
        // DFD are virtual workspaces with no filesystem root; showing the
        // picker is more useful than binding to a 404.
        const ws = found && found.path && !found.isGeneral && !found.isHouse && !found.isPersonal
          ? found
          : null;
        if (ws && deepChatId) {
          setWorkspace(ws);
          setChatId(deepChatId);
          await loadChat(ws.id, deepChatId);
          await refreshFiles(ws.id);
        } else if (ws) {
          // Workspace linked but no chat — open its first chat or create one.
          const chats = await fetchChats(ws.id).catch(() => []);
          let target = chats[0]?.id || '';
          if (!target) {
            const created = await createChat(ws.id, 'Design').catch(() => null);
            if (created) target = created.id;
          }
          if (cancelled) return;
          setWorkspace(ws);
          if (target) {
            setChatId(target);
            await loadChat(ws.id, target);
          }
          await refreshFiles(ws.id);
        }
      } catch {
        /* rooms shows the picker/empty state */
      } finally {
        if (!cancelled) setBooted(true);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loadChat, refreshFiles, opts.workspaceId, opts.chatId]);

  const bindWorkspace = useCallback(async (ws: WorkspaceInfo) => {
    setWorkspace(ws);
    setChatId(null);
    setMessages([]);
    const chats = await fetchChats(ws.id).catch(() => []);
    let target = chats[0]?.id || '';
    if (!target) {
      const created = await createChat(ws.id, 'Design').catch(() => null);
      if (created) target = created.id;
    }
    if (target) {
      setChatId(target);
      await loadChat(ws.id, target);
    }
    await refreshFiles(ws.id);
    if (!opts.embedded) {
      try {
        window.history.replaceState(null, '', `#/design?ws=${encodeURIComponent(ws.id)}&chat=${encodeURIComponent(target)}`);
      } catch { /* hash only */ }
    }
  }, [loadChat, refreshFiles, opts.embedded]);

  const send = useCallback(async (prompt: string, files: File[]) => {
    if (!workspace || !chatId || runningRef.current) return;
    if (!prompt.trim() && !files.length) return;
    setNotification(null);
    let attachments: ChatAttachment[] = [];
    if (files.length) {
      try {
        attachments = await uploadChatAttachments(workspace.id, chatId, files);
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
    runningRef.current = true;
    startedAtRef.current = Date.now();
    setIsRunning(true);
    setLiveState(null);
    liveStateRef.current = null;

    const ws = workspace;
    const cId = chatId;
    abortRef.current = startHousePipeline(
      prompt.trim(),
      ws,
      cId,
      attachments,
      (st) => {
        liveStateRef.current = st;
        setLiveState(st);
      },
      async () => {
        try {
          const state = liveStateRef.current;
          const log = state?.live_terminal_log || '';
          const { steps, assistantResponse } = parseLogToSteps(log);
          const finalResponse = assistantResponse || (state?.outputs as any)?.agent_report || '';
          const duration = startedAtRef.current
            ? Math.round((Date.now() - startedAtRef.current) / 1000) + 's'
            : undefined;
          await addChatMessage(ws.id, cId, 'agent', finalResponse, {
            isTrace: true,
            steps,
            duration,
            artifacts: state?.artifacts,
          });
          const chat = await fetchChat(ws.id, cId);
          setMessages(chat.messages || []);
          setUsage(chat.token_usage ?? null);
          await refreshFiles(ws.id);
        } catch { /* keep the live message visible if persistence fails */ }
        runningRef.current = false;
        setIsRunning(false);
        setLiveState(null);
      },
      (err) => {
        setNotification(typeof err === 'string' ? err : 'Error occurred.');
        setLiveState(null);
        runningRef.current = false;
        setIsRunning(false);
      },
      'design',
    );
  }, [workspace, chatId, refreshFiles]);

  const cancel = useCallback(() => {
    abortRef.current?.abort();
    runningRef.current = false;
    setIsRunning(false);
    setLiveState(null);
  }, []);

  return {
    workspaces, workspace, chatId, messages, isRunning, liveState,
    notification, files, usage, booted, bindWorkspace, send, cancel,
    refreshFiles: () => workspace && refreshFiles(workspace.id),
    fetchFile: (path: string) => (workspace ? fetchWorkspaceFile(workspace.id, path) : Promise.resolve(null)),
  };
}

export type DesignSession = ReturnType<typeof useDesignSession>;
