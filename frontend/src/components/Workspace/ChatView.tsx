import { useEffect, useState } from 'react';
import { X, Minimize2, Maximize2, Search, Download, RotateCcw } from 'lucide-react';
import type { ChatMessage, AppState, ModelOption } from '../../types';
import type { TraceStep } from '../../utils/logParser';
import PromptDock from './PromptDock';
import PlanDock from './PlanDock';
import MessageStream from './MessageStream';
import { getChatSnapshots, restoreRun, type SnapshotGroup } from '../../services/api';

interface Props {
  messages: ChatMessage[];
  isRunning: boolean;
  workspaceName: string | null;
  chatTitle: string | null;
  selectedModel: string;
  models: ModelOption[];
  state: AppState | null;
  hasActiveChat: boolean;
  notification: string | null;
  chatMode?: 'plan' | 'build';
  onSend: (message: string) => void;
  onCancel: () => void;
  onModelChange: (model: string) => void;
  onClearNotification: () => void;
  isVoiceMode?: boolean;
  onToggleVoice: () => void;
  voiceInputText?: string;
  onOpenStats: () => void;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  totalCacheHitTokens: number;
  activeContextTokens: number;
  traceSteps?: TraceStep[];
  workspaceId: string | null;
  chatId: string | null;
  onOpenProject?: (ref: { workspace_id: string; chat_id: string }) => void;
  onRetry?: (prompt: string) => void;
  /** Session-keyed subagent drill-in (delegation rows carrying childId). */
  onOpenSubagentSession?: (childId: string, label: string) => void;
}

