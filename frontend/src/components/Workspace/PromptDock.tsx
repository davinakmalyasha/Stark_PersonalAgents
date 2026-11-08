import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import { ArrowUp, Check, ChevronDown, Cloud, CloudOff, FileText, Image, Plus, Square, X, AudioLines } from 'lucide-react';
import type { ChatAttachment, ModelOption } from '../../types';
import { chatAttachmentUrl, fetchProvider, type ProviderInfo } from '../../services/api';
import { useOptionalAttachmentDrop } from './AttachmentDrop';
import AskQuestionPanel from './AskQuestionPanel';
import { dismissAskBatch, useAskBatchForChat } from '../../hooks/usePendingAsk';

interface Props {
  onSend: (message: string, files: File[], restored?: ChatAttachment[]) => void;
  onCancel?: () => void;
  isRunning: boolean;
  isVoiceMode?: boolean;
  onToggleVoice: () => void;
  voiceInputText?: string;
  onOpenStats: () => void;
  onOpenAgentConfig?: () => void;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  totalCacheHitTokens: number;
  activeContextTokens: number;
  selectedModel?: string;
  models?: ModelOption[];
  onModelChange?: (model: string) => void;
  /** Restored text from a rewind; non-empty values populate the input once. */
  draft?: string;
  /** Attachments restored with a rewound message — sent without re-uploading. */
  draftAttachments?: ChatAttachment[];
  onDraftConsumed?: () => void;
  onDraftAttachmentsConsumed?: () => void;
  /** Used to preview persisted image/audio attachments. */
  workspaceId?: string | null;
  chatId?: string | null;
}

const MAX_FILES = 8;
const MAX_FILE_SIZE = 20 * 1024 * 1024;
const ACCEPTED_TYPES = 'image/*,audio/*,.pdf,.txt,.md,.json,.js,.ts,.tsx,.py,.html,.css,.csv,.doc,.docx,.zip';
const formatSize = (bytes: number) => bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
const isImage = (file: File) => file.type.startsWith('image/');
const isAudio = (file: File) => file.type.startsWith('audio/');

function LocalFileThumb({ file }: { file: File }) {
  const preview = useMemo(() => (isImage(file) ? URL.createObjectURL(file) : ''), [file]);
  useEffect(() => {
    if (!preview) return undefined;
    return () => URL.revokeObjectURL(preview);
  }, [preview]);
  if (isImage(file) && preview) {
    return <div className="image-chip image-chip--preview"><img src={preview} alt="" /></div>;
  }
  if (isAudio(file)) return <div className="audio-chip"><AudioLines size={16} /></div>;
  return <div className="file-chip"><FileText size={16} /></div>;
}

function RestoredFileThumb({ attachment, workspaceId, chatId }: {
  attachment: ChatAttachment;
  workspaceId?: string | null;
  chatId?: string | null;
}) {
  const src = attachment.isImage && workspaceId && chatId
    ? chatAttachmentUrl(workspaceId, chatId, attachment.id)
    : '';
  if (src) {
    return <div className="image-chip image-chip--preview"><img src={src} alt="" /></div>;
  }
  if (attachment.type?.startsWith('audio/')) return <div className="audio-chip"><AudioLines size={16} /></div>;
  if (attachment.isImage) return <div className="image-chip"><Image size={16} /></div>;
  return <div className="file-chip"><FileText size={16} /></div>;
}

