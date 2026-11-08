import type { ChatSummary, SessionTab, WorkspaceInfo } from '../types';
import type { CanvasChatItem } from './canvasTypes';

/**
 * Live canvas graph: open top-bar tabs are roots, spawned chats hang below
 * their parent, and group/project cards stay manual. Everything here is
 * derived — the only persisted canvas state is a card's position.
 */

const ROOT_WIDTH = 468;
const CHILD_WIDTH = 384;
const ROOT_GAP = 120;
const CHILD_GAP = 64;
const LEVEL_GAP = 470;
const MAX_DEPTH = 3;
const MAX_NODES = 60;
const MAX_CHILDREN = 12;

interface GraphNode {
  key: string;
  workspaceId: string;
  chat: ChatSummary;
  depth: number;
  parentKey: string | null;
  isTab: boolean;
  children: string[];
  hiddenChildren: number;
}

export interface CanvasGraphEdge {
  parentId: string;
  childId: string;
}

export interface CanvasGraph {
  items: CanvasChatItem[];
  edges: CanvasGraphEdge[];
  rootCount: number;
  agentCount: number;
}

function chatKey(workspaceId: string, chatId: string): string {
  return `${workspaceId}:${chatId}`;
}

function itemId(key: string): string {
  return `chat:${key}`;
}

function nodeWidth(depth: number): number {
  return depth === 0 ? ROOT_WIDTH : CHILD_WIDTH;
}

function syntheticChat(tab: SessionTab): ChatSummary {
  return {
    id: tab.chatId,
    title: tab.title || 'Untitled',
    messageCount: 0,
    model: '',
    createdAt: tab.updatedAt,
    updatedAt: tab.updatedAt,
    parentChatId: tab.parentChatId,
    parentWorkspaceId: tab.parentWorkspaceId,
  };
}

