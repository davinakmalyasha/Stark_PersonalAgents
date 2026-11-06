// Global "is the app doing work right now?" signal.
// The updater must never relaunch the app mid-run, but it also must not
// require the user to check manually. Surfaces publish their busy state here;
// the updater subscribes and relaunches only when every surface is idle.

const busy = new Map<string, boolean>();
const listeners = new Set<() => void>();

export function setBusy(source: string, value: boolean): void {
  const previous = busy.get(source) ?? false;
  if (previous === value) return;
  busy.set(source, value);
  listeners.forEach((listener) => listener());
}

export function anyBusy(): boolean {
  for (const value of busy.values()) if (value) return true;
  return false;
}

export function onBusyChange(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}
