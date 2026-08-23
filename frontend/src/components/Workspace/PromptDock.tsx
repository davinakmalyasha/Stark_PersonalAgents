import { useEffect, useRef, useState } from 'react';
import { ArrowUp, FileText, Image, Plus, Square, X, AudioLines } from 'lucide-react';
import { fetchProvider, setProvider } from '../../services/api';
import type { ModelOption } from '../../types';

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
}

const MAX_FILES = 8;
const MAX_FILE_SIZE = 20 * 1024 * 1024;
const formatSize = (bytes: number) => bytes < 1024 * 1024 ? `${Math.max(1, Math.round(bytes / 1024))} KB` : `${(bytes / 1024 / 1024).toFixed(1)} MB`;
const isImage = (file: File) => file.type.startsWith('image/');

export default function PromptDock({ onSend, onCancel, isRunning, isVoiceMode, onToggleVoice, voiceInputText, onOpenStats, totalCost, totalInput, totalOutput, totalCacheHitTokens, activeContextTokens, selectedModel, models = [], onModelChange }: Props) {
  const [value, setValue] = useState('');
  const [files, setFiles] = useState<File[]>([]);
  const [dragActive, setDragActive] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [provider, setProviderState] = useState<'deepseek' | 'opencode'>('deepseek');
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const dragDepth = useRef(0);

  // Load the active LLM provider once on mount.
  useEffect(() => {
    let mounted = true;
    fetchProvider().then((info) => { if (mounted) setProviderState(info.provider); }).catch(() => {});
    return () => { mounted = false; };
  }, []);

  const changeProvider = async (value: 'deepseek' | 'opencode') => {
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
  const addFiles = (incoming: FileList | File[]) => {
    const selected = Array.from(incoming);
    const valid = selected.filter((file) => file.size <= MAX_FILE_SIZE);
    if (valid.length !== selected.length) setError('Files must be 20 MB or smaller.');
    setFiles((current) => {
      const next = [...current, ...valid.filter((file) => !current.some((item) => item.name === file.name && item.size === file.size))].slice(0, MAX_FILES);
      if (next.length < current.length + valid.length) setError(`You can attach up to ${MAX_FILES} files at once.`);
      return next;
    });
  };

  const send = () => {
    const text = value.trim();
    if (!text && !files.length) return;
    onSend(text, files);
    setValue(''); setFiles([]); setError(null);
  };
  const cacheRate = totalInput ? (totalCacheHitTokens / totalInput) * 100 : 0;
  const contextPercent = Math.min(100, (activeContextTokens / 1_000_000) * 100);

  return (
    <div className="w-full flex-shrink-0 px-4 pt-3 pb-4">
      <div
        className={`prompt-dock premium-surface relative overflow-visible ${dragActive ? 'is-dragging' : ''} ${isVoiceMode ? 'is-listening' : ''}`}
        onDragEnter={(event) => { event.preventDefault(); dragDepth.current += 1; setDragActive(true); }}
        onDragLeave={(event) => { event.preventDefault(); dragDepth.current -= 1; if (dragDepth.current <= 0) { dragDepth.current = 0; setDragActive(false); } }}
        onDragOver={(event) => event.preventDefault()}
        onDrop={(event) => { event.preventDefault(); dragDepth.current = 0; setDragActive(false); addFiles(event.dataTransfer.files); }}
      >
        <input ref={inputRef} type="file" multiple className="hidden" onChange={(event) => { if (event.target.files) addFiles(event.target.files); event.currentTarget.value = ''; }} />
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
          <textarea ref={textareaRef} value={value} onChange={(event) => setValue(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter' && (event.shiftKey === false || (event.ctrlKey || event.metaKey))) { event.preventDefault(); send(); } }} rows={1}
            placeholder={isVoiceMode ? 'Listening…' : files.length ? 'Add a note about these files…' : 'Ask anything'}
            className="prompt-input" />
        </div>
        {error && <div className="mx-4 mb-2 text-[11px] text-amber-300/90">{error}</div>}
        <div className="prompt-toolbar">
          <div className="flex items-center gap-1">
            <button type="button" onClick={() => inputRef.current?.click()} className="tool-button" title="Attach files"><Plus size={18} /></button>
            <select
              value={provider}
              onChange={(e) => changeProvider(e.target.value as 'deepseek' | 'opencode')}
              className="provider-select"
              title={provider === 'opencode' ? 'LLM provider: OpenCode Go' : 'LLM provider: DeepSeek API'}
              aria-label="LLM provider"
            >
              <option value="deepseek">DeepSeek API</option>
              <option value="opencode">OpenCode Go</option>
            </select>
            {models.length > 0 && onModelChange && (
              <select
                value={selectedModel || 'Automatic Fallback'}
                onChange={(e) => onModelChange(e.target.value)}
                className="provider-select"
                title="Model"
                aria-label="Model"
              >
                <option value="Automatic Fallback">Automatic Fallback</option>
                {models.map((m) => (
                  <option key={m.id} value={m.id}>{m.name}</option>
                ))}
              </select>
            )}
          </div>
          <div className="flex items-center gap-2"><span className="hidden sm:block text-[10px] text-zinc-600">{files.length ? `${files.length} attachment${files.length > 1 ? 's' : ''}` : ''}</span>
            <div className="usage-control"><button type="button" className="usage-orb" title="Usage" aria-label="Usage"><svg viewBox="0 0 40 40" className="usage-ring" aria-hidden="true"><circle className="usage-ring-track" cx="20" cy="20" r="16" pathLength="100"/><circle className="usage-ring-progress" cx="20" cy="20" r="16" pathLength="100" style={{ strokeDasharray: `${contextPercent} 100` }}/></svg><span>{Math.round(cacheRate)}</span></button><div className="usage-popover"><div className="usage-popover-title">Usage</div><div className="usage-line"><span>Cost</span><strong>${totalCost.toFixed(5)}</strong></div><div className="usage-line"><span>Tokens</span><strong>{(totalInput + totalOutput).toLocaleString()}</strong></div><div className="usage-line"><span>Context</span><strong>{activeContextTokens.toLocaleString()}</strong></div><div className="usage-line"><span>Cache</span><strong>{cacheRate.toFixed(1)}%</strong></div><button type="button" onClick={onOpenStats} className="usage-details">View details</button></div></div>
            <button type="button" onClick={onToggleVoice} className={`prompt-voice-button ${isVoiceMode ? 'is-live' : ''}`} title={isVoiceMode ? 'Stop voice mode (Ctrl+G)' : 'Start voice mode (Ctrl+G)'}><AudioLines size={16}/><span className="voice-live-dot" /></button>
            <button type="button" onClick={() => isRunning && !value.trim() && !files.length ? onCancel?.() : send()} disabled={!isRunning && !value.trim() && !files.length} className={`pressable send-button ${isRunning && !value.trim() && !files.length ? 'stop' : ''}`} title={isRunning ? 'Stop execution' : 'Send message'}>{isRunning && !value.trim() && !files.length ? <Square size={14} fill="currentColor" /> : <ArrowUp size={16} strokeWidth={2.4} />}</button>
          </div>
        </div>
      </div>
    </div>
  );
}
