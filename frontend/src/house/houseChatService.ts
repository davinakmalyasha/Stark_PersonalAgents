import { fetchWorkspaces, fetchChats, fetchChat, addChatMessage, streamPipeline, createChat } from '../services/api';
import type { ChatMessage, WorkspaceInfo, AppState, ChatSummary, ChatAttachment } from '../types';
import { parseLogToSteps } from '../utils/logParser';

const MODEL = 'Automatic Fallback';
const TEMP = 0.7;

const HOUSE_PIN_KEY = 'hw-house-chat-id';

/**
 * Resolve (or create) the dedicated House conversation used by the
 * House Workspace dashboard. Order: saved chat id (localStorage, survives
 * renames) → chat with the exact pinned title → fresh chat.
 */
export async function resolvePinnedChat(ws: WorkspaceInfo, title: string, known?: ChatSummary[]): Promise<ChatSummary | null> {
  const saved = window.localStorage.getItem(HOUSE_PIN_KEY);
  if (saved) {
    const chat = await fetchChat(ws.id, saved).catch(() => null);
    if (chat?.id) {
      return {
        id: chat.id,
        title: chat.title || title,
        messageCount: chat.messages?.length ?? 0,
        model: chat.model || 'Automatic Fallback',
        createdAt: chat.createdAt,
        updatedAt: chat.updatedAt,
      };
    }
  }
  const chats = known ?? (await fetchChats(ws.id).catch(() => []));
  const byTitle = chats.find((c) => c.title === title);
  if (byTitle) {
    window.localStorage.setItem(HOUSE_PIN_KEY, byTitle.id);
    return byTitle;
  }
  const created = await createChat(ws.id, title);
  window.localStorage.setItem(HOUSE_PIN_KEY, created.id);
  return {
    id: created.id,
    title: created.title || title,
    messageCount: 0,
    model: created.model || 'Automatic Fallback',
    createdAt: created.createdAt,
    updatedAt: created.updatedAt,
  };
}

export async function initHouseWorkspace(pinnedChatTitle?: string): Promise<{ ws: WorkspaceInfo | null; chats: ChatSummary[]; initialMessages: ChatMessage[]; initialChatId: string | null }> {
  const wsList = await fetchWorkspaces();
  const ws = wsList.find((w) => w.isGeneral) || wsList[0] || null;
  if (!ws) return { ws: null, chats: [], initialMessages: [], initialChatId: null };
  const chats = await fetchChats(ws.id);
  let initialMessages: ChatMessage[] = [];
  let initialChatId: string | null = null;

  if (pinnedChatTitle) {
    // Dedicated House conversation: never touch the user's other chats.
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
) {
  return streamPipeline(
    prompt, MODEL, TEMP,
    (state) => onState(state),
    onComplete,
    onError,
    ws.path, chatId, attachments,
  );
}

export async function finishHouseMessage(ws: WorkspaceInfo, chatId: string, state: AppState | null): Promise<ChatMessage[]> {
  const log = state?.live_terminal_log || '';
  const { steps, assistantResponse } = parseLogToSteps(log);
  const finalResponse = assistantResponse || state?.outputs?.agent_report || '';
  await addChatMessage(ws.id, chatId, 'agent', finalResponse, {
    isTrace: true,
    // Full step list (incl. [SUBAGENT] spawns) — powers the subagent blocks.
    steps,
  });
  const chat = await fetchChat(ws.id, chatId);
  return chat.messages || [];
}
