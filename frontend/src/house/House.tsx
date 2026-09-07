import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Code2 } from 'lucide-react';
import HouseHeader from './HouseHeader';
import HouseChat from './HouseChat';
import HouseSidebar from './HouseSidebar';
import HousePortal from './HousePortal';
import CodingIDE from './CodingIDE';
import NewProjectDialog from './NewProjectDialog';
import SubagentSessionView from './SubagentSessionView';
import AddTaskModal from '../components/Canvas/AddTaskModal';
import TokenStatsModal from '../components/Layout/TokenStatsModal';
import { useHouseChat } from './useHouseChat';
import { useSessions, tabId } from './useSessions';
import { useSessionRunner } from './useSessionRunner';
import { addWorkspace, createChat, updateChatTitle, deleteChat } from '../services/api';
import { seedAssistantMemory } from './assistant';
import '../house/house.css';

export default function House() {
  const [transitioning, setTransitioning] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [isStatsOpen, setIsStatsOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [blockedNote, setBlockedNote] = useState<string | null>(null);
  const [projectDialogOpen, setProjectDialogOpen] = useState(false);
  const [subagentView, setSubagentView] = useState<{ childId: string; label: string } | null>(null);
  const [exitingIdeTab, setExitingIdeTab] = useState<import('../types').SessionTab | null>(null);

  const sessions = useSessions();
  // Main workspace → General chats only (orchestrator agent). House workspace
  // is a separate route (#/house) that binds _house; never leak house here.
  const chat = useHouseChat({ workspaceKind: 'general' });
  const { refreshSessions, newSession, newChatIn, openChat, closeTab } = sessions;
  const {
    activeChatId: generalActiveChatId,
    isRunning: chatRunning,
    isLoading: chatLoading,
    handleSelectChat,
    handleSend: handleGeneralChatSend,
    handleNewChat: handleGeneralNewChat,
  } = chat;

  const activeTab = sessions.activeTab;
  const isCodingTab = activeTab?.type === 'coding';

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

  // Teach the assistant its name + "Sir" address style (persisted in the general profile)
  useEffect(() => {
    seedAssistantMemory();
  }, []);

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

  // Escape closes the sidebar
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

  // Switching sessions closes any open subagent drill-in (render-time reset)
  const prevTabRef = useRef<string | null>(activeTabId);
  if (prevTabRef.current !== activeTabId) {
    prevTabRef.current = activeTabId;
    if (subagentView) setSubagentView(null);
  }

  useEffect(() => {
    if (activeTabType !== 'chat' || !activeTabChatId) return;
    if (chatRunning || chatLoading) return;
    if (generalActiveChatId !== activeTabChatId) {
      handleSelectChat(activeTabChatId, { silent: true });
    }
  }, [activeTabId, activeTabType, activeTabChatId, generalActiveChatId, chatRunning, chatLoading, handleSelectChat]);

  const openTasks = useCallback(async () => {
    if (!generalActiveChatId) await handleGeneralNewChat();
    setTaskModalOpen(true);
  }, [generalActiveChatId, handleGeneralNewChat]);

  // ── Session creation ──
  const handleNewSession = useCallback(() => {
    newSession();
  }, [newSession]);
  const handleNewChatIn = useCallback((workspaceId: string) => {
    newChatIn(workspaceId);
  }, [newChatIn]);
  const handleOpenSession = useCallback((workspaceId: string, chatId: string) => {
    openChat(workspaceId, chatId);
  }, [openChat]);

  /** New Project dialog → register workspace + create its first chat + open it. */
  const handleCreateProject = useCallback(async (folderPath: string, name: string) => {
    try {
      const ws = await addWorkspace(folderPath, name);
      const chatData = await createChat(ws.id, 'New Chat').catch(() => null);
      await refreshSessions();
      if (chatData) openChat(ws.id, chatData.id);
      return ws;
    } catch {
      return null;
    }
  }, [refreshSessions, openChat]);

  const handleRenameChat = useCallback((workspaceId: string, chatId: string, title: string) => {
    updateChatTitle(workspaceId, chatId, title)
      .then(() => refreshSessions())
      .catch(() => {});
  }, [refreshSessions]);

  const handleDeleteChat = useCallback((workspaceId: string, chatId: string) => {
    deleteChat(workspaceId, chatId)
      .then(() => {
        closeTab(tabId(workspaceId, chatId));
        return refreshSessions();
      })
      .catch(() => {});
  }, [closeTab, refreshSessions]);

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

  const launchDesign = useCallback(() => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/design';
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

  const launchAdmin = useCallback(() => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/admin';
    }, 320);
  }, [transitioning]);

  const activeGeneralTitle = useMemo(
    () => chat.chats.find((c) => c.id === chat.activeChatId)?.title ?? 'New conversation',
    [chat.chats, chat.activeChatId],
  );

  return (
    <div className="house-root fixed inset-0 flex flex-col overflow-hidden select-none">
      <HouseHeader
        tabs={sessions.tabs}
        activeTabId={sessions.activeTabId}
        runningTabId={sessions.runningTabId}
        onActivateTab={sessions.activateTab}
        onCloseTab={sessions.closeTab}
        onNewSession={handleNewSession}
        onOpenSidebar={() => setSidebarOpen(true)}
        tasks={chat.tasks}
        onOpenTasks={openTasks}
        onTaskStart={chat.handleTaskStart}
        onTaskDelete={chat.handleTaskDelete}
      />

      <main className="relative z-10 flex-1 min-h-0 flex flex-col overflow-hidden">
        {exitingIdeTab ? (
          <>
            {/* Chat is visible immediately under the exiting IDE — feels like the IDE slides away */}
            {subagentView ? (
              <div className="flex-1 min-h-0 flex flex-col max-w-[1400px] w-full mx-auto px-4 sm:px-8 lg:px-10 pt-2 pb-3">
                <div className="house-chat-card flex flex-col flex-1 min-h-0 rounded-2xl border house-hairline px-4 pt-2 pb-3 overflow-hidden">
                  <SubagentSessionView
                    workspaceId={chat.workspaceId ?? sessions.workspaces.find((w) => w.isGeneral)?.id ?? ''}
                    childId={subagentView.childId}
                    label={subagentView.label}
                    parentTitle={activeGeneralTitle}
                    onBack={() => setSubagentView(null)}
                  />
                </div>
              </div>
            ) : (
              <div className="flex-1 min-h-0 flex flex-col">
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
                  onOpenSubagent={(childId, label) => setSubagentView({ childId, label })}
                  workspaceId={chat.workspaceId}
                  chatId={activeTabChatId}
                />
              </div>
            )}
            <div className="absolute inset-0 z-10 ide-exit bg-[#0a0a0d]">
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
          <div key={activeTab.id} className="flex-1 min-h-0 flex flex-col ide-enter">
            <CodingIDE
              key={activeTab.id}
              session={activeTab}
              runner={runner}
              isGlobalRunning={sessions.isGlobalRunning}
              onCloseIde={handleCloseIde}
            />
          </div>
        ) : subagentView ? (
          <div className="flex-1 min-h-0 flex flex-col max-w-[1400px] w-full mx-auto px-4 sm:px-8 lg:px-10 pt-2 pb-3">
            <div className="house-chat-card flex flex-col flex-1 min-h-0 rounded-2xl border house-hairline px-4 pt-2 pb-3 overflow-hidden">
              <SubagentSessionView
                workspaceId={chat.workspaceId ?? sessions.workspaces.find((w) => w.isGeneral)?.id ?? ''}
                childId={subagentView.childId}
                label={subagentView.label}
                parentTitle={activeGeneralTitle}
                onBack={() => setSubagentView(null)}
              />
            </div>
          </div>
        ) : (
          <div className="flex-1 min-h-0 flex flex-col">
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
              onOpenSubagent={(childId, label) => setSubagentView({ childId, label })}
              workspaceId={chat.workspaceId}
              chatId={activeTabChatId}
            />
          </div>
        )}
      </main>

      <HouseSidebar
        open={sidebarOpen}
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
        onNewProject={() => setProjectDialogOpen(true)}
        onNewChatInProject={handleNewChatIn}
        onOpenRoom={handleLaunchRoom}
        onRenameChat={handleRenameChat}
        onDeleteChat={handleDeleteChat}
        onLaunchPersonal={launchPersonal}
        onLaunchCoding={launchCoding}
        onLaunchHouse={launchHouse}
        onLaunchAdmin={launchAdmin}
        onLaunchDesign={launchDesign}
      />

      {projectDialogOpen && (
        <NewProjectDialog
          onClose={() => setProjectDialogOpen(false)}
          onCreateProject={handleCreateProject}
        />
      )}

      <TokenStatsModal
        isOpen={isStatsOpen}
        onClose={() => setIsStatsOpen(false)}
        tokenUsage={chat.combinedTokenUsage}
        lastModelName={chat.lastModelName}
      />

      {taskModalOpen && (
        <AddTaskModal
          workspaceId={chat.workspaceId}
          chatId={chat.activeChatId}
          isRunning={chat.isRunning}
          onClose={() => setTaskModalOpen(false)}
          onCreated={chat.handleTaskCreated}
          onStartNow={chat.handleTaskRunNow}
        />
      )}

      {transitioning && (
        <HousePortal label="Entering Coding Workspace" icon={<Code2 size={22} />} />
      )}
    </div>
  );
}
