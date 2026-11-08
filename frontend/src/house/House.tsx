import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Code2 } from 'lucide-react';
import iconUrl from '../assets/stark-icon.png';
import HouseHeader from './HouseHeader';
import HouseRail, { type SideView } from './HouseRail';
import HouseMobileNav from './HouseMobileNav';
import HouseProjectsPanel from './HouseProjectsPanel';
import HouseChat from './HouseChat';
import HouseSidebar from './HouseSidebar';
import HousePortal from './HousePortal';
import CodingIDE from './CodingIDE';
import NewProjectDialog from './NewProjectDialog';
import SubagentTabsPane from './SubagentTabsPane';
import { useSubagentTabs } from './useSubagentTabs';
import GroupProjectModal from './GroupProjectModal';
import { useGroupProjects } from './useGroupProjects';
import TokenStatsModal from '../components/Layout/TokenStatsModal';
import SettingsModal from '../components/Layout/SettingsModal';
import { useHouseChat } from './useHouseChat';
import { useSessions, tabId } from './useSessions';
import { useSessionRunner } from './useSessionRunner';
import { useCanvasAgentState } from '../hooks/useCanvasAgentState';
import type { ChatMessage } from '../types';
import type { TraceStep } from '../utils/logParser';
import { kindForWorkspace, sendCanvasChat } from '../canvas/canvasSend';
import { addWorkspace, createChat, fetchChat, rewindChat, updateChatTitle, deleteChat, createGroupProject } from '../services/api';
import { setBusy } from '../utils/runtimeBusy';
import '../house/house.css';

const CanvasStage = lazy(() => import('../canvas/CanvasStage'));

