import { fetchWorkspaces, fetchChats, fetchChat, addChatMessage, streamPipeline } from '../services/api';
import type { ChatMessage, WorkspaceInfo, AppState, ChatSummary, ChatAttachment } from '../types';
import { parseLogToSteps } from '../utils/logParser';

const MODEL = 'Automatic Fallback';
const TEMP = 0.7;

export async function initHouseWorkspace(): Promise<{ ws: WorkspaceInfo | null; chats: ChatSummary[]; initialMessages: ChatMessage[]; initialChatId: string | null }> {
  const wsList = await fetchWorkspaces();
  const ws = wsList.find((w) => w.isGeneral) || wsList[0] || null;
  if (!ws) return { ws: null, chats: [], initialMessages: [], initialChatId: null };
  const chats = await fetchChats(ws.id);
  let initialMessages: ChatMessage[] = [];
  let initialChatId: string | null = null;
  if (chats.length > 0) {
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
  const { assistantResponse } = parseLogToSteps(log);
  const finalResponse = assistantResponse || state?.outputs?.agent_report || '';
  await addChatMessage(ws.id, chatId, 'agent', finalResponse, { isTrace: true });
  const chat = await fetchChat(ws.id, chatId);
  return chat.messages || [];
}
