import { useCallback, useMemo, useRef, useState } from 'react';
import type { DragEvent as ReactDragEvent, PointerEvent as ReactPointerEvent, WheelEvent as ReactWheelEvent } from 'react';
import { LayoutGrid, Plus } from 'lucide-react';
import type { ChatSummary, GroupProjectRow, WorkspaceInfo } from '../types';
import type { BoardAgent } from '../types/board.types';
import CanvasAddMenu from './CanvasAddMenu';
import type { AddAddonEntry, AddChatEntry, AddProjectEntry } from './CanvasAddMenu';
import CanvasItemCard from './CanvasItemCard';
import type { CanvasAddonId, CanvasDraft, CanvasItem, CanvasScope, CanvasView } from './canvasTypes';
import { draftId, makeItem } from './canvasTypes';
import { loadItems, loadView, saveItems, saveView } from './canvasStorage';

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
}

type Interaction =
  | { type: 'pan'; sx: number; sy: number; vx: number; vy: number }
  | { type: 'item'; id: string; sx: number; sy: number; ix: number; iy: number; moved: boolean; item: CanvasItem };

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function isProjectWorkspace(ws: WorkspaceInfo): boolean {
  return !ws.isGeneral && !ws.isHouse && !ws.isPersonal
    && !(ws as { isDfd?: boolean }).isDfd;
}

export default function CanvasStage({
  scope, workspaces, chatsByWs, activeWorkspaceId, activeChatId,
  agents, isRunning, elapsed, projects = [],
  onOpenChat, onOpenGroup, onOpenAddon, onSendToChat,
}: Props) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const [items, setItems] = useState<CanvasItem[]>(() => loadItems(scope));
  const [view, setView] = useState<CanvasView>(() => loadView(scope));
  const [expanded, setExpanded] = useState<string | null>(null);
  const [addOpen, setAddOpen] = useState(false);
  const [interaction, setInteraction] = useState<Interaction | null>(null);
  const itemsRef = useRef(items);
  const viewRef = useRef(view);

  const commitItems = useCallback((next: CanvasItem[]) => {
    itemsRef.current = next;
    setItems(next);
    saveItems(scope, next);
  }, [scope]);

  const commitView = useCallback((next: CanvasView) => {
    viewRef.current = next;
    setView(next);
    saveView(scope, next);
  }, [scope]);

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

  const projectEntries = useMemo<AddProjectEntry[]>(() => {
    return projects.map((project) => {
      const owner = workspaces.find((w) => w.id === project.origin_workspace_id)
        || scopedWorkspaces.find((w) => isProjectWorkspace(w))
        || scopedWorkspaces[0]
        || workspaces[0];
      return { workspace: owner, project };
    }).filter((entry) => Boolean(entry.workspace));
  }, [projects, workspaces, scopedWorkspaces]);

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

  return (
    <div
      ref={containerRef}
      className="relative h-full w-full select-none overflow-hidden bg-[#0a0a0c]"
      onPointerDown={onPointerDown}
      onPointerMove={onPointerMove}
      onPointerUp={onPointerUp}
      onPointerCancel={() => setInteraction(null)}
      onWheel={onWheel}
      onDragOver={(e) => { e.preventDefault(); e.dataTransfer.dropEffect = 'copy'; }}
      onDrop={onDrop}
      style={{ cursor: interaction?.type === 'pan' ? 'grabbing' : 'default' }}
    >
      <div
        className="pointer-events-none absolute inset-0"
        style={{
          backgroundImage: 'radial-gradient(circle, rgba(255,255,255,0.075) 1px, transparent 1px)',
          backgroundSize: '24px 24px',
          backgroundPosition: `${view.x}px ${view.y}px`,
        }}
      />

      <div
        className="pointer-events-none absolute left-0 top-0 origin-top-left"
        style={{ transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})` }}
      >
        {items.map((item, index) => {
          const chatEntry = item.kind === 'chat' ? chatIndex.get(`${item.workspaceId}:${item.chatId}`) : undefined;
          const project = item.kind === 'project' ? projects.find((p) => p.id === item.projectId) : undefined;
          const workspace = item.kind === 'chat'
            ? chatEntry?.workspace
            : item.kind === 'project'
              ? projectEntries.find((p) => p.project.id === item.projectId)?.workspace
              : workspaces.find((w) => w.id === item.workspaceId);
          const isActiveChat = item.kind === 'chat' && item.chatId === activeChatId && item.workspaceId === activeWorkspaceId;
          return (
            <div
              key={item.id}
              className="pointer-events-auto absolute"
              style={{ left: item.x, top: item.y, zIndex: interaction?.type === 'item' && interaction.id === item.id ? 50 : index + 1 }}
            >
              <CanvasItemCard
                item={item}
                chat={chatEntry?.chat}
                workspace={workspace}
                project={project}
                agents={isActiveChat ? agents : []}
                running={isActiveChat && isRunning}
                elapsed={isActiveChat && isRunning ? elapsed : undefined}
                expanded={expanded === item.id}
                onToggleExpanded={() => setExpanded((cur) => (cur === item.id ? null : item.id))}
                onOpen={() => openItem(item)}
                onRemove={() => removeItem(item.id)}
                onSend={item.kind === 'chat' && onSendToChat
                  ? (text) => onSendToChat(item.workspaceId, item.chatId, text)
                  : undefined}
              />
            </div>
          );
        })}
      </div>

      {addOpen && (
        <div data-canvas-ui>
          <CanvasAddMenu
            chats={chatEntries}
            projects={projectEntries}
            addons={addonEntries}
            added={addedIds}
            onAdd={addDraft}
            onClose={() => setAddOpen(false)}
          />
        </div>
      )}

      {items.length === 0 && (
        <div className="pointer-events-none absolute left-6 top-5 w-[300px]">
          <div className="flex flex-col items-start gap-2 text-left">
            <LayoutGrid size={22} className="text-zinc-700" />
            <p className="text-[13px] font-medium text-zinc-400">Your canvas is empty</p>
            <p className="text-[10.5px] leading-4 text-zinc-600">
              Add a chat, project, or addon, then drag the cards anywhere. Nothing appears here automatically.
            </p>
            <button
              type="button"
              data-canvas-ui
              onClick={() => setAddOpen(true)}
              className="pointer-events-auto mt-1 flex h-8 items-center gap-1.5 rounded-lg bg-white px-3 text-[10.5px] font-semibold text-[#0a0a0d] hover:bg-zinc-200"
            >
              <Plus size={12} /> Add to canvas
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
