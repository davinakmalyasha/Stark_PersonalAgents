import { fetchWorkspaces, fetchChats, fetchChat, addChatMessage, streamPipeline, createChat } from '../services/api';
import type { ChatMessage, WorkspaceInfo, AppState, ChatSummary, ChatAttachment, StreamDelta } from '../types';
import { parseLogToSteps } from '../utils/logParser';

const MODEL = 'Automatic Fallback';
const TEMP = 0.7;

/**
 * Resolve (or create) the dedicated House conversation used by the
 * House Workspace dashboard. Phase 7: House chats live in the real
 * `_house` workspace (workspace_registry.py:16), not localStorage-pinned
 * General chats. Order: chat with the exact pinned title → fresh chat.
 */
export async function resolvePinnedChat(ws: WorkspaceInfo, title: string, known?: ChatSummary[]): Promise<ChatSummary | null> {
  const chats = known ?? (await fetchChats(ws.id).catch(() => []));
  const byTitle = chats.find((c) => c.title === title);
  if (byTitle) {
    return byTitle;
  }
  const created = await createChat(ws.id, title);
  return {
    id: created.id,
    title: created.title || title,
    messageCount: 0,
    model: created.model || 'Automatic Fallback',
    createdAt: created.createdAt,
    updatedAt: created.updatedAt,
  };
}

export async function initHouseWorkspace(pinnedChatTitle?: string, kind: 'general' | 'house' | 'personal' = 'general'): Promise<{ ws: WorkspaceInfo | null; chats: ChatSummary[]; initialMessages: ChatMessage[]; initialChatId: string | null }> {
  const wsList = await fetchWorkspaces();
  // Phase 7: each workspace kind binds to its own virtual workspace — General
  // chat → _general, House → _house, Personal → _personal. NEVER fall back to
  // _house for general (that leaked the house agent into every chat).
  let ws: WorkspaceInfo | null = null;
  if (kind === 'house') {
    ws = wsList.find((w: any) => (w as any).isHouse) || null;
  } else if (kind === 'personal') {
    ws = wsList.find((w: any) => (w as any).isPersonal) || null;
  }
  if (!ws) {
    ws = wsList.find((w: any) => (w as any).isGeneral) || wsList[0] || null;
  }
  if (!ws) return { ws: null, chats: [], initialMessages: [], initialChatId: null };
  const chats = await fetchChats(ws.id);
  let initialMessages: ChatMessage[] = [];
  let initialChatId: string | null = null;

  if (pinnedChatTitle) {
    // Dedicated conversation: never touch the user's other chats.
    const pinned = await resolvePinnedChat(ws, pinnedChatTitle, chats).catch(() => null);
    if (pinned) {
      initialChatId = pinned.id;
      const chat = await fetchChat(ws.id, pinned.id).catch(() => null);
      initialMessages = chat?.messages || [];
    }
  } else if (chats.length > 0) {
    const chat = await fetchChat(ws.id, chats[0].id);
    initialChatId = chat.id;
    initialMessages = chat.messages || [];
  }
  return { ws, chats, initialMessages, initialChatId };
}

export function startHousePipeline(
  prompt: string,
  ws: WorkspaceInfo,
  chatId: string,
  attachments: ChatAttachment[],
  onState: (state: AppState) => void,
  onComplete: () => Promise<void>,
  onError: (err: unknown) => void,
  kind: 'general' | 'house' | 'personal' | 'design' = 'general',
  onDelta?: (delta: StreamDelta) => void,
) {
  // workspaceKind ensures deterministic routing: house→House agent, general→
  // orchestrator, personal→general/orchestrator, design→UI design loop
  // (agents/ui_design_direct). NEVER hardcode 'house'.
  return streamPipeline(
    prompt, MODEL, TEMP,
    (state) => onState(state),
    onComplete,
    onError,
    ws.path, chatId, attachments,
    onDelta,
    kind,
  );
}

export interface FinishMessageOpts {
  /** Streamed reasoning to persist so thought survives history (bug: it vanished). */
  thinking?: string;
  /** Interleaved reasoning/tool segments (Codex-like timeline) persisted for history. */
  liveTimeline?: any[];
  /** Stop (not done): keep the partial as-is and mark it stopped. */
  stopped?: boolean;
  /** Partial streamed text wins over the log-derived response (Stop path). */
  textOverride?: string;
}

export async function finishHouseMessage(ws: WorkspaceInfo, chatId: string, state: AppState | null, durationSec?: number, opts: FinishMessageOpts = {}): Promise<ChatMessage[]> {
  const log = state?.live_terminal_log || '';
  const { steps, assistantResponse } = parseLogToSteps(log);
  const finalResponse = (opts.textOverride || '').trim() || assistantResponse || state?.outputs?.agent_report || '';
  await addChatMessage(ws.id, chatId, 'agent', finalResponse, {
    isTrace: true,
    // Full step list (incl. [SUBAGENT] spawns) — powers the subagent blocks.
    steps,
    // Phase 8-C: persist the same rich snapshot the coding workspace saves so
    // "Worked for Ns" + usage chip + Review survive a reload.
    duration: durationSec ? `${durationSec}s` : undefined,
    artifacts: state?.artifacts,
    usage: state?.token_usage,
    project_ref: (state as any)?.project_ref,
    ...(opts.thinking?.trim() ? { thinking: opts.thinking } : {}),
    ...(opts.liveTimeline && opts.liveTimeline.length ? { liveTimeline: opts.liveTimeline } : {}),
    ...(opts.stopped ? { stopped: true } : {}),
  });
  const chat = await fetchChat(ws.id, chatId);
  return chat.messages || [];
}
