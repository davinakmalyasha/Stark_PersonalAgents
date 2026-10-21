import type { AppState, WorkspaceInfo } from '../types';
import { finishHouseMessage, startHousePipeline } from '../house/houseChatService';

export type CanvasRunKind = 'general' | 'house' | 'personal' | 'design';

/**
 * Send from a canvas card without switching the active chat.
 *
 * The server already writes the user message and owns the run. We keep the
 * stream here only to persist the assistant's final trace when it finishes —
 * the same single-writer path the normal chat uses.
 */
export function sendCanvasChat(
  ws: WorkspaceInfo,
  chatId: string,
  text: string,
  kind: CanvasRunKind = 'general',
): Promise<void> {
  return new Promise((resolve, reject) => {
    let liveState: AppState | null = null;
    const startedAt = Date.now();
    startHousePipeline(
      text,
      ws,
      chatId,
      [],
      (state) => { liveState = state; },
      async () => {
        try {
          const duration = Math.round((Date.now() - startedAt) / 1000);
          await finishHouseMessage(ws, chatId, liveState, duration);
          resolve();
        } catch (err) {
          reject(err instanceof Error ? err : new Error(String(err)));
        }
      },
      (err) => {
        reject(err instanceof Error ? err : new Error(typeof err === 'string' ? err : 'Run failed.'));
      },
      kind,
    );
  });
}

export function kindForWorkspace(ws: WorkspaceInfo): CanvasRunKind {
  if (ws.isHouse) return 'house';
  if (ws.isPersonal) return 'personal';
  return 'general';
}
