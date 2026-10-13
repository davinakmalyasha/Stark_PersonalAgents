import { useCallback, useEffect, useMemo, useState } from 'react';
import { ArrowLeft, ChevronDown, ChevronRight, LoaderCircle, Wrench } from 'lucide-react';
import type { ChatData, SessionTab, SubagentSession, SubagentSessionEntry, WorkspaceInfo } from '../types';
import { fetchChat, fetchSubagentSession, fetchWorkspaces } from '../services/api';
import MessageStream from '../components/Workspace/MessageStream';
import Markdown from '../components/Workspace/Markdown';
import PromptDock from '../components/Workspace/PromptDock';
import { useSessionRunner } from './useSessionRunner';
import './house.css';

const POLL_MS = 2500;

interface SubagentSessionViewProps {
  /** Workspace hint from the delegation step. It may be an id, path, or name. */
  workspaceId: string;
  childId: string;
  /** Fallback label while the child chat loads. */
  label: string;
  parentTitle: string;
  onBack: () => void;
  onOpenSubagentSession?: (childId: string, label: string, childWorkspace?: string) => void;
}

function ToolRow({ entry }: { entry: SubagentSessionEntry }) {
  const [open, setOpen] = useState(false);
  const firstLine = (entry.content || '').split('\n')[0] || '(no output)';
  return (
    <div className="rounded-lg border border-white/[0.06] bg-white/[0.02] overflow-hidden">
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        className="w-full flex items-center gap-2 px-2.5 py-1.5 text-left cursor-pointer hover:bg-white/[0.04] transition-colors"
      >
        {open
          ? <ChevronDown size={11} className="text-zinc-600 shrink-0" />
          : <ChevronRight size={11} className="text-zinc-600 shrink-0" />}
        <span className={`grid place-items-center w-4 h-4 rounded ${entry.isError ? 'text-red-400' : 'text-zinc-500'}`}>
          <Wrench size={10} />
        </span>
        <span className="text-[11.5px] font-medium text-zinc-400 shrink-0">{entry.toolName || 'tool'}</span>
        <span className="text-[11px] text-zinc-600 truncate flex-1">{firstLine}</span>
      </button>
      {open && (
        <pre className="px-3 py-2 text-[11px] leading-5 text-zinc-400 whitespace-pre-wrap max-h-56 overflow-y-auto border-t border-white/[0.05] font-mono">
          {entry.content}
        </pre>
      )}
    </div>
  );
}

function workspaceMatches(workspace: WorkspaceInfo, hint: string): boolean {
  const normalized = hint.trim().toLowerCase();
  if (!normalized) return false;
  return [workspace.id, workspace.path, workspace.name]
    .filter(Boolean)
    .some((value) => String(value).toLowerCase() === normalized);
}

function workspaceFallback(hint: string): WorkspaceInfo {
  return {
    id: hint,
    name: hint.replace(/^_/, '') || 'Subagent',
    path: hint,
    exists: true,
    chatCount: 0,
    addedAt: '',
  };
}

function pipelineKindFor(workspace: WorkspaceInfo): 'general' | 'house' | 'personal' {
  const kind = workspace.workspaceKind
    || (workspace.isHouse ? 'house' : workspace.isPersonal ? 'personal' : workspace.isGeneral ? 'general' : undefined);
  if (kind === 'house') return 'house';
  // The ordinary delegated General agent is the direct personal/general
  // worker. The default `general` kind would route a follow-up back through
  // the orchestrator, which would make the child drawer feel like a new root.
  if (kind === 'general' || workspace.id.toLowerCase().includes('general')) return 'personal';
  return 'general';
}

/**
 * Drill into the actual child chat created by delegation. New delegated
 * agents are normal chat-store records (`chat-*`), so they use the same
 * MessageStream and PromptDock as every other conversation. The durable
 * subagent-session projection remains as a compatibility fallback for older
 * `cont-*`/`sub-*` sessions.
 */
