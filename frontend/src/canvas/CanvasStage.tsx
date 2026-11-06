import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { DragEvent as ReactDragEvent, PointerEvent as ReactPointerEvent, WheelEvent as ReactWheelEvent } from 'react';
import { FolderPlus, Minus, Plus, RotateCcw, Scan, Waypoints, X } from 'lucide-react';
import type { ChatSummary, GroupProjectRow, SessionTab, WorkspaceInfo } from '../types';
import type { BoardAgent } from '../types/board.types';
import CanvasAddMenu from './CanvasAddMenu';
import type { AddAddonEntry, AddChatEntry } from './CanvasAddMenu';
import CanvasItemCard from './CanvasItemCard';
import type { CanvasAddonId, CanvasDraft, CanvasItem, CanvasScope, CanvasView } from './canvasTypes';
import { draftId, makeItem } from './canvasTypes';
import { loadItems, loadView, saveItems, saveView } from './canvasStorage';
import { buildCanvasGraph } from './canvasGraph';
import './canvas.css';

export interface CanvasUsage {
  input?: number;
  output?: number;
  cost?: number;
  context?: number;
}

interface Props {
  scope: CanvasScope;
  workspaces: WorkspaceInfo[];
  chatsByWs: Record<string, ChatSummary[]>;
  activeWorkspaceId?: string | null;
  activeChatId?: string | null;
  agents: BoardAgent[];
  isRunning: boolean;
  elapsed?: string;
  projects?: GroupProjectRow[];
  onOpenChat?: (workspaceId: string, chatId: string) => void;
  onOpenGroup?: (projectId: string) => void;
  onOpenAddon?: (workspaceId: string, addonId: CanvasAddonId, chatId?: string) => void;
  onSendToChat?: (workspaceId: string, chatId: string, text: string) => Promise<void> | void;
  /** Open top-bar tabs; when present they are auto-added as map roots. */
  tabs?: SessionTab[];
  runningTabId?: string | null;
  variant?: 'background' | 'map';
  usage?: CanvasUsage;
  toolCalls?: number;
  errors?: number;
  onCloseTab?: (tabId: string) => void;
  onCloseMap?: () => void;
}

type Interaction =
  | { type: 'pan'; sx: number; sy: number; vx: number; vy: number }
  | { type: 'item'; id: string; sx: number; sy: number; ix: number; iy: number; moved: boolean; item: CanvasItem };

interface CardSize {
  w: number;
  h: number;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function isProjectWorkspace(ws: WorkspaceInfo): boolean {
  return !ws.isGeneral && !ws.isHouse && !ws.isPersonal
    && !(ws as { isDfd?: boolean }).isDfd;
}

function compactNumber(value?: number): string {
  if (!value || value < 1) return '0';
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(1)}M`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(value >= 10_000 ? 0 : 1)}k`;
  return String(Math.round(value));
}

function sizesEqual(left: Record<string, CardSize>, right: Record<string, CardSize>): boolean {
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  if (leftKeys.length !== rightKeys.length) return false;
  for (const key of leftKeys) {
    const a = left[key];
    const b = right[key];
    if (!b || a.w !== b.w || a.h !== b.h) return false;
  }
  return true;
}

