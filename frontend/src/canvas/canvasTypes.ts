export type CanvasScope = 'main' | `ws:${string}`;
export type CanvasAddonId = 'design';

export interface CanvasItemBase {
  id: string;
  x: number;
  y: number;
}

export interface CanvasChatItem extends CanvasItemBase {
  kind: 'chat';
  workspaceId: string;
  chatId: string;
  /** Derived from a live tab/spawned agent rather than manually pinned. */
  auto?: boolean;
  /** True when this auto node is itself an open top-bar tab. */
  isTab?: boolean;
  /** 0 = tab root, 1 = spawned agent, 2 = nested child. */
  depth?: number;
  parentItemId?: string;
  /** Spawned children hidden by the per-parent map cap. */
  hiddenChildren?: number;
}

export interface CanvasProjectItem extends CanvasItemBase {
  kind: 'project';
  workspaceId: string;
  projectId: string;
}

export interface CanvasAddonItem extends CanvasItemBase {
  kind: 'addon';
  workspaceId: string;
  addonId: CanvasAddonId;
  chatId?: string;
}

export type CanvasItem = CanvasChatItem | CanvasProjectItem | CanvasAddonItem;

export type CanvasDraft =
  | { kind: 'chat'; workspaceId: string; chatId: string }
  | { kind: 'project'; workspaceId: string; projectId: string }
  | { kind: 'addon'; workspaceId: string; addonId: CanvasAddonId; chatId?: string };

export function draftId(draft: CanvasDraft): string {
  if (draft.kind === 'chat') return `chat:${draft.workspaceId}:${draft.chatId}`;
  if (draft.kind === 'project') return `project:${draft.workspaceId}:${draft.projectId}`;
  return `addon:${draft.workspaceId}:${draft.addonId}`;
}

export function makeItem(draft: CanvasDraft, x: number, y: number): CanvasItem {
  const base = { id: draftId(draft), x, y };
  if (draft.kind === 'chat') {
    return { ...base, kind: 'chat', workspaceId: draft.workspaceId, chatId: draft.chatId };
  }
  if (draft.kind === 'project') {
    return { ...base, kind: 'project', workspaceId: draft.workspaceId, projectId: draft.projectId };
  }
  return {
    ...base, kind: 'addon', workspaceId: draft.workspaceId,
    addonId: draft.addonId, chatId: draft.chatId,
  };
}

export interface CanvasView {
  x: number;
  y: number;
  scale: number;
}

export const DEFAULT_VIEW: CanvasView = { x: 48, y: 48, scale: 1 };
