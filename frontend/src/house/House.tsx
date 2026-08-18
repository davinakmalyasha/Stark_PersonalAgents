import { useCallback, useEffect, useMemo, useState } from 'react';
import { Code2 } from 'lucide-react';
import HouseNeurons from './HouseNeurons';
import HouseHeader from './HouseHeader';
import HouseChat from './HouseChat';
import HouseSidebar from './HouseSidebar';
import HousePortal from './HousePortal';
import AddTaskModal from '../components/Canvas/AddTaskModal';
import TokenStatsModal from '../components/Layout/TokenStatsModal';
import { useHouseChat } from './useHouseChat';
import { seedAssistantMemory } from './assistant';
import { fetchWorkspaces, fetchChats } from '../services/api';
import '../house/house.css';

export default function House() {
  const [transitioning, setTransitioning] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const [isStatsOpen, setIsStatsOpen] = useState(false);
  const [taskModalOpen, setTaskModalOpen] = useState(false);
  const [workspaceCount, setWorkspaceCount] = useState<number | null>(null);
  const [chatCount, setChatCount] = useState<number | null>(null);
  const [clock, setClock] = useState(() => new Date());
  const chat = useHouseChat({});

  // Teach the assistant its name + "Sir" address style (persisted in the general profile)
  useEffect(() => {
    seedAssistantMemory();
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const wsList = await fetchWorkspaces();
        if (cancelled) return;
        setWorkspaceCount(wsList.length);
        const counts = await Promise.all(wsList.map((w) => fetchChats(w.id).catch(() => [])));
        if (!cancelled) setChatCount(counts.reduce((sum, c) => sum + c.length, 0));
      } catch { /* stats are decorative — silent fail */ }
    })();
    return () => { cancelled = true; };
  }, []);

  useEffect(() => {
    const id = setInterval(() => setClock(new Date()), 1000 * 30);
    return () => clearInterval(id);
  }, []);

  // Escape closes the sidebar
  useEffect(() => {
    if (!sidebarOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setSidebarOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [sidebarOpen]);

  const timeStr = useMemo(() => clock.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }), [clock]);

  const openTasks = useCallback(async () => {
    if (!chat.activeChatId) await chat.handleNewChat();
    setTaskModalOpen(true);
  }, [chat]);

  const launchCoding = useCallback(() => {
    if (transitioning) return;
    setSidebarOpen(false);
    setTransitioning(true);
    setTimeout(() => {
      window.location.hash = '#/coding';
    }, 320);
  }, [transitioning]);

  return (
    <div className="house-root fixed inset-0 flex flex-col overflow-hidden select-none">
      <HouseNeurons className="absolute inset-0 w-full h-full opacity-40 pointer-events-none" />

      <HouseHeader
        timeStr={timeStr}
        isRunning={chat.isRunning}
        tasks={chat.tasks}
        onOpenSidebar={() => setSidebarOpen(true)}
        onNewChat={chat.handleNewChat}
        onOpenTasks={openTasks}
        onTaskStart={chat.handleTaskStart}
        onTaskDelete={chat.handleTaskDelete}
      />

      <main className="relative z-10 flex-1 min-h-0 flex flex-col overflow-hidden">
        <div className="flex-1 min-h-0 flex flex-col">
          <HouseChat
            chats={chat.chats}
            activeChatId={chat.activeChatId}
            messages={chat.displayMessages}
            isRunning={chat.isRunning}
            isLoading={chat.isLoading}
            notification={chat.notification}
            threadRef={chat.threadRef}
            isVoiceMode={chat.isVoiceMode}
            totalCost={chat.totalCost}
            totalInput={chat.totalInput}
            totalOutput={chat.totalOutput}
            totalCacheHitTokens={chat.totalCacheHitTokens}
            activeContextTokens={chat.activeContextTokens}
            onSend={chat.handleSend}
            onCancel={chat.handleCancel}
            onToggleVoice={chat.toggleVoice}
            onOpenStats={() => setIsStatsOpen(true)}
          />
        </div>
      </main>

      <HouseSidebar
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        chats={chat.chats}
        activeChatId={chat.activeChatId}
        isRunning={chat.isRunning}
        workspaceCount={workspaceCount}
        chatCount={chatCount}
        onNewChat={chat.handleNewChat}
        onSelectChat={chat.handleSelectChat}
        onRenameChat={chat.handleRenameChat}
        onDeleteChat={chat.handleDeleteChat}
        onLaunchCoding={launchCoding}
      />

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