export default function ChatView({
  messages, isRunning,
  hasActiveChat, notification, chatMode = 'build',
  onSend, onCancel, onClearNotification,
  isVoiceMode, onToggleVoice, voiceInputText,   onOpenStats, totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens, traceSteps = [], workspaceId, chatId, onOpenProject,
  selectedModel, models, onModelChange, onRetry, state, onOpenSubagentSession,
}: Props) {
  const [minimized, setMinimized] = useState(false);
  const [activeSubagent, setActiveSubagent] = useState<string | null>(null);
  const [searchQuery, setSearchQuery] = useState('');
  const [showSearch, setShowSearch] = useState(false);
  const [showRewind, setShowRewind] = useState(false);

  // P10/Tier-3: client-side chat search — only show matching messages.
  const visibleMessages = searchQuery.trim()
    ? messages.filter((m) => (m.content || '').toLowerCase().includes(searchQuery.trim().toLowerCase()))
    : messages;

  // P10/Tier-3: export the chat as Markdown (blob download).
  const exportChat = () => {
    const lines: string[] = [];
    for (const m of messages) {
      if (!m.content) continue;
      if (m.role === 'user') lines.push(`**You:**\n\n${m.content}\n`);
      else if (m.role === 'agent') lines.push(`**Assistant${m.agentName ? ` (${m.agentName})` : ''}:**\n\n${m.content}\n`);
      else if (m.role === 'system') lines.push(`> ${m.content}\n`);
      else if (m.role === 'error') lines.push(`**Error:**\n\n${m.content}\n`);
    }
    const blob = new Blob([lines.join('\n---\n\n')], { type: 'text/markdown;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `chat-${new Date().toISOString().slice(0, 10)}.md`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div 
      className="absolute bottom-6 left-3 right-3 z-40 w-auto max-w-none flex flex-col pointer-events-auto rounded-2xl border border-zinc-800/50 backdrop-blur-md shadow-2xl transition-all duration-300 premium-surface"
      style={{
        maxHeight: '80vh',
      }}
    >
      {/* Floating minimize/expand button (icon only, proper bg color, bigger icon) */}
      <button
        onClick={() => setMinimized((m) => !m)}
        className="absolute top-3.5 right-4 z-50 p-2 bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 hover:border-zinc-700/80 rounded-xl text-zinc-400 hover:text-zinc-100 transition-all cursor-pointer shadow-xl flex items-center justify-center"
        title={minimized ? "Expand chat history" : "Collapse chat history"}
        style={{
          width: '32px',
          height: '32px',
        }}
      >
        {minimized ? (
          <Maximize2 size={16} className="text-indigo-400" />
        ) : (
          <Minimize2 size={16} />
        )}
      </button>

      {/* Chat toolbar: mode badge + search + export + rewind (right of the minimize button) */}
      <div className="absolute top-3.5 right-[52px] z-50 flex items-center gap-1.5">
        {chatMode === 'plan' && (
          <span
            className="pressable px-2 py-1 bg-amber-900/60 border border-amber-700/60 text-amber-200 rounded-lg text-[11px] font-semibold tracking-wide shadow-xl"
            title="Plan mode: read-only. The agent can research and propose_plan, but cannot edit. Type /build to leave plan mode."
          >
            PLAN
          </span>
        )}
        <button
          onClick={() => { setShowSearch((v) => !v); setSearchQuery(''); }}
          className="pressable p-2 bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 hover:border-zinc-700/80 rounded-xl text-zinc-400 hover:text-zinc-100 transition-all cursor-pointer shadow-xl flex items-center justify-center"
          title={showSearch ? 'Close search' : 'Search in chat'}
          style={{ width: '32px', height: '32px' }}
        >
          <Search size={14} />
        </button>
        <button
          onClick={exportChat}
          className="pressable p-2 bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 hover:border-zinc-700/80 rounded-xl text-zinc-400 hover:text-zinc-100 transition-all cursor-pointer shadow-xl flex items-center justify-center"
          title="Export chat as Markdown"
          style={{ width: '32px', height: '32px' }}
        >
          <Download size={14} />
        </button>
        <button
          onClick={() => setShowRewind((v) => !v)}
          className="pressable p-2 bg-zinc-900/95 border border-zinc-800/80 hover:bg-zinc-800 hover:border-zinc-700/80 rounded-xl text-zinc-400 hover:text-zinc-100 transition-all cursor-pointer shadow-xl flex items-center justify-center"
          title="Rewind chat to a previous point"
          style={{ width: '32px', height: '32px' }}
        >
          <RotateCcw size={14} />
        </button>
      </div>
      {showRewind && workspaceId && chatId && (
        <RewindPopover
          chatId={chatId}
          workspaceId={workspaceId}
          onClose={() => setShowRewind(false)}
        />
      )}
      {showSearch && (
        <div className="absolute top-3.5 left-3 right-[100px] z-50">
          <input
            autoFocus
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            placeholder="Search messages…"
            className="w-full px-3 py-1.5 rounded-xl bg-zinc-900/95 border border-zinc-800/80 text-[12.5px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none focus:border-indigo-500/50"
          />
        </div>
      )}

      {/* Collapsible Area */}
      <div 
        className="flex-1 flex flex-col overflow-y-auto transition-all duration-300 ease-in-out"
        style={{
          maxHeight: minimized ? '0px' : '480px',
          opacity: minimized ? 0 : 1,
        }}
      >
        {/* Notifications */}
        {notification && (
          <div className="flex items-center gap-2.5 mx-4 mt-2 px-3 py-2 rounded-lg bg-blue-500/10 border border-blue-500/20 text-[11px] text-blue-300 flex-shrink-0">
            <span className="flex-1 truncate">{notification}</span>
            <button onClick={onClearNotification} className="p-0.5 text-blue-400 hover:text-blue-200">
              <X size={12} />
            </button>
          </div>
        )}

        {/* Message stream */}
        <MessageStream
          messages={visibleMessages}
          hasActiveChat={hasActiveChat}
          onOpenSubagent={(name) => setActiveSubagent(name)}
          onOpenSubagentSession={onOpenSubagentSession}
          activeSubagent={activeSubagent}
          traceSteps={traceSteps}
          workspaceId={workspaceId}
          chatId={chatId}
          onCloseSubagent={() => setActiveSubagent(null)}
          onOpenProject={onOpenProject}
          onRetry={onRetry}
        />
      </div>

      {/* Always visible Bottom Prompt Dock */}
      <div className="border-t border-zinc-800/50 flex-shrink-0">
        <PlanDock plan={state?.plan} />
        <PromptDock onSend={onSend} onCancel={onCancel} isRunning={isRunning} isVoiceMode={isVoiceMode} onToggleVoice={onToggleVoice} voiceInputText={voiceInputText} onOpenStats={onOpenStats} totalCost={totalCost} totalInput={totalInput} totalOutput={totalOutput} totalCacheHitTokens={totalCacheHitTokens} activeContextTokens={activeContextTokens} selectedModel={selectedModel} models={models} onModelChange={onModelChange} />
      </div>
    </div>
  );
}

// Stage 3: chat-level rewind popover
function RewindPopover({ chatId, workspaceId, onClose }: { chatId: string; workspaceId: string; onClose: () => void }) {
  const [groups, setGroups] = useState<SnapshotGroup[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [restoring, setRestoring] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const result = await getChatSnapshots(chatId, workspaceId);
        if (!cancelled) setGroups(result);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [chatId, workspaceId]);

  const handleRestore = async (runId: string) => {
    if (restoring) return;
    if (!confirm('Restore every file touched by this run? This will overwrite the current state of those files.')) return;
    setRestoring(runId);
    try {
      const result = await restoreRun(chatId, runId, workspaceId);
      alert(result.ok ? 'Restored.' : `Some files were skipped:\n${result.messages.join('\n')}`);
      onClose();
    } catch (e) {
      alert(`Restore failed: ${e}`);
    } finally {
      setRestoring(null);
    }
  };

  return (
    <div className="absolute top-14 right-3 z-50 w-96 max-h-[70vh] overflow-y-auto bg-zinc-900/98 border border-zinc-700/80 rounded-2xl shadow-2xl p-3">
      <div className="flex items-center justify-between mb-2">
        <strong className="text-zinc-100 text-sm">Rewind chat</strong>
        <button onClick={onClose} className="text-zinc-500 hover:text-zinc-200 text-xs">Close</button>
      </div>
      {loading && <div className="text-zinc-500 text-xs py-4 text-center">Loading snapshots…</div>}
      {error && <div className="text-rose-400 text-xs py-4">{error}</div>}
      {!loading && !error && groups.length === 0 && (
        <div className="text-zinc-500 text-xs py-4 text-center">No snapshots yet. Snapshots are captured automatically when the agent writes files.</div>
      )}
      <ul className="space-y-1.5">
        {groups.map((g) => (
          <li key={g.run_id} className="bg-zinc-800/60 rounded-lg p-2 text-xs">
            <div className="flex items-center justify-between gap-2">
              <div className="flex-1 min-w-0">
                <div className="text-zinc-200 truncate">{new Date(g.created_at * 1000).toLocaleString()}</div>
                <div className="text-zinc-500 truncate">{g.files.length} file{g.files.length === 1 ? '' : 's'} · {(g.total_size / 1024).toFixed(1)} KB</div>
              </div>
              <button
                onClick={() => handleRestore(g.run_id)}
                disabled={restoring === g.run_id}
                className="pressable px-2 py-1 bg-indigo-600 hover:bg-indigo-500 disabled:bg-zinc-700 text-white rounded-md text-[11px] font-medium transition-colors"
              >
                {restoring === g.run_id ? 'Restoring…' : 'Restore'}
              </button>
            </div>
            <ul className="mt-1.5 space-y-0.5 text-[10px] text-zinc-500">
              {g.files.slice(0, 4).map((f) => (
                <li key={f.path} className="truncate font-mono">{f.path}</li>
              ))}
              {g.files.length > 4 && <li className="text-zinc-600">+{g.files.length - 4} more…</li>}
            </ul>
          </li>
        ))}
      </ul>
    </div>
  );
}