function UsagePopover({ orbRef, open, onClose, onHoverStart, onHoverEnd, totalCost, totalInput, totalOutput, cacheRate, activeContextTokens, onOpenStats }: {
  orbRef: React.RefObject<HTMLButtonElement | null>;
  open: boolean;
  onClose: () => void;
  onHoverStart: () => void;
  onHoverEnd: () => void;
  totalCost: number;
  totalInput: number;
  totalOutput: number;
  cacheRate: number;
  activeContextTokens: number;
  onOpenStats: () => void;
}) {
  const panelRef = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState({ left: 0, top: 0, placement: 'above' as 'above' | 'below', visible: false });

  useLayoutEffect(() => {
    if (!open) return;
    const place = () => {
      const rect = orbRef.current?.getBoundingClientRect();
      if (!rect) return;
      const width = 190;
      const height = 210;
      const margin = 8;
      const left = Math.max(margin, Math.min(rect.right - width, window.innerWidth - width - margin));
      const aboveTop = rect.top - height - 10;
      const belowTop = rect.bottom + 10;
      const placement = aboveTop >= margin ? 'above' : 'below';
      const top = placement === 'above' ? Math.max(margin, aboveTop) : Math.min(belowTop, window.innerHeight - height - margin);
      setPos({ left, top, placement, visible: true });
    };
    place();
    window.addEventListener('resize', place);
    window.addEventListener('scroll', place, true);
    return () => {
      window.removeEventListener('resize', place);
      window.removeEventListener('scroll', place, true);
    };
  }, [open, orbRef]);

  useEffect(() => {
    if (!open) return;
    const onDown = (event: PointerEvent) => {
      const target = event.target as Node | null;
      if (panelRef.current?.contains(target)) return;
      if (orbRef.current?.contains(target)) return;
      onClose();
    };
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose(); };
    document.addEventListener('pointerdown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('pointerdown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, onClose, orbRef]);

  if (!open || typeof document === 'undefined') return null;
  return createPortal(
    <div
      ref={panelRef}
      className={`usage-popover-portal is-${pos.placement} ${pos.visible ? 'is-visible' : ''}`}
      style={{ left: pos.left, top: pos.top }}
      role="dialog"
      aria-label="Usage"
      onPointerEnter={onHoverStart}
      onPointerLeave={onHoverEnd}
    >
      <div className="usage-popover-title">Usage</div>
      <div className="usage-line"><span>Cost</span><strong>${totalCost.toFixed(5)}</strong></div>
      <div className="usage-line"><span>Tokens</span><strong>{(totalInput + totalOutput).toLocaleString()}</strong></div>
      <div className="usage-line"><span>Context</span><strong>{activeContextTokens.toLocaleString()}</strong></div>
      <div className="usage-line"><span>Cache</span><strong>{cacheRate.toFixed(1)}%</strong></div>
      <button type="button" onClick={() => { onClose(); onOpenStats(); }} className="usage-details">View details</button>
    </div>,
    document.body,
  );
}