export default function SubagentSessionView({
  workspaceId,
  childId,
  label,
  parentTitle,
  onBack,
  onOpenSubagentSession,
}: SubagentSessionViewProps) {
  const [workspaces, setWorkspaces] = useState<WorkspaceInfo[]>([]);
  const [childChat, setChildChat] = useState<ChatData | null>(null);
  const [legacySession, setLegacySession] = useState<SubagentSession | null>(null);
  const [loadedKey, setLoadedKey] = useState('');
  const [errorKey, setErrorKey] = useState('');
  const requestKey = `${workspaceId}:${childId}`;

  const resolvedWorkspace = useMemo(() => {
    return workspaces.find((workspace) => workspaceMatches(workspace, workspaceId))
      || (workspaceId ? workspaceFallback(workspaceId) : null);
  }, [workspaces, workspaceId]);

  // Resolve the workspace hint once per opened child. Delegation metadata
  // stores the compact workspace id, while older traces sometimes stored a
  // path or display name.
  useEffect(() => {
    let cancelled = false;
    fetchWorkspaces()
      .then((items) => { if (!cancelled) setWorkspaces(items); })
      .catch(() => { /* the id fallback below can still load a known chat */ });
    return () => { cancelled = true; };
  }, [workspaceId, childId]);

  const loadChild = useCallback(async () => {
    if (!resolvedWorkspace) return;
    const resolvedId = resolvedWorkspace.id;
    const chat = await fetchChat(resolvedId, childId).catch(() => null);
    if (chat) {
      setChildChat(chat);
      setLegacySession(null);
      setErrorKey('');
      setLoadedKey(requestKey);
      return;
    }

    // Compatibility path for the old durable session endpoint. This is not
    // the primary path for delegated chat ids, but keeps historical links
    // usable instead of showing a misleading "blocked" state.
    const session = await fetchSubagentSession(resolvedId, childId).catch(() => null);
    if (session) {
      setLegacySession(session);
      setChildChat(null);
      setErrorKey('');
    } else {
      setErrorKey(requestKey);
    }
    setLoadedKey(requestKey);
  }, [resolvedWorkspace, childId, requestKey]);

  useEffect(() => {
    if (!resolvedWorkspace) return;
    let cancelled = false;
    const load = async () => {
      if (cancelled) return;
      await loadChild();
    };
    load();
    // Delegation can be opened while the child is still running and before
    // its first assistant message is persisted. Keep the drawer live until
    // it becomes a normal chat record.
    const timer = window.setInterval(() => { if (!document.hidden) load(); }, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [resolvedWorkspace, loadChild]);

  const childTab = useMemo<SessionTab | null>(() => {
    const activeChat = loadedKey === requestKey ? childChat : null;
    if (!activeChat || !resolvedWorkspace) return null;
    return {
      id: `${resolvedWorkspace.id}:${childId}`,
      type: 'chat',
      title: activeChat.title || label || 'Subagent',
      workspaceId: resolvedWorkspace.id,
      workspaceName: resolvedWorkspace.name,
      workspacePath: resolvedWorkspace.path,
      chatId: childId,
      updatedAt: activeChat.updatedAt || '',
    };
  }, [childChat, loadedKey, requestKey, resolvedWorkspace, childId, label]);

  const runner = useSessionRunner(childTab, undefined, childTab ? pipelineKindFor(resolvedWorkspace!) : 'general');
  const activeChildChat = loadedKey === requestKey ? childChat : null;
  const activeLegacySession = loadedKey === requestKey ? legacySession : null;
  // Full parity: merge persisted + runner messages (dedup by id) instead of
  // XOR-switching, so previous replies never vanish mid-run and rapid sends
  // never collapse into one bubble.
  const childMessages = (() => {
    const persisted = activeChildChat?.messages || [];
    const live = runner.messages;
    if (!live.length && !runner.isRunning) return persisted;
    if (!persisted.length) return live;
    const seen = new Set(persisted.map((m) => m.id));
    // Runner's first items may duplicate persisted when seeded after key switch.
    const fresh = live.filter((m) => !seen.has(m.id));
    // If runner already contains full history (post-fetch), prefer it.
    if (!fresh.length && live.length >= persisted.length) return live;
    return [...persisted, ...fresh];
  })();
  const running = runner.isRunning || activeLegacySession?.info.status === 'running';
  const childName = activeChildChat?.title || activeLegacySession?.info.name || label || 'Subagent';
  const displayLabel = childName && childName !== label ? childName : (label || childName);
  const isLoading = loadedKey !== requestKey && !errorKey;
  const hasLoadError = errorKey === requestKey;

  const handleSend = useCallback((prompt: string, files: File[]) => {
    if (childTab) runner.send(childTab, prompt, files);
  }, [childTab, runner]);

  return (
    <div className="subagent-session-view">
      <header className="subagent-session-view-header">
        <button
          type="button"
          onClick={onBack}
          title="Back to main session"
          aria-label="Back to main session"
          className="subagent-session-back"
        >
          <ArrowLeft size={15} />
        </button>
        <div className="subagent-session-breadcrumb">
          <button type="button" onClick={onBack} className="subagent-session-parent" title={parentTitle}>
            {parentTitle}
          </button>
          <span aria-hidden="true">/</span>
          <strong title={displayLabel}>{displayLabel}</strong>
        </div>
        {running && (
          <span className="subagent-session-status">
            <span className="session-run-dot" /> running
          </span>
        )}
      </header>

      <div className="subagent-session-view-body">
        {isLoading && !activeChildChat && !activeLegacySession ? (
          <div className="subagent-session-state">
            <LoaderCircle size={18} className="animate-spin" />
            <span>Opening child chat…</span>
          </div>
        ) : childTab ? (
          <MessageStream
            messages={childMessages}
            hasActiveChat
            onOpenSubagent={() => {}}
            onOpenSubagentSession={onOpenSubagentSession}
            workspaceId={childTab.workspaceId}
            chatId={childTab.chatId}
            onRetry={(prompt) => { if (prompt) runner.send(childTab, prompt, []); }}
          />
        ) : activeLegacySession ? (
          <div className="subagent-session-legacy-thread">
            {activeLegacySession.entries.length === 0 && (
              <p className="text-[12px] text-zinc-600 text-center py-6">No transcript recorded yet.</p>
            )}
            {activeLegacySession.entries.map((entry, index) => {
              if (entry.kind === 'user') {
                return (
                  <div key={index} className="flex justify-end house-enter">
                    <div className="max-w-[85%] px-3.5 py-2 rounded-2xl rounded-br-xs bg-white/[0.09] border border-white/[0.08] text-[13px] leading-6 text-zinc-100 whitespace-pre-wrap">
                      {entry.content}
                    </div>
                  </div>
                );
              }
              if (entry.kind === 'tool') return <ToolRow key={index} entry={entry} />;
              return (
                <div key={index} className="text-[13px] leading-6 text-zinc-200 house-enter">
                  <Markdown content={entry.content} />
                </div>
              );
            })}
          </div>
        ) : hasLoadError ? (
          <div className="subagent-session-state">
            <p>That child chat is no longer available.</p>
            <button type="button" onClick={loadChild} className="subagent-session-retry">Try again</button>
          </div>
        ) : null}
      </div>

      <div className="subagent-session-view-dock">
        {runner.notification && <div className="subagent-session-notice">{runner.notification}</div>}
        {childTab ? (
          <PromptDock
            onSend={handleSend}
            onCancel={runner.cancel}
            isRunning={runner.isRunning}
            isVoiceMode={false}
            onToggleVoice={() => {}}
            voiceInputText=""
            onOpenStats={() => {}}
            totalCost={runner.usage.total_cost}
            totalInput={runner.usage.total_input_tokens}
            totalOutput={runner.usage.total_output_tokens}
            totalCacheHitTokens={runner.usage.total_cache_hit_tokens || 0}
            activeContextTokens={runner.activeContextTokens}
          />
        ) : activeLegacySession ? (
          <div className="subagent-session-legacy-dock">This older child session is read-only.</div>
        ) : null}
      </div>
    </div>
  );
}
