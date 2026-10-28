import { useEffect, useRef, useState } from 'react';
import { ArrowUp, Check, ChevronDown, FileText, Image, Plus, Square, X, AudioLines } from 'lucide-react';
import { fetchProvider, setProvider } from '../../services/api';
import type { ProviderMode } from '../../services/api';
import type { ModelOption } from '../../types';

const PROVIDER_LABELS: Record<ProviderMode, string> = {
  free: 'Free Providers',
  deepseek: 'DeepSeek API',
  opencode: 'OpenCode Go',
  command: 'Command Code',
  openrouter: 'OpenRouter',
};

interface Props {
  onSend: (message: string, files: File[]) => void;
  onCancel?: () => void;
  isRunning: boolean;
  isVoiceMode?: boolean;
  onToggleVoice: () => void;
  voiceInputText?: string;
  onOpenStats: () => void;
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
  onDraftConsumed?: () => void;
}

const MAX_FILES = 8;
const MAX_FILE_SIZE = 20 * 1024 * 1024;
const formatSize = (bytes: number) => bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
const isImage = (file: File) => file.type.startsWith('image/');

export default function PromptDock({ onSend, onCancel, isRunning, isVoiceMode, onToggleVoice, voiceInputText, onOpenStats, totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens, selectedModel, models = [], onModelChange, draft, onDraftConsumed }: Props) {
  const [value, setValue] = useState('');
  const [files, setFiles] = useState<File[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [provider, setProviderState] = useState<ProviderMode>('deepseek');
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);

  const [availableProviders, setAvailableProviders] = useState<ProviderMode[] | null>(null);
  const [openMenu, setOpenMenu] = useState<'provider' | 'model' | null>(null);
  const providerMenuRef = useRef<HTMLDivElement>(null);
  const modelMenuRef = useRef<HTMLDivElement>(null);

  // Close dropup on outside click / Escape
  useEffect(() => {
    if (!openMenu) return;
    const onPointerDown = (e: PointerEvent) => {
      const el = openMenu === 'provider' ? providerMenuRef.current : modelMenuRef.current;
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
  // Load the active LLM provider once on mount — derive options from server matrix, not hardcoded
  useEffect(() => {
    let mounted = true;
    fetchProvider().then((info) => {
      if (!mounted) return;
      setProviderState(info.provider);
      const modes = (info as any).modes as ProviderMode[] | undefined;
      const matrix = (info as any).matrix as Record<string, unknown> | undefined;
      const derived = modes && modes.length ? modes : matrix ? (Object.keys(matrix) as ProviderMode[]) : null;
      if (derived && derived.length) setAvailableProviders(derived);
    }).catch(() => {});
    return () => { mounted = false; };
  }, []);

  const changeProvider = async (value: ProviderMode) => {
    setProviderState(value);
    try { await setProvider(value); } catch (e) { console.error('Failed to set LLM provider', e); }
  };

  useEffect(() => {
    const el = textareaRef.current;
    if (!el) return;
    el.style.height = 'auto';
    el.style.height = `${Math.min(el.scrollHeight, 180)}px`;
  }, [value]);

  useEffect(() => { if (isVoiceMode && voiceInputText !== undefined) setValue(voiceInputText); }, [voiceInputText, isVoiceMode]);
  useEffect(() => {
    if (!draft) return;
    setValue(draft);
    onDraftConsumed?.();
  }, [draft, onDraftConsumed]);
  const addFiles = (incoming: FileList | File[]) => {
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
  };
  useEffect(() => {
    if (files.length < MAX_FILES) {
      setError((prev) => (prev === `You can attach up to ${MAX_FILES} files at once.` ? null : prev));
    }
  }, [files.length]);

  const send = () => {
    const text = value.trim();
    if (!text && !files.length) return;
    onSend(text, files);
    setValue(''); setFiles([]); setError(null);
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

  return (
    <div className="w-full flex-shrink-0 px-4 pt-3 pb-4">
      <div
        className={`prompt-dock premium-surface premium-surface--dark relative overflow-visible ${dragActive ? 'is-dragging' : ''} ${isVoiceMode ? 'is-listening' : ''}`}
        onDragEnter={(event) => { event.preventDefault(); dragDepth.current += 1; setDragActive(true); }}
        onDragLeave={(event) => { event.preventDefault(); dragDepth.current -= 1; if (dragDepth.current <= 0) { dragDepth.current = 0; setDragActive(false); } }}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => { event.preventDefault(); dragDepth.current = 0; setDragActive(false); addFiles(event.dataTransfer.files); }}
        onPaste={handlePaste}
      >
        <input ref={inputRef} type="file" multiple accept="image/*,.pdf,.txt,.md,.json,.js,.ts,.tsx,.py,.html,.css" className="hidden" onChange={(event) => { if (event.target.files) addFiles(event.target.files); event.currentTarget.value = ''; }} />
        {dragActive && <div className="drop-capture" aria-hidden="true"><div className="drop-orbit"><Plus size={24} /></div><strong>Drop to add to your message</strong><span>Images, documents, and code files up to 20 MB</span></div>}

        {files.length > 0 && <div className="attachment-deck" aria-label="Files ready to send">
          {files.map((file) => <div className="attachment-card" key={`${file.name}-${file.size}`}>
            {isImage(file) ? <div className="image-chip"><Image size={16} /></div> : <div className="file-chip"><FileText size={16} /></div>}
            <div className="min-w-0 flex-1"><div className="truncate text-[12px] font-medium text-zinc-200">{file.name}</div><div className="text-[10px] text-zinc-500">{isImage(file) ? 'Image' : file.type.split('/').pop() || 'File'} · {formatSize(file.size)}</div></div>
            <button type="button" onClick={() => setFiles((all) => all.filter((item) => item !== file))} className="remove-file" title={`Remove ${file.name}`}><X size={13} /></button>
          </div>)}
          <button type="button" className="attachment-add" onClick={() => inputRef.current?.click()} title="Add files"><Plus size={17} /></button>
        </div>}

        <div style={{ padding: '14px 18px 5px' }}>
          <textarea ref={textareaRef} value={value} onChange={(event) => setValue(event.target.value)} onPaste={handlePaste} onKeyDown={(event) => { if (event.key === 'Enter' && (event.shiftKey === false || (event.ctrlKey || event.metaKey))) { event.preventDefault(); send(); } }} rows={1}
            placeholder={isVoiceMode ? 'Listening…' : files.length ? 'Add a note about these files…' : 'Ask anything'}
            className="prompt-input" />
        </div>
        {error && <div className="mx-4 mb-2 text-[11px] text-amber-300/90">{error}</div>}
        <div className="prompt-toolbar">
          <div className="flex items-center gap-1">
            <button type="button" onClick={() => inputRef.current?.click()} className="tool-button" title="Attach files"><Plus size={18} /></button>
          </div>
          <div className="flex items-center gap-2">
            <span className="hidden sm:block text-[10px] text-zinc-600">{files.length ? `${files.length} attachment${files.length > 1 ? 's' : ''}` : ''}</span>
            <div className="usage-control"><button type="button" className="usage-orb" title="Usage" aria-label="Usage"><svg viewBox="0 0 40 40" className="usage-ring" aria-hidden="true"><circle className="usage-ring-track" cx="20" cy="20" r="16" pathLength="100"/><circle className="usage-ring-progress" cx="20" cy="20" r="16" pathLength="100" style={{ strokeDasharray: `${contextPercent} 100` }}/></svg><span>{Math.round(cacheRate)}</span></button><div className="usage-popover"><div className="usage-popover-title">Usage</div><div className="usage-line"><span>Cost</span><strong>${totalCost.toFixed(5)}</strong></div><div className="usage-line"><span>Tokens</span><strong>{(totalInput + totalOutput).toLocaleString()}</strong></div><div className="usage-line"><span>Context</span><strong>{activeContextTokens.toLocaleString()}</strong></div><div className="usage-line"><span>Cache</span><strong>{cacheRate.toFixed(1)}%</strong></div><button type="button" onClick={onOpenStats} className="usage-details">View details</button></div></div>
            <div className="provider-dropup" ref={providerMenuRef}>
              <button
                type="button"
                onClick={() => setOpenMenu((m) => (m === 'provider' ? null : 'provider'))}
                className="provider-trigger"
                title={`LLM provider: ${PROVIDER_LABELS[provider] ?? provider}`}
                aria-label="LLM provider"
                aria-expanded={openMenu === 'provider'}
              >
                <span className="truncate">{PROVIDER_LABELS[provider] ?? provider}</span>
                <ChevronDown size={13} className={`provider-chevron ${openMenu === 'provider' ? 'is-open' : ''}`} />
              </button>
              {openMenu === 'provider' && (
                <div className="provider-menu" role="menu" aria-label="LLM provider">
                  {(availableProviders || (['free','deepseek','opencode','command'] as ProviderMode[])).map((p) => {
                    const selected = p === provider;
                    return (
                      <button
                        key={p}
                        type="button"
                        role="menuitemradio"
                        aria-checked={selected}
                        className={`provider-item ${selected ? 'selected' : ''}`}
                        onClick={() => { changeProvider(p); setOpenMenu(null); }}
                      >
                        <span className="truncate">{PROVIDER_LABELS[p] ?? p}</span>
                        {selected && <Check size={13} className="shrink-0" />}
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
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
            <button type="button" onClick={() => isRunning && !value.trim() && !files.length ? onCancel?.() : send()} disabled={!isRunning && !value.trim() && !files.length} className={`pressable send-button ${isRunning && !value.trim() && !files.length ? 'stop' : ''}`} title={isRunning ? 'Stop execution' : 'Send message'}>{isRunning && !value.trim() && !files.length ? <Square size={14} fill="currentColor" /> : <ArrowUp size={16} strokeWidth={2.4} />}</button>
          </div>
        </div>
      </div>
    </div>
  );
}
