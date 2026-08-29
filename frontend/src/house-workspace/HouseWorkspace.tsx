import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ArrowLeft, House, Menu, RotateCcw, Thermometer, Zap, Shield, Lightbulb, LoaderCircle,
  ChevronDown, ChevronUp, Sparkles,
} from 'lucide-react';
import FloorPlan from './FloorPlan';
import DeviceCard from './DeviceCard';
import SceneBar from './SceneBar';
import EnergyChart from './EnergyChart';
import type { HouseState, HouseDevice, DeviceState } from './house.types';
import { fetchHouseState, setDeviceState, runScene } from './houseApi';
import { useHouseChat } from '../house/useHouseChat';
import HouseSidebar from '../house/HouseSidebar';
import { useSessions, tabId } from '../house/useSessions';
import MessageStream from '../components/Workspace/MessageStream';
import PromptDock from '../components/Workspace/PromptDock';
import TokenStatsModal from '../components/Layout/TokenStatsModal';
import { updateChatTitle, deleteChat } from '../services/api';
import { getAssistantName, seedAssistantMemory } from '../house/assistant';
import './houseControl.css';
import '../house/house.css';

type Toast = { id: number; text: string } | null;

const HOUSE_CHAT_TITLE = 'House';

export default function HouseWorkspace() {
  const [house, setHouse] = useState<HouseState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [selectedRoom, setSelectedRoom] = useState<string | null>(null);
  const [busyDevice, setBusyDevice] = useState<string | null>(null);
  const [busyScene, setBusyScene] = useState(false);
  const [toast, setToast] = useState<Toast>(null);
  const [chatOpen, setChatOpen] = useState(true);
  const [isStatsOpen, setIsStatsOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const toastTimer = useRef<number | null>(null);

  // Chat index for the hamburger sidebar (same session layer as the main
  // workspace — routes are exclusive, so only one instance polls at a time).
  const sessions = useSessions();

  // Escape closes the sidebar
  useEffect(() => {
    if (!sidebarOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setSidebarOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [sidebarOpen]);

  const go = useCallback((hash: string) => {
    setSidebarOpen(false);
    window.location.hash = hash;
  }, []);

  const handleRenameChat = useCallback((workspaceId: string, chatId: string, title: string) => {
    updateChatTitle(workspaceId, chatId, title)
      .then(() => sessions.refreshSessions())
      .catch(() => {});
  }, [sessions]);

  const handleDeleteChat = useCallback((workspaceId: string, chatId: string) => {
    deleteChat(workspaceId, chatId)
      .then(() => {
        sessions.closeTab(tabId(workspaceId, chatId));
        return sessions.refreshSessions();
      })
      .catch(() => {});
  }, [sessions]);

  const openChatAtHome = useCallback((workspaceId: string, chatId: string) => {
    go(`#/?ws=${encodeURIComponent(workspaceId)}&chat=${encodeURIComponent(chatId)}`);
  }, [go]);

  // Dedicated "House" conversation — bound to the _house workspace + house agent.
  const chat = useHouseChat({ pinnedChatTitle: HOUSE_CHAT_TITLE, workspaceKind: 'house' });
  const assistantName = getAssistantName();

  useEffect(() => {
    seedAssistantMemory();
  }, []);

  const showToast = useCallback((text: string) => {
    if (toastTimer.current) window.clearTimeout(toastTimer.current);
    setToast({ id: Date.now(), text });
    toastTimer.current = window.setTimeout(() => setToast(null), 3200);
  }, []);

  const load = useCallback(async () => {
    try {
      const state = await fetchHouseState();
      setHouse(state);
      setError(null);
      setSelectedRoom((cur) => cur ?? state.rooms[0]?.id ?? null);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'House backend unreachable');
    }
  }, []);

  // Deferred boot fetch (matches useHouseChat's deferred pattern)
  useEffect(() => {
    const id = window.setTimeout(() => { load(); }, 0);
    return () => window.clearTimeout(id);
  }, [load]);

  const devices = useMemo(() => house?.devices ?? [], [house]);

  const stats = useMemo(() => {
    const thermo = devices.find((d) => d.type === 'thermostat');
    const lightsOn = devices.filter((d) => d.type === 'light' && d.state.power).length;
    const lightsTotal = devices.filter((d) => d.type === 'light').length;
    const unlocked = devices.filter((d) => d.type === 'lock' && !d.state.locked).length;
    const cams = devices.filter((d) => d.type === 'camera');
    const camsLive = cams.filter((d) => d.state.recording).length;
    return {
      temp: thermo?.state.currentTemp ?? null,
      target: thermo?.state.targetTemp ?? null,
      lightsOn, lightsTotal,
      unlocked,
      cams: `${camsLive}/${cams.length}`,
      armed: house?.security.armed ?? false,
    };
  }, [devices, house]);

  const handleChange = useCallback(async (deviceId: string, patch: Partial<DeviceState>) => {
    setBusyDevice(deviceId);
    try {
      const updated: HouseDevice = await setDeviceState(deviceId, patch as Record<string, unknown>);
      setHouse((cur) => cur
        ? { ...cur, devices: cur.devices.map((d) => (d.id === updated.id ? updated : d)) }
        : cur);
    } finally {
      setBusyDevice(null);
    }
  }, []);

  const handleScene = useCallback(async (sceneId: string) => {
    setBusyScene(true);
    try {
      const result = await runScene(sceneId);
      showToast(`Scene "${result.name}" — ${result.applied.length} device(s) updated`);
      await load();
    } catch (e) {
      showToast(e instanceof Error ? e.message : 'Scene failed');
    } finally {
      setBusyScene(false);
    }
  }, [load, showToast]);

  const roomDevices = useMemo(
    () => (selectedRoom ? devices.filter((d) => d.roomId === selectedRoom) : devices),
    [devices, selectedRoom],
  );
  const roomName = house?.rooms.find((r) => r.id === selectedRoom)?.name ?? 'All rooms';

  if (error) {
    return (
      <div className="hw-root fixed inset-0 grid place-items-center">
        <div className="text-center px-6">
          <House size={28} className="mx-auto text-zinc-600 mb-3" />
          <p className="text-[13px] text-zinc-400">{error}</p>
          <p className="text-[11px] text-zinc-600 mt-1">Is the API server running on port 8123?</p>
          <button
            type="button"
            onClick={load}
            className="mt-4 inline-flex items-center gap-1.5 h-8 px-3.5 rounded-lg bg-white text-[#0a0a0d] text-[12px] font-semibold hover:bg-zinc-200 transition-colors cursor-pointer"
          >
            <RotateCcw size={12} /> Retry
          </button>
        </div>
      </div>
    );
  }

  if (!house) {
    return (
      <div className="hw-root fixed inset-0 grid place-items-center">
        <LoaderCircle size={22} className="text-zinc-500 animate-spin" />
      </div>
    );
  }

  return (
    <div className="hw-root fixed inset-0 flex flex-col overflow-hidden">
      {/* ── Header ── */}
      <header
        className="flex items-center justify-between px-4 sm:px-6 h-[52px] border-b house-hairline shrink-0"
        style={{ WebkitAppRegion: 'drag' } as React.CSSProperties}
      >
        <div className="flex items-center gap-2.5" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
          <button
            type="button"
            onClick={() => setSidebarOpen(true)}
            title="Chat history"
            aria-label="Open chat history"
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
          >
            <Menu size={16} />
          </button>
          <button
            type="button"
            onClick={() => { window.location.hash = ''; }}
            title="Back to home"
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-400 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
          >
            <ArrowLeft size={16} />
          </button>
          <div className="flex items-center gap-2.5">
            <div className="grid place-items-center w-7 h-7 rounded-lg bg-white text-[#0a0a0d]">
              <House size={14} />
            </div>
            <span className="house-font-display text-[15px] font-bold tracking-wide text-white">
              House Workspace
            </span>
          </div>
          <span className="hidden sm:flex items-center gap-1.5 ml-2 text-[10.5px] text-zinc-500">
            <span className="hw-live-dot !w-1.5 !h-1.5" />
            live · mock backend
          </span>
        </div>
        <div className="flex items-center gap-3" style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties}>
          <span className={`text-[10.5px] px-2.5 py-1 rounded-full border ${
            stats.armed ? 'border-white/40 text-white bg-white/10' : 'border-white/10 text-zinc-500'
          }`}>
            <Shield size={10} className="inline mr-1 -mt-0.5" />
            {stats.armed ? 'Armed' : 'Disarmed'}
          </span>
          <button
            type="button"
            onClick={load}
            title="Refresh"
            className="grid place-items-center w-8 h-8 rounded-lg text-zinc-500 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
          >
            <RotateCcw size={13} />
          </button>
        </div>
      </header>

      {/* ── Body: page scrolls on mobile; independent columns on lg, chat dock pinned bottom-right ── */}
      <main className="flex-1 min-h-0 overflow-y-auto hw-scroll lg:overflow-hidden">
        <div className="max-w-[1400px] mx-auto px-4 sm:px-8 lg:px-12 py-5 lg:h-full flex flex-col min-h-0 gap-4">
          {/* Stat strip */}
          <div className="grid grid-cols-2 lg:grid-cols-4 gap-3 hw-rise-1 shrink-0">
            <div className="hw-panel rounded-xl p-3.5 flex items-center gap-3">
              <span className="hw-device-icon grid place-items-center w-9 h-9 rounded-lg text-zinc-300"><Thermometer size={16} /></span>
              <div>
                <div className="hw-stat-value text-[18px] font-bold text-white leading-none">
                  {stats.temp != null ? `${stats.temp.toFixed(1)}°` : '—'}
                </div>
                <div className="text-[10px] text-zinc-500 mt-1">
                  {stats.target != null ? `target ${stats.target.toFixed(1)}°` : 'no thermostat'}
                </div>
              </div>
            </div>
            <div className="hw-panel rounded-xl p-3.5 flex items-center gap-3">
              <span className="hw-device-icon grid place-items-center w-9 h-9 rounded-lg text-zinc-300"><Lightbulb size={16} /></span>
              <div>
                <div className="hw-stat-value text-[18px] font-bold text-white leading-none">{stats.lightsOn}<span className="text-[11px] text-zinc-500 font-medium">/{stats.lightsTotal}</span></div>
                <div className="text-[10px] text-zinc-500 mt-1">lights on</div>
              </div>
            </div>
            <div className="hw-panel rounded-xl p-3.5 flex items-center gap-3">
              <span className="hw-device-icon grid place-items-center w-9 h-9 rounded-lg text-zinc-300"><Shield size={16} /></span>
              <div>
                <div className="hw-stat-value text-[18px] font-bold text-white leading-none">
                  {stats.unlocked === 0 ? 'Secure' : `${stats.unlocked} open`}
                </div>
                <div className="text-[10px] text-zinc-500 mt-1">cams {stats.cams} recording</div>
              </div>
            </div>
            <div className="hw-panel rounded-xl p-3.5">
              <EnergyChart energy={house.energy} />
            </div>
          </div>

          {/* Plan + room + chat dock */}
          <div className="grid lg:grid-cols-[380px_1fr] gap-4 flex-1 min-h-0">
            {/* Left: floor plan + scenes (independent scroll on lg) */}
            <div className="space-y-4 hw-rise-2 lg:min-h-0 lg:overflow-y-auto hw-scroll lg:pr-1">
              <div className="hw-panel rounded-xl p-4">
                <div className="text-[10px] uppercase tracking-widest font-semibold text-zinc-600 mb-3">Floor plan</div>
                <FloorPlan
                  rooms={house.rooms}
                  devices={devices}
                  selectedRoomId={selectedRoom}
                  onSelectRoom={setSelectedRoom}
                />
                <div className="flex items-center justify-between mt-3 text-[10px] text-zinc-600">
                  <span className="flex items-center gap-1.5"><span className="w-2 h-2 rounded-full bg-white inline-block" /> active</span>
                  <span className="flex items-center gap-1.5"><span className="w-1.5 h-1.5 rounded-full bg-white/30 inline-block" /> idle</span>
                  <span>click a room</span>
                </div>
              </div>
              <div className="hw-panel rounded-xl p-4">
                <div className="text-[10px] uppercase tracking-widest font-semibold text-zinc-600 mb-3">Scenes</div>
                <SceneBar scenes={house.scenes} busy={busyScene} onRun={handleScene} />
              </div>
            </div>

            {/* Right: room devices + house agent chat dock */}
            <div className="hw-rise-3 flex flex-col gap-3 lg:min-h-0">
              <div className="flex items-center justify-between shrink-0">
                <div className="flex items-center gap-2">
                  <Zap size={13} className="text-zinc-500" />
                  <span className="text-[13px] font-semibold text-zinc-200">{roomName}</span>
                  <span className="text-[10.5px] text-zinc-600">{roomDevices.length} devices</span>
                </div>
                <div className="flex gap-1.5">
                  {house.rooms.map((r) => (
                    <button
                      key={r.id}
                      type="button"
                      onClick={() => setSelectedRoom(r.id)}
                      className={`text-[10.5px] px-2.5 py-1 rounded-lg border transition-colors cursor-pointer ${
                        r.id === selectedRoom
                          ? 'border-white/40 text-white bg-white/10'
                          : 'border-white/10 text-zinc-500 hover:text-zinc-200 hover:border-white/25'
                      }`}
                    >
                      {r.name}
                    </button>
                  ))}
                </div>
              </div>
              <div className="grid sm:grid-cols-2 xl:grid-cols-3 gap-3 content-start auto-rows-min lg:flex-1 lg:min-h-[120px] lg:overflow-y-auto hw-scroll lg:pr-1">
                {roomDevices.map((d) => (
                  <DeviceCard key={d.id} device={d} busy={busyDevice === d.id} onChange={handleChange} />
                ))}
              </div>

              {/* ── House agent chat dock (dedicated "House" conversation) ── */}
              <div className="hw-panel rounded-xl flex flex-col shrink-0">
                <div className="flex items-center justify-between px-4 pt-3 pb-1.5 shrink-0">
                  <div className="flex items-center gap-2 select-none">
                    <Sparkles size={13} className="text-zinc-400" />
                    <span className="house-font-display text-[12px] font-semibold tracking-wide text-zinc-200">
                      {assistantName}
                    </span>
                    <span className="house-status-dot" />
                    <span className="text-[10px] text-zinc-600">house agent</span>
                    {chat.isRunning && (
                      <span className="flex gap-1 ml-1">
                        <span className="house-typing-dot" />
                        <span className="house-typing-dot" />
                        <span className="house-typing-dot" />
                      </span>
                    )}
                  </div>
                  <button
                    type="button"
                    onClick={() => setChatOpen((o) => !o)}
                    title={chatOpen ? 'Hide conversation' : 'Show conversation'}
                    className="grid place-items-center w-6 h-6 rounded-md text-zinc-500 hover:text-white hover:bg-white/[0.07] transition-colors cursor-pointer"
                  >
                    {chatOpen ? <ChevronDown size={14} /> : <ChevronUp size={14} />}
                  </button>
                </div>

                {chatOpen && (
                  <div className="h-[clamp(140px,22vh,230px)] min-h-0 flex flex-col px-2">
                    <div ref={chat.threadRef} className="flex-1 min-h-0 overflow-y-auto">
                      {chat.isLoading ? (
                        <div className="h-full flex flex-col items-center justify-center gap-3 text-zinc-500 py-6">
                          <div className="flex gap-1.5">
                            <span className="house-typing-dot" />
                            <span className="house-typing-dot" />
                            <span className="house-typing-dot" />
                          </div>
                          <span className="text-[12px]">Connecting...</span>
                        </div>
                      ) : (
                        <MessageStream
                          messages={chat.displayMessages}
                          hasActiveChat={chat.displayMessages.length > 0}
                          onOpenSubagent={() => {}}
                          onOpenSubagentSession={undefined}
                          workspaceId={chat.workspaceId}
                          chatId={chat.activeChatId}
                        />
                      )}
                    </div>
                  </div>
                )}

                {chat.notification && (
                  <div className="mx-4 mb-2 py-1.5 text-[11px] text-zinc-400 bg-white/[0.05] border border-white/10 rounded px-3">
                    {chat.notification}
                  </div>
                )}

                <div className="px-3 pb-3 pt-1 shrink-0">
                  <PromptDock
                    onSend={chat.handleSend}
                    onCancel={chat.handleCancel}
                    isRunning={chat.isRunning}
                    isVoiceMode={chat.isVoiceMode}
                    onToggleVoice={chat.toggleVoice}
                    voiceInputText=""
                    onOpenStats={() => setIsStatsOpen(true)}
                    totalCost={chat.totalCost}
                    totalInput={chat.totalInput}
                    totalOutput={chat.totalOutput}
                    totalCacheHitTokens={chat.totalCacheHitTokens}
                    activeContextTokens={chat.activeContextTokens}
                  />
                </div>
              </div>
            </div>
          </div>
        </div>
      </main>

      {/* Hamburger sidebar (room variant: search + all chats + workspaces) */}
      <HouseSidebar
        variant="room"
        open={sidebarOpen}
        onClose={() => setSidebarOpen(false)}
        workspaces={sessions.workspaces}
        chatsByWs={sessions.chatsByWs}
        activeTabId={sessions.activeTabId}
        runningTabId={sessions.runningTabId}
        pinnedChats={sessions.pinnedChats}
        onTogglePin={sessions.togglePin}
        onOpenSession={openChatAtHome}
        onRenameChat={handleRenameChat}
        onDeleteChat={handleDeleteChat}
        onLaunchPersonal={() => go('#/personal')}
        onLaunchCoding={() => go('#/coding')}
        onLaunchHouse={() => go('#/house')}
      />

      {/* Toast */}
      {toast && (
        <div className="hw-toast fixed bottom-5 left-1/2 -translate-x-1/2 z-50 px-4 py-2.5 rounded-xl border house-hairline bg-[#141419]/95 backdrop-blur-xl shadow-2xl text-[12px] text-zinc-200">
          {toast.text}
        </div>
      )}

      <TokenStatsModal
        isOpen={isStatsOpen}
        onClose={() => setIsStatsOpen(false)}
        tokenUsage={chat.combinedTokenUsage}
        lastModelName={chat.lastModelName}
      />
    </div>
  );
}
