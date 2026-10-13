import type { ChatSummary, WorkspaceInfo } from '../types';

export interface HierarchyChat extends ChatSummary {
  workspaceId: string;
  workspaceName: string;
  /** Depth in a same-workspace parent chain (0 = top-level). */
  depth: number;
  /** True when this chat was auto-spawned by a higher agent. */
  isSpawned: boolean;
  /** Resolved parent title for badges (cross-workspace aware). */
  parentTitle?: string;
  parentWsName?: string;
}

export interface ChatFamily {
  top: HierarchyChat;
  /** Direct + transitive children, oldest-first for stable nesting. */
  children: HierarchyChat[];
}

/**
 * Global lookup across every workspace: chatId -> { title, ws }.
 * Lets a child in a project workspace resolve a parent that lives in
 * _general (orchestrator → workspace-agent fan-out) and vice versa.
 */
export function buildGlobalLookup(
  workspaces: WorkspaceInfo[],
  chatsByWs: Record<string, ChatSummary[]>,
): Map<string, { title: string; wsId: string; wsName: string }> {
  const map = new Map<string, { title: string; wsId: string; wsName: string }>();
  for (const ws of workspaces) {
    for (const c of chatsByWs[ws.id] ?? []) {
      if (!map.has(c.id)) {
        map.set(c.id, { title: c.title || 'Untitled', wsId: ws.id, wsName: ws.name });
      }
    }
  }
  return map;
}

/** A chat counts as spawned when the backend recorded a parent link. */
export function isSpawnedChat(c: ChatSummary): boolean {
  return Boolean((c as ChatSummary).parentChatId);
}

/**
 * Split one workspace's chats into top-level parents + nested children.
 * Same-workspace chains nest by depth (supports orchestrator → dev →
 * sub-delegation). Cross-workspace children (parent lives elsewhere) stay
 * top-level here but carry parentTitle/parentWsName badges — the caller
 * (e.g. HouseSidebar general view) can additionally render them nested
 * under their true parent via `collectSpawnedChildrenOf`.
 */
export function groupWorkspaceChats(
  ws: WorkspaceInfo,
  chats: ChatSummary[],
  lookup: Map<string, { title: string; wsId: string; wsName: string }>,
): { tops: HierarchyChat[]; childrenByParent: Map<string, HierarchyChat[]> } {
  const byId = new Map(chats.map((c) => [c.id, c] as const));
  const childrenByParent = new Map<string, HierarchyChat[]>();
  const tops: HierarchyChat[] = [];

  const toHierarchy = (c: ChatSummary, depth: number): HierarchyChat => {
    const parentId = (c as ChatSummary).parentChatId;
    const parentWsId = (c as ChatSummary).parentWorkspaceId;
    const resolved = (parentId && lookup.get(parentId)) || null;
    return {
      ...c,
      workspaceId: ws.id,
      workspaceName: ws.name,
      depth,
      isSpawned: isSpawnedChat(c),
      parentTitle: resolved?.title,
      parentWsName:
        resolved?.wsName ??
        (parentWsId ? undefined : undefined),
    };
  };

  // First pass: separate tops from same-workspace children.
  const pending: ChatSummary[] = [];
  for (const c of chats) {
    const p = (c as ChatSummary).parentChatId;
    if (p && byId.has(p)) pending.push(c);
    else tops.push(toHierarchy(c, 0));
  }

  // Resolve nesting depth for same-workspace chains (guard against cycles).
  const depthOf = (c: ChatSummary): number => {
    let d = 0;
    let cur: ChatSummary | undefined = c;
    const seen = new Set<string>();
    while (cur) {
      const p = (cur as ChatSummary).parentChatId;
      if (!p || !byId.has(p) || seen.has(p)) break;
      seen.add(p);
      d += 1;
      cur = byId.get(p);
      if (d > 8) break;
    }
    return Math.min(d, 8);
  };

  for (const c of pending) {
    const p = (c as ChatSummary).parentChatId as string;
    const arr = childrenByParent.get(p) || [];
    arr.push(toHierarchy(c, depthOf(c)));
    childrenByParent.set(p, arr);
  }
  // Oldest-first inside each nest so the spawn order reads top-down.
  for (const arr of childrenByParent.values()) {
    arr.sort((a, b) => new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime());
  }
  return { tops, childrenByParent };
}

/**
 * All chats (any workspace) spawned directly or transitively from a parent
 * chat id. Powers the "N spawned" nest under an orchestrator chat in the
 * sidebar, even though the children live in project workspaces.
 */
export function collectSpawnedChildrenOf(
  parentChatId: string,
  workspaces: WorkspaceInfo[],
  chatsByWs: Record<string, ChatSummary[]>,
  lookup: Map<string, { title: string; wsId: string; wsName: string }>,
  maxDepth = 3,
): HierarchyChat[] {
  const out: HierarchyChat[] = [];
  const seen = new Set<string>([parentChatId]);
  let frontier = [parentChatId];
  for (let depth = 1; depth <= maxDepth && frontier.length > 0; depth += 1) {
    const next: string[] = [];
    for (const ws of workspaces) {
      for (const c of chatsByWs[ws.id] ?? []) {
        const p = (c as ChatSummary).parentChatId;
        if (p && frontier.includes(p) && !seen.has(c.id)) {
          seen.add(c.id);
          const resolved = lookup.get(p);
          out.push({
            ...c,
            workspaceId: ws.id,
            workspaceName: ws.name,
            depth,
            isSpawned: true,
            parentTitle: resolved?.title,
            parentWsName: resolved?.wsName,
          });
          next.push(c.id);
        }
      }
    }
    frontier = next;
  }
  out.sort((a, b) => new Date(a.createdAt).getTime() - new Date(b.createdAt).getTime());
  return out;
}

/** Short "from X" label for a spawned row. */
export function spawnedFromLabel(c: HierarchyChat): string | null {
  if (!c.isSpawned) return null;
  if (c.parentTitle) return `from ${c.parentTitle}`;
  return 'auto-spawned';
}