export default function House() {
  const [transitioning, setTransitioning] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() =>
    typeof window === 'undefined' ? true : window.matchMedia('(min-width: 768px)').matches,
  );
  const [sideView, setSideView] = useState<SideView>('chats');
  const [isStatsOpen, setIsStatsOpen] = useState(false);
  const [isSettingsOpen, setIsSettingsOpen] = useState(false);
  const [blockedNote, setBlockedNote] = useState<string | null>(null);
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const [projectDialogMode, setProjectDialogMode] = useState<'group' | 'solo'>('group');
  const subagents = useSubagentTabs();
  const [exitingIdeTab, setExitingIdeTab] = useState<import('../types').SessionTab | null>(null);

  const sessions = useSessions();
  // Main workspace → General chats only (orchestrator agent). House workspace
  // is a separate route (#/house) that binds _house; never leak house here.
  const chat = useHouseChat({ workspaceKind: 'general' });

  // Publish busy state for the auto-updater (never restart mid-run).
  useEffect(() => {
    setBusy('home', chat.isRunning || sessions.isGlobalRunning);
    return () => setBusy('home', false);
  }, [chat.isRunning, sessions.isGlobalRunning]);
  const { refreshSessions, newSession, newChatIn, openChat, closeTab } = sessions;
  const {
    activeChatId: generalActiveChatId,
    isRunning: chatRunning,
    isLoading: chatLoading,
    handleSelectChat,
    handleSend: handleGeneralChatSend,
  } = chat;

  const activeTab = sessions.activeTab;
  const isCodingTab = activeTab?.type === 'coding';

  // Group projects follow the active tab: a coding workspace sees only the
  // projects started from its own folder, while General sees them all. The
  // rule itself lives on the server — this only carries the id.
  const [openProjectId, setOpenProjectId] = useState<string | null>(null);
  const groupScopeId = activeTab?.workspaceId || chat.workspaceId || '';
  const { projects, scoped: projectsScoped, refresh: refreshProjects } = useGroupProjects(groupScopeId);

  // IDE open/close transition: keep the exiting IDE mounted for the exit
  // animation so it feels like staying on the same page (only the IDE shows/
  // hides), not a hard page swap.
  const prevIsCodingRef = useRef(false);
  const prevActiveTabRef = useRef<import('../types').SessionTab | null>(null);
  useEffect(() => {
    const wasCoding = prevIsCodingRef.current;
    const prevTab = prevActiveTabRef.current;
    prevIsCodingRef.current = !!isCodingTab;
    prevActiveTabRef.current = activeTab;
    if (wasCoding && !isCodingTab && prevTab?.type === 'coding') {
      setExitingIdeTab(prevTab);
      const t = window.setTimeout(() => setExitingIdeTab(null), 320);
      return () => window.clearTimeout(t);
    }
    if (isCodingTab) setExitingIdeTab(null);
    return undefined;
  }, [isCodingTab, activeTab]);

  const handleCloseIde = useCallback(() => {
    // Back to the most recent chat tab — the coding tab stays in the strip
    // (user can re-open it), matching "Tab stays (Recommended)".
    const chatTab = [...sessions.tabs].reverse().find((t) => t.type === 'chat');
    if (chatTab) {
      sessions.activateTab(chatTab.id);
    } else {
      // No chat tab open (edge case) — create one
      sessions.newSession();
    }
  }, [sessions]);

  // One runner for whichever coding session is active. Living at House level
  // means a running pipeline keeps streaming while you flip through tabs.
  const handleRunFinished = useCallback(() => {
    refreshSessions();
  }, [refreshSessions]);
  const runner = useSessionRunner(isCodingTab && activeTab ? activeTab : null, handleRunFinished);
  const lastTraceSteps = useMemo<TraceStep[]>(() => {
    for (let i = chat.displayMessages.length - 1; i >= 0; i--) {
      const steps = chat.displayMessages[i]?.metadata?.steps;
      if (Array.isArray(steps) && steps.length > 0) return steps as TraceStep[];
    }
    return [];
  }, [chat.displayMessages]);
  const generalCanvasAgents = useCanvasAgentState({
    isRunning: chat.isRunning,
    liveLog: chat.liveState?.live_terminal_log,
    tokenUsage: chat.liveState?.token_usage,
    traceSteps: lastTraceSteps,
  }).agents;

  const activeToolCalls = useMemo(
    () => lastTraceSteps.filter((step) => step.type === 'tool' || step.type === 'command').length,
    [lastTraceSteps],
  );
  const activeErrors = useMemo(
    () => chat.displayMessages.slice(-40).filter((message) => message.role === 'error').length,
    [chat.displayMessages],
  );
  const mapUsage = useMemo(() => ({
    input: chat.totalInput,
    output: chat.totalOutput,
    cost: chat.totalCost,
    context: chat.activeContextTokens,
  }), [chat.totalInput, chat.totalOutput, chat.totalCost, chat.activeContextTokens]);

  // Mini-chat cards reuse the exact live thread the full chat renders, so
  // streaming text, tool steps, and thinking all stay in sync.
  const liveMessagesByChat = useMemo<Record<string, ChatMessage[]>>(() => {
    const map: Record<string, ChatMessage[]> = {};
    if (chat.isRunning && chat.workspaceId && chat.activeChatId) {
      map[`${chat.workspaceId}:${chat.activeChatId}`] = chat.displayMessages;
    }
    if (runner.isRunning && activeTab) {
      map[`${activeTab.workspaceId}:${activeTab.chatId}`] = runner.messages;
    }
    return map;
  }, [
    chat.isRunning, chat.workspaceId, chat.activeChatId, chat.displayMessages,
    runner.isRunning, runner.messages, activeTab,
  ]);

  // Room-workspace sidebars navigate home via #/?ws=<id>&chat=<id> — once the
  // session layer has booted, open that chat and clean the hash.
  useEffect(() => {
    if (!sessions.booted) return;
    const h = window.location.hash;
    const qi = h.indexOf('?');
    if (!h.startsWith('#/') || qi < 0) return;
    const params = new URLSearchParams(h.slice(qi + 1));
    const wsId = params.get('ws');
    const chatId = params.get('chat');
    if (wsId && chatId) {
      openChat(wsId, chatId);
      window.history.replaceState(null, '', '#/');
    }
  }, [sessions.booted, openChat]);

  // Rail/nav view click: always open (or switch) the panel. Collapsing is the
  // backdrop on mobile and the rail staying open on desktop — no tiny close
  // button that strands the user.
  const handleSideView = useCallback((view: SideView) => {
    setSideView(view);
    setSidebarOpen(true);
  }, []);

  // Escape collapses the side panel (it stays docked, never slides away)
  useEffect(() => {
    if (!sidebarOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setSidebarOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [sidebarOpen]);

  // ── Sync: activating a General chat tab points useHouseChat at that chat ──
  const activeTabId = activeTab?.id ?? null;
  const activeTabType = activeTab?.type ?? null;
  const activeTabChatId = activeTab?.chatId ?? null;

  // A child pane belongs to its parent session; clear it when switching chats.
  const prevTabRef = useRef<string | null>(activeTabId);
  const clearSubagents = subagents.clear;
  useEffect(() => {
    if (prevTabRef.current !== activeTabId) clearSubagents();
    prevTabRef.current = activeTabId;
  }, [activeTabId, clearSubagents]);

  useEffect(() => {
    if (activeTabType !== 'chat' || !activeTabChatId) return;
    if (chatRunning || chatLoading) return;
    if (generalActiveChatId !== activeTabChatId) {
      handleSelectChat(activeTabChatId, { silent: true });
    }
  }, [activeTabId, activeTabType, activeTabChatId, generalActiveChatId, chatRunning, chatLoading, handleSelectChat]);

  // ── Session creation ──
  const handleNewSession = useCallback(() => {
    newSession();
  }, [newSession]);

  const openNewGroupProject = useCallback(() => {
    setProjectDialogMode('group');
    setProjectDialogOpen(true);
  }, []);

  const openNewWorkspace = useCallback(() => {
    setProjectDialogMode('solo');
    setProjectDialogOpen(true);
  }, []);

  const handleNewChatIn = useCallback((workspaceId: string) => {
    newChatIn(workspaceId);
  }, [newChatIn]);
  const handleOpenSession = useCallback((workspaceId: string, chatId: string) => {
    openChat(workspaceId, chatId);
  }, [openChat]);

  /** New Project dialog → register workspace + create its first chat + open it. */
  const handleCreateProject = useCallback(async (folderPath: string, name: string, initialPrompt?: string) => {
    try {
      const ws = await addWorkspace(folderPath, name);
      const chatData = await createChat(ws.id, 'New Chat').catch(() => null);
      await refreshSessions();
      if (chatData) {
        openChat(ws.id, chatData.id);
        if (initialPrompt?.trim()) chat.seedDraft(initialPrompt.trim());
      }
      return ws;
    } catch {
      return null;
    }
  }, [refreshSessions, openChat, chat.seedDraft]);

  /** Step 2 of the new-project flow: durable team board inside the workspace. */
  const handleCreateGroupFromDialog = useCallback(async (workspace: import('../types').WorkspaceInfo, task: string) => {
    const project = await createGroupProject({ goal: task, workspaceId: workspace.id });
    if (!project.id) throw new Error('The backend did not return a project id.');
    await refreshProjects();
    setOpenProjectId(project.id);
  }, [refreshProjects]);

  const handleRenameChat = useCallback((workspaceId: string, chatId: string, title: string) => {
    // Optimistic so the sidebar shows the new title instantly.
    sessions.renameChatLocal(workspaceId, chatId, title);
    updateChatTitle(workspaceId, chatId, title).catch(() => {
      sessions.forceRefresh();
    });
  }, [sessions]);

  const handleDeleteChat = useCallback((workspaceId: string, chatId: string) => {
    // Optimistic: close tab + remove from list immediately; server is fire-and-forget.
    sessions.removeChatLocal(workspaceId, chatId);
    closeTab(tabId(workspaceId, chatId));
    deleteChat(workspaceId, chatId).catch(() => {
      sessions.forceRefresh();
    });
  }, [closeTab, sessions]);

  const handleLaunchRoom = useCallback((room: 'personal' | 'house') => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = room === 'personal' ? '#/personal' : '#/house';
    }, 320);
  }, [transitioning]);

  // ── Send gating: one pipeline at a time, globally ──
  const foreignRunActive = sessions.isGlobalRunning
    && (!!sessions.runningTabId && sessions.runningTabId !== activeTabId);

  const notifyBlocked = useCallback(() => {
    setBlockedNote('Another session is running — switch to its tab to watch it.');
    window.setTimeout(() => setBlockedNote(null), 3500);
  }, []);

  const handleGeneralSend = useCallback((prompt: string, files: File[]) => {
    if (chatRunning) return;
    if (runner.isRunning || foreignRunActive) {
      notifyBlocked();
      return;
    }
    handleGeneralChatSend(prompt, files);
  }, [chatRunning, handleGeneralChatSend, runner.isRunning, foreignRunActive, notifyBlocked]);

  const launchCoding = useCallback(() => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/coding';
    }, 320);
  }, [transitioning]);

  const launchPersonal = useCallback(() => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/personal';
    }, 320);
  }, [transitioning]);

  const launchHouse = useCallback(() => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/house';
    }, 320);
  }, [transitioning]);

  const activeGeneralTitle = useMemo(
    () => chat.chats.find((c) => c.id === chat.activeChatId)?.title ?? 'New conversation',
    [chat.chats, chat.activeChatId],
  );

  const mainWorkspace = sessions.workspaces.find((w) => w.isGeneral) || sessions.workspaces[0] || null;
  const generalChatView = (
    <div className="relative flex-1 min-h-0">
      <div className="absolute inset-0 overflow-y-auto house-thread px-3 pt-3 sm:px-5">
        <Suspense fallback={null}>
          <CanvasStage
            key="main"
            scope="main"
            variant="map"
            workspaces={sessions.workspaces}
            chatsByWs={sessions.chatsByWs}
            activeWorkspaceId={mainWorkspace?.id ?? null}
            activeChatId={chat.activeChatId}
            agents={generalCanvasAgents}
            isRunning={chat.isRunning}
            projects={projects}
            tabs={sessions.tabs}
            runningTabId={sessions.runningTabId}
            usage={mapUsage}
            toolCalls={activeToolCalls}
            errors={activeErrors}
            liveMessages={liveMessagesByChat}
            onCloseTab={sessions.closeTab}
            onOpenChat={(wsId, chatId) => {
              const ws = sessions.workspaces.find((w) => w.id === wsId);
              if (ws && !ws.isGeneral && !ws.isHouse && !ws.isPersonal && !(ws as { isDfd?: boolean }).isDfd) {
                window.location.hash = `#/coding?ws=${encodeURIComponent(wsId)}&chat=${encodeURIComponent(chatId)}`;
              } else {
                sessions.openChat(wsId, chatId);
              }
            }}
            onOpenGroup={(projectId) => { setOpenProjectId(projectId); }}
            onOpenAddon={(wsId, addonId, chatId) => {
              const cid = chatId || sessions.chatsByWs[wsId]?.[0]?.id || '';
              window.location.hash = `#/coding?ws=${encodeURIComponent(wsId)}&chat=${encodeURIComponent(cid)}&addon=${addonId}`;
            }}
            onSendToChat={async (wsId, chatId, text, files = []) => {
              const ws = sessions.workspaces.find((w) => w.id === wsId);
              if (!ws) throw new Error('Workspace not found.');
              if (wsId === mainWorkspace?.id && chatId === chat.activeChatId) {
                handleGeneralSend(text, files);
                return;
              }
              await sendCanvasChat(ws, chatId, text, files, kindForWorkspace(ws));
              await sessions.forceRefresh();
            }}
            onRewindMessage={async (wsId, chatId, message) => {
              const isActive = wsId === mainWorkspace?.id && chatId === chat.activeChatId;
              if (isActive) {
                try {
                  // The summary carries only the fields a card needs; the
                  // active chat's rewind needs the full row (attachments).
                  const full = await fetchChat(wsId, chatId);
                  const target = full.messages.find((candidate) => candidate.id === message.id);
                  if (target) {
                    await chat.handleRewind(target);
                    await sessions.forceRefresh();
                    return;
                  }
                } catch {
                  /* fall through to the direct rewind */
                }
              }
              await rewindChat(wsId, chatId, message.id);
              await sessions.forceRefresh();
            }}
          />
        </Suspense>
      </div>
      <div className="pointer-events-none absolute inset-x-0 bottom-0 z-[60]">
      <HouseChat
        title={activeGeneralTitle}
        messages={chat.displayMessages}
        isRunning={chat.isRunning}
        isLoading={chat.isLoading}
        notification={chat.notification ?? blockedNote}
        threadRef={chat.threadRef}
        isVoiceMode={chat.isVoiceMode}
        totalCost={chat.totalCost}
        totalInput={chat.totalInput}
        totalOutput={chat.totalOutput}
        totalCacheHitTokens={chat.totalCacheHitTokens}
        activeContextTokens={chat.activeContextTokens}
        onSend={handleGeneralSend}
        onCancel={chat.handleCancel}
        onToggleVoice={chat.toggleVoice}
        onOpenStats={() => setIsStatsOpen(true)}
        onOpenSubagent={subagents.open}
        workspaceId={chat.workspaceId}
        chatId={activeTabChatId}
        onRewind={chat.handleRewind}
        onRetry={(prompt) => handleGeneralSend(prompt, [])}
        draft={chat.draft}
        draftAttachments={chat.draftAttachments}
        onDraftConsumed={chat.clearDraft}
        onDraftAttachmentsConsumed={chat.clearDraftAttachments}
      />
      </div>
    </div>
  );

  // Initial blank (#0c0c0c) while the session layer boots.
  if (!sessions.booted) {
    if (sessions.bootError) {
      return (
        <div className="fixed inset-0 grid place-items-center bg-[#0c0c0c] px-6 select-none">
          <div className="flex max-w-sm flex-col items-center gap-4 text-center">
            <img src={iconUrl} alt="Stark" width={72} height={72} style={{ borderRadius: 18 }} />
            <div className="space-y-1.5">
              <div className="text-sm font-medium text-zinc-200">Stark backend is not ready</div>
              <div className="text-xs leading-relaxed text-zinc-500">
                Start the backend or retry once it has finished loading.
              </div>
            </div>
            <button
              onClick={() => void sessions.retryBoot()}
              className="rounded-lg border border-zinc-700 bg-zinc-800 px-4 py-2 text-xs font-medium text-zinc-200 transition-colors hover:bg-zinc-700"
            >
              Retry
            </button>
          </div>
        </div>
      );
    }
    return (
      <div className="fixed inset-0 grid place-items-center bg-[#0c0c0c] select-none">
        <div className="flex flex-col items-center gap-4 animate-[splashIn_.35s_ease]">
          <img src={iconUrl} alt="Stark" width={96} height={96} style={{ borderRadius: 24 }} className="shadow-[0_8px_32px_rgba(0,0,0,.45)]" />
          <div className="flex gap-1.5">
            <span className="house-typing-dot" />
            <span className="house-typing-dot" />
            <span className="house-typing-dot" />
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="house-root fixed inset-0 h-[100dvh] flex flex-col overflow-hidden select-none">
      <HouseHeader
        tabs={sessions.tabs}
        activeTabId={sessions.activeTabId}
        runningTabId={sessions.runningTabId}
        onActivateTab={sessions.activateTab}
        onCloseTab={sessions.closeTab}
        onNewSession={handleNewSession}
      />

      <div className="relative z-10 flex flex-1 min-h-0 overflow-hidden">
      <div className="hidden md:flex shrink-0">
        <HouseRail
          view={sideView}
          onView={handleSideView}
          onNewChat={handleNewSession}
          onOpenSettings={() => setIsSettingsOpen(true)}
          onOpenCloud={() => { window.location.hash = '#/cloud'; }}
          waiting={projects.reduce((n, p) => n + (p.awaiting_approval || 0), 0)}
        />
      </div>
      {sidebarOpen && (
        <div
          className="md:hidden fixed inset-0 z-40 bg-black/60 backdrop-blur-sm"
          onClick={() => setSidebarOpen(false)}
          aria-hidden="true"
        />
      )}
      {sidebarOpen && (
        <div className="house-drawer-enter max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-50 max-md:w-[84vw] max-md:max-w-[320px] max-md:shadow-2xl flex min-h-0">
      {sideView === 'chats' ? (
        <HouseSidebar
          persistent
          open
          onClose={() => setSidebarOpen(false)}
          workspaces={sessions.workspaces}
          chatsByWs={sessions.chatsByWs}
          activeTabId={sessions.activeTabId}
          runningTabId={sessions.runningTabId}
          pinnedChats={sessions.pinnedChats}
          chatsFilter={sessions.chatsFilter}
          onFilterChange={sessions.setChatsFilter}
          onTogglePin={sessions.togglePin}
          onOpenSession={handleOpenSession}
          onNewChat={handleNewSession}
          onNewProject={openNewWorkspace}
          onNewChatInProject={handleNewChatIn}
          onOpenRoom={handleLaunchRoom}
          onRenameChat={handleRenameChat}
          onDeleteChat={handleDeleteChat}
          onLaunchPersonal={launchPersonal}
          onLaunchCoding={launchCoding}
          onLaunchHouse={launchHouse}
        />
      ) : (
        <div className="w-[248px] max-md:w-full shrink-0 bg-[#0f0f0f] border-r house-hairline min-h-0">
          <HouseProjectsPanel
            projects={projects}
            projectsScoped={projectsScoped}
            onOpenProject={setOpenProjectId}
            onNewProject={openNewGroupProject}
          />
        </div>
      )}
        </div>
      )}
      <main className="relative flex-1 min-h-0 flex overflow-hidden">
        <div className="relative flex-1 min-w-0 min-h-0 flex flex-col overflow-hidden">
        {exitingIdeTab ? (
          <>
            {/* Chat is visible immediately under the exiting IDE — feels like the IDE slides away */}
            {generalChatView}
            <div className="absolute inset-0 z-10 flex ide-exit bg-[#0a0a0a]">
              <CodingIDE
                key={exitingIdeTab.id}
                session={exitingIdeTab}
                runner={runner}
                isGlobalRunning={sessions.isGlobalRunning}
                onCloseIde={handleCloseIde}
              />
            </div>
          </>
        ) : isCodingTab && activeTab ? (
          <div key={activeTab.id} className="relative flex-1 min-w-0 min-h-0 flex ide-enter">
            <CodingIDE
              key={activeTab.id}
              session={activeTab}
              runner={runner}
              isGlobalRunning={sessions.isGlobalRunning}
              onCloseIde={handleCloseIde}
            />
          </div>
        ) : (
          generalChatView
        )}
        </div>
        {!isCodingTab && !exitingIdeTab && subagents.tabs.length > 0 && (
          <SubagentTabsPane
            tabs={subagents.tabs}
            activeChildId={subagents.activeChildId}
            parentTitle={activeGeneralTitle}
            fallbackWorkspaceId={chat.workspaceId || sessions.workspaces.find((w) => w.isGeneral)?.id || ''}
            onActivate={subagents.activate}
            onClose={subagents.close}
            onOpenSubagent={subagents.open}
          />
        )}
      </main>
      </div>

      <HouseMobileNav
        view={sideView}
        onView={handleSideView}
        onNewChat={handleNewSession}
        onOpenSettings={() => setIsSettingsOpen(true)}
        waiting={projects.reduce((n, p) => n + (p.awaiting_approval || 0), 0)}
      />

      {projectDialogOpen && (
        <NewProjectDialog
          onClose={() => setProjectDialogOpen(false)}
          onCreateProject={handleCreateProject}
          onCreateGroup={handleCreateGroupFromDialog}
          defaultMode={projectDialogMode}
        />
      )}

      <TokenStatsModal
        isOpen={isStatsOpen}
        onClose={() => setIsStatsOpen(false)}
        tokenUsage={chat.combinedTokenUsage}
        lastModelName={chat.lastModelName}
      />

      <SettingsModal
        isOpen={isSettingsOpen}
        onClose={() => setIsSettingsOpen(false)}
      />

      {openProjectId && (
        <GroupProjectModal
          key={openProjectId}
          projectId={openProjectId}
          onClose={() => { setOpenProjectId(null); refreshProjects(); }}
        />
      )}

      {transitioning && (
        <HousePortal label="Entering Coding Workspace" icon={<Code2 size={22} />} />
      )}
    </div>
  );
}