export function buildCanvasGraph(
  tabs: SessionTab[],
  chatsByWs: Record<string, ChatSummary[]>,
  workspaces: WorkspaceInfo[],
): CanvasGraph {
  const byKey = new Map<string, { workspaceId: string; chat: ChatSummary }>();
  for (const [workspaceId, chats] of Object.entries(chatsByWs)) {
    for (const chat of chats) {
      byKey.set(chatKey(workspaceId, chat.id), { workspaceId, chat });
    }
  }

  const tabByKey = new Map<string, SessionTab>();
  for (const tab of tabs) tabByKey.set(chatKey(tab.workspaceId, tab.chatId), tab);

  const nodes = new Map<string, GraphNode>();
  for (const tab of tabs) {
    const key = chatKey(tab.workspaceId, tab.chatId);
    if (nodes.has(key)) continue;
    const stored = byKey.get(key);
    nodes.set(key, {
      key,
      workspaceId: tab.workspaceId,
      chat: stored?.chat ?? syntheticChat(tab),
      depth: 0,
      parentKey: null,
      isTab: true,
      children: [],
      hiddenChildren: 0,
    });
  }

  // If a tab's parent tab is open, it belongs under that tab, not in the root
  // row. Every tab stays visible either way.
  const rootKeys: string[] = [];
  for (const tab of tabs) {
    const key = chatKey(tab.workspaceId, tab.chatId);
    const parentKey = tab.parentChatId
      ? chatKey(tab.parentWorkspaceId || tab.workspaceId, tab.parentChatId)
      : '';
    if (parentKey && parentKey !== key && tabByKey.has(parentKey)) {
      nodes.get(key)!.parentKey = parentKey;
    } else {
      rootKeys.push(key);
    }
  }

  const childrenOf = (parentKey: string): Array<{ key: string; workspaceId: string; chat: ChatSummary }> => {
    const parent = nodes.get(parentKey);
    if (!parent) return [];
    const parentWs = parent.workspaceId;
    const parentChatId = parent.chat.id;
    const result: Array<{ key: string; workspaceId: string; chat: ChatSummary }> = [];
    for (const [key, entry] of byKey) {
      if (key === parentKey) continue;
      if (entry.chat.parentChatId !== parentChatId) continue;
      const declaredParentWs = entry.chat.parentWorkspaceId || entry.workspaceId;
      if (declaredParentWs !== parentWs) continue;
      result.push({ key, workspaceId: entry.workspaceId, chat: entry.chat });
    }
    return result.sort((left, right) => {
      const a = new Date(left.chat.updatedAt || 0).getTime();
      const b = new Date(right.chat.updatedAt || 0).getTime();
      return a - b;
    });
  };

  const visited = new Set<string>(rootKeys);
  let total = nodes.size;

  const visit = (parentKey: string, depth: number) => {
    if (depth >= MAX_DEPTH) return;
    const parent = nodes.get(parentKey);
    if (!parent) return;
    const candidates = childrenOf(parentKey);
    let added = 0;
    const childDepth = depth + 1;
    for (const candidate of candidates) {
      if (total >= MAX_NODES) break;
      if (added >= MAX_CHILDREN) break;
      if (visited.has(candidate.key)) continue;
      const existing = nodes.get(candidate.key);
      const isTab = tabByKey.has(candidate.key);
      const node: GraphNode = existing ?? {
        key: candidate.key,
        workspaceId: candidate.workspaceId,
        chat: candidate.chat,
        depth: childDepth,
        parentKey,
        isTab,
        children: [],
        hiddenChildren: 0,
      };
      node.depth = childDepth;
      node.parentKey = parentKey;
      node.isTab = isTab;
      nodes.set(candidate.key, node);
      visited.add(candidate.key);
      parent.children.push(candidate.key);
      total += existing ? 0 : 1;
      added += 1;
      visit(candidate.key, childDepth);
    }
    parent.hiddenChildren = Math.max(0, candidates.length - added);
  };

  for (const rootKey of rootKeys) {
    visited.add(rootKey);
    visit(rootKey, 0);
  }

  // Any tab that was not claimed by an open parent and was skipped by the cap
  // must still be on the map. Promote it to a root.
  for (const tab of tabs) {
    const key = chatKey(tab.workspaceId, tab.chatId);
    if (nodes.has(key) && !visited.has(key)) {
      const node = nodes.get(key)!;
      node.parentKey = null;
      node.depth = 0;
      rootKeys.push(key);
      visited.add(key);
    }
  }

  // ── Tidy tree layout ──
  const subtreeWidth = new Map<string, number>();
  const widthOf = (key: string): number => {
    const cached = subtreeWidth.get(key);
    if (cached != null) return cached;
    const node = nodes.get(key);
    if (!node) return 0;
    const own = nodeWidth(node.depth);
    if (node.children.length === 0) {
      subtreeWidth.set(key, own);
      return own;
    }
    const childTotal = node.children.reduce((sum, child, index) => (
      sum + widthOf(child) + (index > 0 ? CHILD_GAP : 0)
    ), 0);
    const width = Math.max(own, childTotal);
    subtreeWidth.set(key, width);
    return width;
  };

  const positions = new Map<string, { x: number; y: number }>();
  const place = (key: string, left: number): number => {
    const node = nodes.get(key);
    if (!node) return left;
    const width = widthOf(key);
    const own = nodeWidth(node.depth);
    const y = node.depth * LEVEL_GAP;
    if (node.children.length === 0) {
      positions.set(key, { x: left + (width - own) / 2, y });
      return left + width;
    }
    const childWidths = node.children.reduce((sum, child, index) => (
      sum + widthOf(child) + (index > 0 ? CHILD_GAP : 0)
    ), 0);
    let cursor = left + Math.max(0, (width - childWidths) / 2);
    for (const child of node.children) {
      cursor = place(child, cursor) + CHILD_GAP;
    }
    positions.set(key, { x: left + (width - own) / 2, y });
    return left + width;
  };

  let cursor = 0;
  for (const rootKey of rootKeys) {
    cursor = place(rootKey, cursor) + ROOT_GAP;
  }
  const forestWidth = Math.max(0, cursor - ROOT_GAP);
  const shiftX = 72 - forestWidth / 2;
  const shiftY = 64;

  const items: CanvasChatItem[] = [];
  const edges: CanvasGraphEdge[] = [];
  for (const [key, node] of nodes) {
    const position = positions.get(key);
    if (!position) continue;
    items.push({
      id: itemId(key),
      kind: 'chat',
      workspaceId: node.workspaceId,
      chatId: node.chat.id,
      x: Math.round(position.x + shiftX),
      y: Math.round(position.y + shiftY),
      auto: true,
      isTab: node.isTab,
      depth: node.depth,
      parentItemId: node.parentKey ? itemId(node.parentKey) : undefined,
      hiddenChildren: node.hiddenChildren,
    });
    if (node.parentKey) {
      edges.push({ parentId: itemId(node.parentKey), childId: itemId(key) });
    }
  }

  // Workspace metadata is used by the map header; keep the function signature
  // intentional even though layout does not need it.
  void workspaces;

  return {
    items,
    edges,
    rootCount: rootKeys.length,
    agentCount: items.filter((item) => !item.isTab).length,
  };
}
