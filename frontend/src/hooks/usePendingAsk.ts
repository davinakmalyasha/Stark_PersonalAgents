import { useSyncExternalStore } from 'react';
import { fetchPendingQuestions, type AskBatch } from '../services/api';

interface AskState {
  batches: AskBatch[];
  dismissed: Set<string>;
  version: number;
}

let state: AskState = { batches: [], dismissed: new Set(), version: 0 };
const listeners = new Set<() => void>();
let timer: number | null = null;
let fetching = false;
let started = false;

const POLL_MS = 1500;

function publish(next: AskState) {
  state = next;
  listeners.forEach((listener) => listener());
}

function tick() {
  if (typeof document !== 'undefined' && document.hidden) return;
  if (fetching) return;
  fetching = true;
  fetchPendingQuestions()
    .then((next) => {
      const nextIds = next.map((batch) => batch.batch_id).join('|');
      const currentIds = state.batches.map((batch) => batch.batch_id).join('|');
      const present = new Set(next.map((batch) => batch.batch_id));
      const dismissed = new Set([...state.dismissed].filter((id) => present.has(id)));
      const dismissedChanged = dismissed.size !== state.dismissed.size;
      if (nextIds !== currentIds || dismissedChanged) {
        publish({ batches: next, dismissed, version: state.version + 1 });
      }
    })
    .catch(() => {
      /* backend briefly unavailable */
    })
    .finally(() => {
      fetching = false;
    });
}

function start() {
  if (started || typeof window === 'undefined') return;
  started = true;
  void tick();
  timer = window.setInterval(tick, POLL_MS);
  document.addEventListener('visibilitychange', tick);
}

function stop() {
  if (!started) return;
  started = false;
  if (timer) window.clearInterval(timer);
  timer = null;
  document.removeEventListener('visibilitychange', tick);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  start();
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) stop();
  };
}

function getSnapshot(): AskState {
  return state;
}

/** Hide a batch for this client session; the agent keeps waiting. */
export function dismissAskBatch(batchId: string) {
  if (state.dismissed.has(batchId)) return;
  const dismissed = new Set(state.dismissed);
  dismissed.add(batchId);
  publish({ ...state, dismissed, version: state.version + 1 });
}

/**
 * Pending ask batch for one chat. Each mounted chat/card shows its own batch,
 * so two questions from two chats never fight over a global overlay.
 */
export function useAskBatchForChat(chatId?: string | null): AskBatch | null {
  const snapshot = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
  if (!chatId) return null;
  return snapshot.batches.find(
    (batch) => batch.chat_id === chatId && !snapshot.dismissed.has(batch.batch_id),
  ) ?? null;
}
