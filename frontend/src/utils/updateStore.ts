// Single source of truth for desktop update progress. Every surface (home,
// coding, settings, toast) reads the same state, so "updating…" can never
// appear in one place while "up to date" sits somewhere else.

export type UpdatePhase =
  | 'idle'
  | 'checking'
  | 'uptodate'
  | 'downloading'
  | 'ready'
  | 'error'
  | 'unsupported';

export interface UpdateState {
  phase: UpdatePhase;
  version: string;
  message: string;
  /** True once the new build is installed and only a restart is missing. */
  restartRequired: boolean;
}

const initial: UpdateState = {
  phase: 'idle',
  version: '',
  message: '',
  restartRequired: false,
};

let state: UpdateState = initial;
const listeners = new Set<(state: UpdateState) => void>();

export function getUpdateState(): UpdateState {
  return state;
}

export function setUpdateState(patch: Partial<UpdateState>): void {
  state = { ...state, ...patch };
  listeners.forEach((listener) => listener(state));
}

export function subscribeUpdate(listener: (state: UpdateState) => void): () => void {
  listeners.add(listener);
  listener(state);
  return () => listeners.delete(listener);
}