export default function CanvasStage({
  scope, workspaces, chatsByWs, activeWorkspaceId, activeChatId,
  agents, isRunning, elapsed, projects = [],
  onOpenChat, onOpenGroup, onOpenAddon, onSendToChat,
  tabs, runningTabId, variant = 'background', usage, toolCalls, errors,
  onCloseTab, onCloseMap,
}: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [items, setItems] = useState<CanvasItem[]>(() => loadItems(scope));
  const [view, setView] = useState<CanvasView>(() => loadView(scope));
  const [expanded, setExpanded] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [addAnchor, setAddAnchor] = useState<{ left: number; top: number } | null>(null);
  const [interaction, setInteraction] = useState<Interaction | null>(null);
  const [sizes, setSizes] = useState<Record<string, CardSize>>({});
  const [viewSmooth, setViewSmooth] = useState(false);
  const smoothTimer = useRef<number | null>(null);
  const itemsRef = useRef(items);
  const viewRef = useRef(view);
  const autoSigRef = useRef('');
  const autoSyncedRef = useRef(false);
  const fittedRef = useRef(false);

  const commitItems = useCallback((next: CanvasItem[]) => {
    itemsRef.current = next;
    setItems(next);
    saveItems(scope, next);
  }, [scope]);

  const commitView = useCallback((next: CanvasView, smooth = false) => {
    if (smoothTimer.current) window.clearTimeout(smoothTimer.current);
    if (smooth) {
      setViewSmooth(true);
      smoothTimer.current = window.setTimeout(() => setViewSmooth(false), 340);
    } else {
      setViewSmooth(false);
    }
    viewRef.current = next;
    setView(next);
    saveView(scope, next);
  }, [scope]);

  useEffect(() => () => {
    if (smoothTimer.current) window.clearTimeout(smoothTimer.current);
  }, []);

  const openAddMenu = useCallback(() => {
    if (addOpen) {
      setAddOpen(false);
      return;
    }
    const rect = containerRef.current?.getBoundingClientRect();
    setAddAnchor({
      left: (rect?.left ?? 0) + 12,
      top: (rect?.top ?? 0) + (variant === 'map' ? 76 : 48),
    });
    setAddOpen(true);
  }, [addOpen, variant]);

  const scopedWorkspaces = useMemo(() => {
    const wsId = scope.startsWith('ws:') ? scope.slice(3) : '';
    return wsId ? workspaces.filter((w) => w.id === wsId) : workspaces;
  }, [scope, workspaces]);

  const chatEntries = useMemo<AddChatEntry[]>(() => {
    const rows: AddChatEntry[] = [];
    for (const ws of scopedWorkspaces) {
      for (const chat of chatsByWs[ws.id] ?? []) rows.push({ workspace: ws, chat });
    }
    rows.sort((a, b) => new Date(b.chat.updatedAt).getTime() - new Date(a.chat.updatedAt).getTime());
    return rows;
  }, [scopedWorkspaces, chatsByWs]);

  const chatIndex = useMemo(() => {
    const map = new Map<string, AddChatEntry>();
    for (const entry of chatEntries) {
      map.set(`${entry.workspace.id}:${entry.chat.id}`, entry);
    }
    return map;
  }, [chatEntries]);

  const entryByKey = useMemo(() => {
    const map = new Map<string, AddChatEntry>();
    for (const [workspaceId, chats] of Object.entries(chatsByWs)) {
      const workspace = workspaces.find((candidate) => candidate.id === workspaceId);
      if (!workspace) continue;
      for (const chat of chats) map.set(`${workspaceId}:${chat.id}`, { workspace, chat });
    }
    return map;
  }, [chatsByWs, workspaces]);

  const childrenByParent = useMemo(() => {
    const map = new Map<string, ChatSummary[]>();
    for (const entry of entryByKey.values()) {
      const parentId = entry.chat.parentChatId;
      if (!parentId) continue;
      const parentKey = `${entry.chat.parentWorkspaceId || entry.workspace.id}:${parentId}`;
      const list = map.get(parentKey) ?? [];
      list.push(entry.chat);
      map.set(parentKey, list);
    }
    for (const list of map.values()) {
      list.sort((a, b) => new Date(b.updatedAt).getTime() - new Date(a.updatedAt).getTime());
    }
    return map;
  }, [entryByKey]);

  const graph = useMemo(
    () => buildCanvasGraph(tabs ?? [], chatsByWs, workspaces),
    [tabs, chatsByWs, workspaces],
  );

  // Auto-map sync: roots + spawned agents are derived; existing positions are
  // preserved so a session poll never makes the map jump.
  useEffect(() => {
    if (!tabs) return;
    const signature = graph.items
      .map((item) => `${item.id}|${item.parentItemId || ''}|${item.isTab ? 1 : 0}|${item.depth || 0}`)
      .join('~');
    if (autoSyncedRef.current && signature === autoSigRef.current) return;
    autoSigRef.current = signature;
    autoSyncedRef.current = true;

    const current = itemsRef.current;
    const isAutoChat = (item: CanvasItem): item is CanvasItem & { kind: 'chat'; auto: true } =>
      item.kind === 'chat' && !!item.auto;
    const manual = current.filter((item) => !isAutoChat(item));
    const manualIds = new Set(manual.map((item) => item.id));
    const previousAuto = new Map(current.filter(isAutoChat).map((item) => [item.id, item]));
    const nextAuto = graph.items
      .filter((item) => !manualIds.has(item.id))
      .map((item) => {
        const existing = previousAuto.get(item.id);
        if (!existing) return item;
        const structuralChange = existing.depth !== item.depth
          || existing.parentItemId !== item.parentItemId
          || existing.isTab !== item.isTab;
        return structuralChange
          ? item
          : { ...item, x: existing.x, y: existing.y };
      });
    const next = [...manual, ...nextAuto];
    const unchanged = next.length === current.length
      && next.every((item, index) => item.id === current[index]?.id && item.x === current[index]?.x && item.y === current[index]?.y);
    if (!unchanged) commitItems(next);
  }, [graph, tabs, commitItems]);

  // Measure real card boxes so connectors always meet the border, including
  // when a card is expanded or a streaming task wraps to a second line.
  useEffect(() => {
    const root = containerRef.current;
    if (!root) return;
    let frame = 0;
    const measure = () => {
      const next: Record<string, CardSize> = {};
      root.querySelectorAll<HTMLElement>('[data-canvas-card]').forEach((el) => {
        const id = el.dataset.itemId;
        if (!id) return;
        next[id] = { w: el.offsetWidth, h: el.offsetHeight };
      });
      setSizes((previous) => (sizesEqual(previous, next) ? previous : next));
    };
    const schedule = () => {
      window.cancelAnimationFrame(frame);
      frame = window.requestAnimationFrame(measure);
    };
    const observer = new ResizeObserver(schedule);
    root.querySelectorAll<HTMLElement>('[data-canvas-card]').forEach((el) => observer.observe(el));
    schedule();
    return () => {
      window.cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, [items, expanded, variant]);

  const addonEntries = useMemo<AddAddonEntry[]>(() => {
    // Workspace scope always offers its addon. In the main canvas, addons are
    // contextual: only workspaces already represented by a chat/project card
    // are offered, instead of dumping every historical workspace.
    let wsList: WorkspaceInfo[];
    if (scope.startsWith('ws:')) {
      wsList = scopedWorkspaces;
    } else {
      const represented = new Set(
        items
          .filter((item) => item.kind === 'chat' || item.kind === 'project')
          .map((item) => item.workspaceId),
      );
      wsList = workspaces.filter((w) => represented.has(w.id) && isProjectWorkspace(w));
    }
    return wsList.map((workspace) => ({
      workspace,
      addonId: 'design' as const,
      chatId: (chatsByWs[workspace.id] ?? [])[0]?.id,
    }));
  }, [scope, scopedWorkspaces, workspaces, chatsByWs, items]);

  const addedIds = useMemo(() => new Set(items.map((item) => item.id)), [items]);

  const addDraft = useCallback((draft: CanvasDraft, at?: { x: number; y: number }) => {
    const id = draftId(draft);
    if (itemsRef.current.some((item) => item.id === id)) {
      setAddOpen(false);
      return;
    }
    let x: number;
    let y: number;
    if (at) {
      x = at.x - 140;
      y = at.y - 60;
    } else {
      // New cards land in the visible top-left area, never under the floating
      // chat card in the middle of the workspace.
      const current = viewRef.current;
      const n = itemsRef.current.length;
      const screenX = 10;
      const screenY = 76 + n * 190;
      x = (screenX - current.x) / current.scale;
      y = (screenY - current.y) / current.scale;
    }
    const item = makeItem(draft, Math.round(x), Math.round(y));
    commitItems([...itemsRef.current, item]);
    setAddOpen(false);
  }, [commitItems]);

  const removeItem = useCallback((id: string) => {
    commitItems(itemsRef.current.filter((item) => item.id !== id));
    if (expanded === id) setExpanded(null);
  }, [commitItems, expanded]);

  const openItem = useCallback((item: CanvasItem) => {
    if (item.kind === 'chat') {
      onOpenChat?.(item.workspaceId, item.chatId);
    } else if (item.kind === 'project') {
      onOpenGroup?.(item.projectId);
    } else {
      onOpenAddon?.(item.workspaceId, item.addonId, item.chatId);
    }
  }, [onOpenChat, onOpenGroup, onOpenAddon]);

  const isItemRunning = useCallback((item: CanvasItem) => {
    if (item.kind !== 'chat') return false;
    if (item.workspaceId === activeWorkspaceId && item.chatId === activeChatId && isRunning) return true;
    return !!runningTabId && item.id === `chat:${runningTabId}`;
  }, [activeWorkspaceId, activeChatId, isRunning, runningTabId]);

  const edges = useMemo(() => {
    const chatItems = new Map<string, CanvasItem>();
    for (const item of items) {
      if (item.kind === 'chat') chatItems.set(`${item.workspaceId}:${item.chatId}`, item);
    }
    const result: Array<{ parent: CanvasItem; child: CanvasItem }> = [];
    for (const item of items) {
      if (item.kind !== 'chat') continue;
      let parent = item.parentItemId
        ? items.find((candidate) => candidate.id === item.parentItemId)
        : undefined;
      if (!parent) {
        const entry = entryByKey.get(`${item.workspaceId}:${item.chatId}`);
        const parentId = entry?.chat.parentChatId;
        if (parentId) {
          const parentWs = entry?.chat.parentWorkspaceId || item.workspaceId;
          parent = chatItems.get(`${parentWs}:${parentId}`);
        }
      }
      if (parent) result.push({ parent, child: item });
    }
    return result;
  }, [items, entryByKey]);

  const onWheel = (e: ReactWheelEvent) => {
    if ((e.target as HTMLElement).closest('[data-canvas-ui]')) return;
    const rect = containerRef.current?.getBoundingClientRect();
    if (!rect) return;
    const current = viewRef.current;
    const factor = e.deltaY < 0 ? 1.08 : 0.92;
    const scale = clamp(current.scale * factor, 0.45, 1.8);
    const cx = e.clientX - rect.left;
    const cy = e.clientY - rect.top;
    const worldX = (cx - current.x) / current.scale;
    const worldY = (cy - current.y) / current.scale;
    commitView({
      scale,
      x: cx - worldX * scale,
      y: cy - worldY * scale,
    });
  };

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) return;
    const target = e.target as HTMLElement;
    if (target.closest('[data-canvas-ui]')) return;
    setViewSmooth(false);
    const card = target.closest('[data-canvas-card]') as HTMLElement | null;
    if (card && !target.closest('button, input, a, [data-no-drag]')) {
      const id = card.dataset.itemId || '';
      const item = itemsRef.current.find((candidate) => candidate.id === id);
      if (!item) return;
      e.preventDefault();
      containerRef.current?.setPointerCapture(e.pointerId);
      setInteraction({
        type: 'item', id, item,
        sx: e.clientX, sy: e.clientY, ix: item.x, iy: item.y, moved: false,
      });
      return;
    }
    e.preventDefault();
    containerRef.current?.setPointerCapture(e.pointerId);
    setInteraction({
      type: 'pan', sx: e.clientX, sy: e.clientY,
      vx: viewRef.current.x, vy: viewRef.current.y,
    });
  };

  const onPointerMove = (e: ReactPointerEvent) => {
    if (!interaction) return;
    if (interaction.type === 'pan') {
      const next = {
        ...viewRef.current,
        x: interaction.vx + (e.clientX - interaction.sx),
        y: interaction.vy + (e.clientY - interaction.sy),
      };
      viewRef.current = next;
      setView(next);
      return;
    }
    const dx = (e.clientX - interaction.sx) / viewRef.current.scale;
    const dy = (e.clientY - interaction.sy) / viewRef.current.scale;
    const moved = interaction.moved || Math.abs(dx) > 3 || Math.abs(dy) > 3;
    const next = itemsRef.current.map((item) =>
      item.id === interaction.id ? { ...item, x: interaction.ix + dx, y: interaction.iy + dy } : item,
    );
    itemsRef.current = next;
    setItems(next);
    if (moved && !interaction.moved) setInteraction({ ...interaction, moved: true });
  };

  const onPointerUp = (e: ReactPointerEvent) => {
    if (!interaction) return;
    if (interaction.type === 'pan') {
      saveView(scope, viewRef.current);
    } else {
      if (interaction.moved) {
        saveItems(scope, itemsRef.current);
      } else {
        const item = itemsRef.current.find((candidate) => candidate.id === interaction.id);
        if (item?.kind === 'chat') {
          setExpanded((cur) => (cur === item.id ? null : item.id));
        } else if (item) {
          openItem(item);
        }
      }
    }
    containerRef.current?.releasePointerCapture(e.pointerId);
    setInteraction(null);
  };

  const onDrop = (e: ReactDragEvent) => {
    e.preventDefault();
    const raw = e.dataTransfer.getData('application/x-stark-canvas-item');
    if (!raw) return;
    try {
      const draft = JSON.parse(raw) as CanvasDraft;
      const rect = containerRef.current?.getBoundingClientRect();
      if (!rect) return;
      const current = viewRef.current;
      const world = {
        x: (e.clientX - rect.left - current.x) / current.scale,
        y: (e.clientY - rect.top - current.y) / current.scale,
      };
      addDraft(draft, { x: world.x, y: world.y });
    } catch {
      /* ignore malformed drag payloads */
    }
  };

  const zoomBy = useCallback((factor: number) => {
    const rect = containerRef.current?.getBoundingClientRect();
    if (!rect) return;
    const current = viewRef.current;
    const scale = clamp(current.scale * factor, 0.45, 1.8);
    const cx = rect.width / 2;
    const cy = rect.height / 2;
    const worldX = (cx - current.x) / current.scale;
    const worldY = (cy - current.y) / current.scale;
    commitView({ scale, x: cx - worldX * scale, y: cy - worldY * scale });
  }, [commitView]);

  const fitView = useCallback(() => {
    const rect = containerRef.current?.getBoundingClientRect();
    if (!rect || itemsRef.current.length === 0) return;
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const item of itemsRef.current) {
      const size = sizes[item.id] || { w: item.kind === 'chat' && (item.depth || 0) > 0 ? 282 : 326, h: expanded === item.id ? 420 : 210 };
      minX = Math.min(minX, item.x);
      minY = Math.min(minY, item.y);
      maxX = Math.max(maxX, item.x + size.w);
      maxY = Math.max(maxY, item.y + size.h);
    }
    if (!Number.isFinite(minX)) return;
    const paddingX = rect.width < 720 ? 36 : 90;
    const paddingTop = variant === 'map' ? 120 : 60;
    const paddingBottom = 70;
    const width = Math.max(1, maxX - minX);
    const height = Math.max(1, maxY - minY);
    const scale = clamp(Math.min(
      (rect.width - paddingX * 2) / width,
      (rect.height - paddingTop - paddingBottom) / height,
    ), 0.45, 1.15);
    const centerX = (minX + maxX) / 2;
    const centerY = (minY + maxY) / 2;
    commitView({
      scale,
      x: rect.width / 2 - centerX * scale,
      y: paddingTop + (rect.height - paddingTop - paddingBottom) / 2 - centerY * scale,
    }, true);
  }, [sizes, expanded, variant, commitView]);

  const relayout = useCallback(() => {
    const layout = new Map(graph.items.map((item) => [item.id, item]));
    const next = itemsRef.current.map((item) => {
      if (!(item.kind === 'chat' && item.auto)) return item;
      const node = layout.get(item.id);
      return node ? { ...item, x: node.x, y: node.y } : item;
    });
    commitItems(next);
    window.requestAnimationFrame(fitView);
  }, [graph, commitItems, fitView]);

  useEffect(() => {
    if (variant !== 'map' || fittedRef.current || items.length === 0) return;
    fittedRef.current = true;
    fitView();
  }, [variant, items.length, fitView]);

  const autoRoots = items.filter((item) => item.kind === 'chat' && item.auto && (item.depth || 0) === 0).length;
  const autoAgents = items.filter((item) => item.kind === 'chat' && item.auto && (item.depth || 0) > 0).length;
  const totalTokens = (usage?.input || 0) + (usage?.output || 0);

  return (
    <div
      ref={containerRef}
      className={`canvas-stage ${variant === 'map' ? 'is-map' : 'is-background'} ${interaction?.type === 'pan' ? 'is-panning' : ''}`}
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => setInteraction(null)}
      onWheel={onWheel}
      onDragOver={(e) => { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; }}
      onDrop={onDrop}
    >
      <div className="canvas-grid" style={{ backgroundPosition: `${view.x}px ${view.y}px` }} />
      {variant === 'map' && <div className="canvas-vignette" />}

      <div
        className={`canvas-world ${viewSmooth ? 'is-smooth' : ''}`}
        style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` }}
      >
        <svg className="canvas-edges" aria-hidden="true">
          {edges.map(({ parent, child }) => {
            const parentSize = sizes[parent.id] || { w: (parent.kind === 'chat' && (parent.depth || 0) > 0) ? 282 : 326, h: 210 };
            const childSize = sizes[child.id] || { w: (child.kind === 'chat' && (child.depth || 0) > 0) ? 282 : 326, h: 210 };
            const sx = parent.x + parentSize.w / 2;
            const sy = parent.y + parentSize.h;
            const tx = child.x + childSize.w / 2;
            const ty = child.y;
            const bend = Math.max(34, (ty - sy) * 0.48);
            const d = `M ${sx} ${sy} C ${sx} ${sy + bend}, ${tx} ${ty - bend}, ${tx} ${ty}`;
            const active = isItemRunning(parent) || isItemRunning(child);
            return (
              <g key={`${parent.id}->${child.id}`} className={`canvas-edge ${active ? 'is-active' : ''}`}>
                <path d={d} className="canvas-edge-line" />
                <circle cx={sx} cy={sy} r={2.2} className="canvas-edge-node" />
                <circle cx={tx} cy={ty} r={2.2} className="canvas-edge-node" />
              </g>
            );
          })}
        </svg>

        {items.map((item, index) => {
          const chatEntry = item.kind === 'chat' ? chatIndex.get(`${item.workspaceId}:${item.chatId}`) : undefined;
          const childChats = item.kind === 'chat' ? (childrenByParent.get(`${item.workspaceId}:${item.chatId}`) ?? []) : [];
          const project = item.kind === 'project' ? projects.find((candidate) => candidate.id === item.projectId) : undefined;
          const workspace = item.kind === 'chat'
            ? chatEntry?.workspace
            : item.kind === 'project'
              ? workspaces.find((candidate) => candidate.id === item.workspaceId)
              : workspaces.find((candidate) => candidate.id === item.workspaceId);
          const activeChat = item.kind === 'chat'
            && item.chatId === activeChatId
            && item.workspaceId === activeWorkspaceId;
          const running = isItemRunning(item);
          return (
            <div
              key={item.id}
              className="canvas-node"
              style={{
                left: item.x,
                top: item.y,
                zIndex: interaction?.type === 'item' && interaction.id === item.id ? 50 : expanded === item.id ? 40 : index + 1,
                animationDelay: `${Math.min(index, 10) * 34}ms`,
              }}
            >
              <CanvasItemCard
                item={item}
                chat={chatEntry?.chat}
                workspace={workspace}
                project={project}
                agents={activeChat ? agents : []}
                running={running}
                elapsed={activeChat && running ? elapsed : undefined}
                expanded={expanded === item.id}
                onToggleExpanded={() => setExpanded((cur) => (cur === item.id ? null : item.id))}
                onOpen={() => openItem(item)}
                onRemove={() => removeItem(item.id)}
                onSend={item.kind === 'chat' && onSendToChat
                  ? (text) => onSendToChat(item.workspaceId, item.chatId, text)
                  : undefined}
                compact={item.kind === 'chat' && (item.depth || 0) > 0}
                isActive={activeChat}
                childChats={childChats}
                hiddenChildren={item.kind === 'chat' ? item.hiddenChildren : 0}
                onOpenChild={(wsId, chatId) => onOpenChat?.(wsId, chatId)}
                onCloseTab={item.kind === 'chat' && item.auto && item.isTab && onCloseTab
                  ? () => onCloseTab(`${item.workspaceId}:${item.chatId}`)
                  : undefined}
                tokens={activeChat ? totalTokens : undefined}
                cost={activeChat ? usage?.cost : undefined}
                toolCalls={activeChat ? toolCalls : undefined}
              />
            </div>
          );
        })}
      </div>

      {variant === 'map' && (
        <div data-canvas-ui className="canvas-map-head">
          <div className="canvas-map-title-wrap">
            <span className="canvas-map-mark"><Waypoints size={15} /></span>
            <div className="canvas-map-title-copy">
              <div className="canvas-map-title">Agent map</div>
              <div className="canvas-map-subtitle">
                {autoRoots} {autoRoots === 1 ? 'chat' : 'chats'}
                <span>·</span>
                {autoAgents} {autoAgents === 1 ? 'agent' : 'agents'}
                {errors ? <><span>·</span><b className="is-error">{errors} errors</b></> : null}
              </div>
            </div>
          </div>
          <div className="canvas-map-stats">
            <span><b>{tabs?.length ?? autoRoots}</b> open</span>
            {!!totalTokens && <span><b>{compactNumber(totalTokens)}</b> tokens</span>}
            {!!usage?.cost && <span><b>${usage.cost < 0.01 ? usage.cost.toFixed(4) : usage.cost.toFixed(2)}</b></span>}
            {!!toolCalls && <span><b>{toolCalls}</b> tools</span>}
          </div>
          <div className="canvas-map-controls">
            <button type="button" onClick={openAddMenu} title="Add to canvas" aria-label="Add to canvas"><FolderPlus size={13} /></button>
            <button type="button" onClick={() => zoomBy(0.86)} title="Zoom out" aria-label="Zoom out"><Minus size={13} /></button>
            <button type="button" onClick={() => zoomBy(1.16)} title="Zoom in" aria-label="Zoom in"><Plus size={13} /></button>
            <button type="button" onClick={fitView} title="Fit to view" aria-label="Fit to view"><Scan size={13} /></button>
            <button type="button" onClick={relayout} title="Reset layout" aria-label="Reset layout"><RotateCcw size={13} /></button>
            {onCloseMap && (
              <button type="button" className="is-close" onClick={onCloseMap} title="Back to chat" aria-label="Back to chat"><X size={14} /></button>
            )}
          </div>
        </div>
      )}

      {variant !== 'map' && items.length > 0 && (
        <button
          type="button"
          data-canvas-ui
          onClick={openAddMenu}
          className="canvas-add-fab"
          title="Add to canvas"
          aria-label="Add to canvas"
        >
          <Plus size={13} />
          <span>Add</span>
        </button>
      )}

      {addOpen && (
        <CanvasAddMenu
          chats={chatEntries}
          projects={projects.map((project) => {
            const owner = workspaces.find((candidate) => candidate.id === project.origin_workspace_id)
              || scopedWorkspaces.find((candidate) => isProjectWorkspace(candidate))
              || scopedWorkspaces[0]
              || workspaces[0];
            return { workspace: owner, project };
          }).filter((entry) => Boolean(entry.workspace))}
          addons={addonEntries}
          added={addedIds}
          onAdd={addDraft}
          onClose={() => setAddOpen(false)}
          anchor={addAnchor}
        />
      )}

      {items.length === 0 && (
        <div className="canvas-empty" data-canvas-ui>
          <button type="button" onClick={openAddMenu} className="canvas-empty-add">
            <span><Plus size={14} /></span>
            Add to Canvas
          </button>
          {variant === 'map' && <div className="canvas-empty-copy">Open a chat and it will appear here automatically.</div>}
        </div>
      )}

      {variant === 'map' && items.length > 0 && (
        <div className="canvas-map-hint" data-canvas-ui>
          Drag to pan · scroll to zoom · drag cards to arrange
        </div>
      )}
    </div>
  );
}