export default function PromptDock({ onSend, onCancel, isRunning, isVoiceMode, onToggleVoice, voiceInputText, onOpenStats, onOpenAgentConfig, totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens, selectedModel, models = [], onModelChange, draft, draftAttachments, onDraftConsumed, onDraftAttachmentsConsumed, workspaceId, chatId }: Props) {
  const [value, setValue] = useState('');
  const [files, setFiles] = useState<File[]>([]);
  const [restored, setRestored] = useState<ChatAttachment[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);

  const [openMenu, setOpenMenu] = useState<'model' | null>(null);
  const modelMenuRef = useRef<HTMLDivElement>(null);
  const [providerInfo, setProviderInfo] = useState<ProviderInfo | null>(null);
  // Run target: where this chat's next prompt goes. Persisted per chat so a
  // refresh keeps Local vs Cloud. Cloud transfer itself lands next — this
  // toggle + the row badge already read the live runs/active + health data.
  const [runTarget, setRunTarget] = useState<'local' | 'cloud'>(() => {
    try {
      return window.localStorage.getItem(`stark.runTarget.${chatId ?? 'none'}`) === 'cloud' ? 'cloud' : 'local';
    } catch { return 'local'; }
  });
  useEffect(() => {
    try {
      window.localStorage.setItem(`stark.runTarget.${chatId ?? 'none'}`, runTarget);
    } catch { /* private mode */ }
  }, [runTarget, chatId]);
  useEffect(() => {
    try {
      setRunTarget(window.localStorage.getItem(`stark.runTarget.${chatId ?? 'none'}`) === 'cloud' ? 'cloud' : 'local');
    } catch { /* keep current */ }
  }, [chatId]);
  const [usageOpen, setUsageOpen] = useState(false);
  const [usagePinned, setUsagePinned] = useState(false);
  const usageOrbRef = useRef<HTMLButtonElement | null>(null);
  const usageCloseTimer = useRef<number | null>(null);
  const askBatch = useAskBatchForChat(chatId);

  const cancelUsageClose = useCallback(() => {
    if (usageCloseTimer.current) {
      window.clearTimeout(usageCloseTimer.current);
      usageCloseTimer.current = null;
    }
  }, []);
  const openUsage = useCallback((pin = false) => {
    cancelUsageClose();
    setUsageOpen(true);
    if (pin) setUsagePinned(true);
  }, [cancelUsageClose]);
  const closeUsage = useCallback(() => {
    cancelUsageClose();
    setUsageOpen(false);
    setUsagePinned(false);
  }, [cancelUsageClose]);
  const scheduleUsageClose = useCallback(() => {
    if (usagePinned) return;
    cancelUsageClose();
    usageCloseTimer.current = window.setTimeout(() => {
      // Moving from the orb into the portaled panel fires leave/enter in an
      // order that can resurrect this timer. Trust the actual hover state at
      // fire time, not event ordering.
      if (usageOrbRef.current?.matches(':hover') || document.querySelector('.usage-popover-portal:hover')) {
        usageCloseTimer.current = null;
        return;
      }
      setUsageOpen(false);
      usageCloseTimer.current = null;
    }, 260);
  }, [usagePinned, cancelUsageClose]);
  useEffect(() => () => cancelUsageClose(), [cancelUsageClose]);

  // The old label was hardcoded to "OpenCode Go" even when the server was on
  // another gateway. Read the live provider from the backend instead.
  useEffect(() => {
    let active = true;
    const load = () => {
      fetchProvider()
        .then((info) => { if (active) setProviderInfo(info); })
        .catch(() => {});
    };
    load();
    const timer = window.setInterval(load, 30_000);
    window.addEventListener('stark-provider-changed', load);
    return () => {
      active = false;
      window.clearInterval(timer);
      window.removeEventListener('stark-provider-changed', load);
    };
  }, []);

  const providerHealth = providerInfo?.health?.[providerInfo.provider] || '';
  const providerLabel = providerInfo
    ? (providerInfo.provider === 'opencode' ? 'OpenCode Go' : providerInfo.provider_label || providerInfo.provider)
    : 'Provider';
  const providerTitle = providerInfo
    ? `${providerInfo.provider_label || providerInfo.provider}\n${providerInfo.base_url}\nmodel: ${providerInfo.model_flash || '—'}${providerInfo.has_key ? '' : '\nAPI key missing'}${providerHealth ? `\nprovider error: ${providerHealth}` : ''}\nClick to configure agents and providers`
    : 'Loading provider…';

  // Close dropup on outside click / Escape
  useEffect(() => {
    if (!openMenu) return;
    const onPointerDown = (e: PointerEvent) => {
      const el = modelMenuRef.current;
      if (el && !el.contains(e.target as Node)) setOpenMenu(null);
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpenMenu(null); };
    document.addEventListener('pointerdown', onPointerDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [openMenu]);
  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
  }, [value]);

  useEffect(() => { if (isVoiceMode && voiceInputText !== undefined) setValue(voiceInputText); }, [voiceInputText, isVoiceMode]);
  useEffect(() => {
    setRestored(draftAttachments ?? []);
    if (!draft) return;
    setValue(draft);
    onDraftConsumed?.();
  }, [draft, draftAttachments, onDraftConsumed]);
  const totalAttachments = files.length + restored.length;
  const attachContext = useOptionalAttachmentDrop();
  const addFiles = useCallback((incoming: FileList | File[]) => {
    const selected = Array.from(incoming);
    const valid = selected.filter((file) => file.size <= MAX_FILE_SIZE);
    if (valid.length !== selected.length) setError('Files must be 20 MB or smaller.');
    else if (valid.length) setError(null);
    setFiles((current) => {
      const next = [...current, ...valid.filter((file) => !current.some((item) => item.name === file.name && item.size === file.size))].slice(0, MAX_FILES);
      if (next.length < current.length + valid.length) setError(`You can attach up to ${MAX_FILES} files at once.`);
      else if (next.length < MAX_FILES) setError((prev) => (prev && prev.includes('up to 8') ? null : prev) as any);
      return next;
    });
  }, []);
  useEffect(() => attachContext?.register(addFiles), [attachContext, addFiles]);
  useEffect(() => {
    if (totalAttachments < MAX_FILES) {
      setError((prev) => (prev === `You can attach up to ${MAX_FILES} files at once.` ? null : prev));
    }
  }, [totalAttachments]);

  const send = () => {
    const text = value.trim();
    if ((!text && !files.length && !restored.length) || isRunning) return;
    onSend(text, files, restored);
    setValue(''); setFiles([]); setRestored([]); setError(null);
    onDraftConsumed?.();
    onDraftAttachmentsConsumed?.();
  };
  const cacheRate = totalInput ? (totalCacheHitTokens / totalInput) * 100 : 0;
  const contextPercent = Math.min(100, (activeContextTokens / 1_000_000) * 100);

  const handlePaste = (event: React.ClipboardEvent) => {
    const items = event.clipboardData?.items;
    if (!items) return;
    const imageFiles: File[] = [];
    for (let i = 0; i < items.length; i++) {
      const item = items[i];
      if (item.kind === 'file' && item.type.startsWith('image/')) {
        const f = item.getAsFile();
        if (f) {
          // Preserve original name if available, fallback to clipboard image
          const named = new File([f], f.name || `pasted-${Date.now()}.${(f.type.split('/')[1] || 'png')}`, { type: f.type });
          imageFiles.push(named);
        }
      }
    }
    if (imageFiles.length) {
      event.preventDefault();
      addFiles(imageFiles);
    }
  };

  if (askBatch) {
    return (
      <div
        className="ask-prompt-slot w-full flex-shrink-0 px-1.5 md:px-2 pt-1.5 pb-1.5 md:pb-2"
        style={{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 6px)' }}
      >
        <AskQuestionPanel
          key={askBatch.batch_id}
          batch={askBatch}
          onClose={() => dismissAskBatch(askBatch.batch_id)}
        />
      </div>
    );
  }

  return (
    <div
      className="w-full flex-shrink-0 px-1.5 md:px-2 pt-1.5 pb-1.5 md:pb-2"
      style={{ paddingBottom: 'calc(env(safe-area-inset-bottom) + 6px)' }}
    >
      <div
        data-prompt-dock
        className={`prompt-dock premium-surface premium-surface--dark relative overflow-visible ${dragActive ? 'is-dragging' : ''} ${isVoiceMode ? 'is-listening' : ''}`}
        onDragEnter={(event) => { event.preventDefault(); dragDepth.current += 1; setDragActive(true); }}
        onDragLeave={(event) => { event.preventDefault(); dragDepth.current -= 1; if (dragDepth.current <= 0) { dragDepth.current = 0; setDragActive(false); } }}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => { event.preventDefault(); dragDepth.current = 0; setDragActive(false); addFiles(event.dataTransfer.files); }}
        onPaste={handlePaste}
      >
        <input ref={inputRef} type="file" multiple accept={ACCEPTED_TYPES} className="hidden" onChange={(event) => { if (event.target.files) addFiles(event.target.files); event.currentTarget.value = ''; }} />

        {totalAttachments > 0 && <div className="attachment-deck" aria-label="Files ready to send">
          {files.map((file) => <div className="attachment-card" key={`${file.name}-${file.size}`}>
            <LocalFileThumb file={file} />
            <div className="min-w-0 flex-1"><div className="truncate text-[12px] font-medium text-zinc-200">{file.name}</div><div className="text-[10px] text-zinc-500">{isImage(file) ? 'Image' : isAudio(file) ? 'Audio' : file.type.split('/').pop() || 'File'} · {formatSize(file.size)}</div></div>
            <button type="button" onClick={() => setFiles((all) => all.filter((item) => item !== file))} className="remove-file" title={`Remove ${file.name}`}><X size={13} /></button>
          </div>)}
          {restored.map((attachment) => <div className="attachment-card" key={`restored-${attachment.id}`}>
            <RestoredFileThumb attachment={attachment} workspaceId={workspaceId} chatId={chatId} />
            <div className="min-w-0 flex-1">
              <div className="truncate text-[12px] font-medium text-zinc-200">{attachment.name}</div>
              <div className="text-[10px] text-zinc-500">{attachment.isImage ? 'Image' : attachment.type?.startsWith('audio/') ? 'Audio' : attachment.type?.split('/').pop() || 'File'} · {formatSize(attachment.size)}</div>
            </div>
            <button type="button" onClick={() => setRestored((all) => all.filter((item) => item.id !== attachment.id))} className="remove-file" title={`Remove ${attachment.name}`}><X size={13} /></button>
          </div>)}
          <button type="button" className="attachment-add" onClick={() => inputRef.current?.click()} title="Add files"><Plus size={17} /></button>
        </div>}

        <div style={{ padding: '10px 12px 4px' }}>
          <textarea ref={textareaRef} value={value} onChange={(event) => setValue(event.target.value)} onPaste={handlePaste} onKeyDown={(event) => { if (event.key === 'Enter' && (event.shiftKey === false || (event.ctrlKey || event.metaKey))) { event.preventDefault(); send(); } }} rows={1}
            placeholder={isVoiceMode ? 'Listening…' : totalAttachments ? 'Add a note about these files…' : 'Ask anything'}
            className="prompt-input" />
        </div>
        {error && <div className="mx-4 mb-2 text-[11px] text-amber-300/90">{error}</div>}
        <div className="prompt-toolbar">
          <div className="flex items-center gap-1">
            <button type="button" onClick={() => inputRef.current?.click()} className="tool-button" title="Attach files"><Plus size={18} /></button>
            <div
              className="flex items-center rounded-lg border border-zinc-800 overflow-hidden"
              role="radiogroup"
              aria-label="Run target"
              title={runTarget === 'cloud' ? 'Run on Cloud (VPS) — preference saved per chat' : 'Run on Local — click to target Cloud (VPS)'}
            >
              <button
                type="button"
                role="radio"
                aria-checked={runTarget === 'local'}
                onClick={() => setRunTarget('local')}
                className={`px-2 py-1.5 text-[11px] font-medium transition-colors cursor-pointer ${runTarget === 'local' ? 'bg-zinc-800 text-zinc-100' : 'text-zinc-500 hover:text-zinc-300'}`}
              >
                Local
              </button>
              <button
                type="button"
                role="radio"
                aria-checked={runTarget === 'cloud'}
                onClick={() => setRunTarget('cloud')}
                className={`px-2 py-1.5 flex items-center gap-1 text-[11px] font-medium transition-colors cursor-pointer ${runTarget === 'cloud' ? 'bg-sky-500/20 text-sky-300 drop-shadow-[0_0_6px_rgba(125,211,252,0.5)]' : 'text-zinc-500 hover:text-zinc-300'}`}
              >
                {runTarget === 'cloud' ? <Cloud size={12} className="animate-pulse" /> : <CloudOff size={12} />}
                Cloud
              </button>
            </div>
          </div>
          <div className="flex items-center gap-2">
            <span className="hidden sm:block text-[10px] text-zinc-600">{totalAttachments ? `${totalAttachments} attachment${totalAttachments > 1 ? 's' : ''}` : ''}</span>
            <div className="usage-control">
              <button
                ref={usageOrbRef}
                type="button"
                className="usage-orb"
                title={`Context ${contextPercent.toFixed(0)}% of 1M`}
                aria-label={`Context ${contextPercent.toFixed(0)} percent`}
                aria-expanded={usageOpen}
                aria-haspopup="dialog"
                onClick={() => { if (usagePinned) closeUsage(); else openUsage(true); }}
                onMouseEnter={() => openUsage()}
                onMouseLeave={scheduleUsageClose}
                onFocus={() => openUsage()}
                onBlur={scheduleUsageClose}
              >
                <svg viewBox="0 0 40 40" className="usage-ring" aria-hidden="true"><circle className="usage-ring-track" cx="20" cy="20" r="16" pathLength="100"/><circle className="usage-ring-progress" cx="20" cy="20" r="16" pathLength="100" style={{ strokeDasharray: `${contextPercent} 100`, stroke: contextPercent >= 95 ? '#f87171' : contextPercent >= 85 ? '#fbbf24' : undefined }}/></svg>
                <span style={contextPercent >= 95 ? { color: '#f87171' } : contextPercent >= 85 ? { color: '#fbbf24' } : undefined}>{Math.round(contextPercent)}</span>
              </button>
              <UsagePopover
                orbRef={usageOrbRef}
                open={usageOpen}
                onClose={closeUsage}
                onHoverStart={() => openUsage()}
                onHoverEnd={scheduleUsageClose}
                totalCost={totalCost}
                totalInput={totalInput}
                totalOutput={totalOutput}
                cacheRate={cacheRate}
                activeContextTokens={activeContextTokens}
                onOpenStats={onOpenStats}
              />
            </div>
            <button
              type="button"
              className={`provider-trigger ${providerInfo && (!providerInfo.has_key || providerHealth) ? 'is-warning' : ''}`}
              title={providerTitle}
              aria-label={`LLM provider: ${providerLabel}. Configure agents`}
              onClick={() => onOpenAgentConfig?.()}
              style={providerInfo && (!providerInfo.has_key || providerHealth) ? { color: '#fbbf24' } : undefined}
            >
              <span className="truncate">{providerLabel}</span>
            </button>
            {models.length > 0 && onModelChange && (
              <div className="provider-dropup" ref={modelMenuRef}>
                <button
                  type="button"
                  onClick={() => setOpenMenu((m) => (m === 'model' ? null : 'model'))}
                  className="provider-trigger"
                  title="Model"
                  aria-label="Model"
                  aria-expanded={openMenu === 'model'}
                >
                  <span className="truncate">{models.find((m) => m.id === selectedModel)?.name || 'Automatic Fallback'}</span>
                  <ChevronDown size={13} className={`provider-chevron ${openMenu === 'model' ? 'is-open' : ''}`} />
                </button>
                {openMenu === 'model' && (
                  <div className="provider-menu" role="menu" aria-label="Model">
                    <button
                      type="button"
                      role="menuitemradio"
                      aria-checked={!selectedModel || selectedModel === 'Automatic Fallback'}
                      className={`provider-item ${(!selectedModel || selectedModel === 'Automatic Fallback') ? 'selected' : ''}`}
                      onClick={() => { onModelChange('Automatic Fallback'); setOpenMenu(null); }}
                    >
                      <span className="truncate">Automatic Fallback</span>
                      {(!selectedModel || selectedModel === 'Automatic Fallback') && <Check size={13} className="shrink-0" />}
                    </button>
                    {models.map((m) => {
                      const selected = m.id === selectedModel;
                      return (
                        <button
                          key={m.id}
                          type="button"
                          role="menuitemradio"
                          aria-checked={selected}
                          className={`provider-item ${selected ? 'selected' : ''}`}
                          onClick={() => { onModelChange(m.id); setOpenMenu(null); }}
                        >
                          <span className="truncate">{m.name}</span>
                          {selected && <Check size={13} className="shrink-0" />}
                        </button>
                      );
                    })}
                  </div>
                )}
              </div>
            )}
            <button type="button" onClick={onToggleVoice} className={`prompt-voice-button ${isVoiceMode ? 'is-live' : ''}`} title={isVoiceMode ? 'Stop voice mode (Ctrl+G)' : 'Start voice mode (Ctrl+G)'}><AudioLines size={16}/><span className="voice-live-dot" /></button>
            <button type="button" onClick={() => isRunning && !value.trim() && !totalAttachments ? onCancel?.() : send()} disabled={!isRunning && !value.trim() && !totalAttachments} className={`pressable send-button ${isRunning && !value.trim() && !totalAttachments ? 'stop' : ''}`} title={isRunning ? 'Stop execution' : 'Send message'}>{isRunning && !value.trim() && !totalAttachments ? <Square size={14} fill="currentColor" /> : <ArrowUp size={16} strokeWidth={2.4} />}</button>
          </div>
        </div>
      </div>
    </div>
  );
}
