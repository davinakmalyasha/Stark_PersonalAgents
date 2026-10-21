import type { CanvasItem, CanvasScope, CanvasView } from './canvasTypes';
import { DEFAULT_VIEW } from './canvasTypes';

const ITEMS_PREFIX = 'stark.canvas.items.v2:';
const VIEW_PREFIX = 'stark.canvas.view.v2:';

function safeParse<T>(raw: string | null): T | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return null;
  }
}

export function loadItems(scope: CanvasScope): CanvasItem[] {
  const raw = safeParse<unknown>(localStorage.getItem(ITEMS_PREFIX + scope));
  if (!Array.isArray(raw)) return [];
  return raw.filter((item): item is CanvasItem => {
    if (!item || typeof item !== 'object') return false;
    const candidate = item as Partial<CanvasItem>;
    return (
      typeof candidate.id === 'string' &&
      typeof candidate.x === 'number' &&
      typeof candidate.y === 'number' &&
      (candidate.kind === 'chat' || candidate.kind === 'project' || candidate.kind === 'addon')
    );
  });
}

export function saveItems(scope: CanvasScope, items: CanvasItem[]): void {
  try {
    localStorage.setItem(ITEMS_PREFIX + scope, JSON.stringify(items));
  } catch {
    /* storage may be unavailable */
  }
}

export function loadView(scope: CanvasScope): CanvasView {
  const raw = safeParse<Partial<CanvasView>>(localStorage.getItem(VIEW_PREFIX + scope));
  if (!raw || typeof raw.x !== 'number' || typeof raw.y !== 'number' || typeof raw.scale !== 'number') {
    return { ...DEFAULT_VIEW };
  }
  return {
    x: raw.x,
    y: raw.y,
    scale: Math.min(1.6, Math.max(0.45, raw.scale)),
  };
}

export function saveView(scope: CanvasScope, view: CanvasView): void {
  try {
    localStorage.setItem(VIEW_PREFIX + scope, JSON.stringify(view));
  } catch {
    /* storage may be unavailable */
  }
}
