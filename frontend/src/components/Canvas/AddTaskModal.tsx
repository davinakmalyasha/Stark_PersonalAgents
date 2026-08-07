import { useRef, useState } from 'react';
import { CalendarClock, Clock3, Paperclip, Play, Save, X } from 'lucide-react';
import { createTask, updateTask, uploadChatAttachments } from '../../services/api';
import type { AgentTask, ChatAttachment, TaskRecurrence } from '../../types';

interface Props {
  workspaceId: string | null;
  chatId: string | null;
  isRunning: boolean;
  onClose: () => void;
  onCreated: (task: AgentTask) => void;
  task?: AgentTask | null;
  onUpdated?: (task: AgentTask) => void;
  onStartNow: (prompt: string, files: File[]) => Promise<void> | void;
}

const MAX_FILES = 8;
const MAX_FILE_SIZE = 20 * 1024 * 1024;

const toLocalDateTime = (value?: string) => {
  if (!value) return '';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return '';
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60000);
  return local.toISOString().slice(0, 16);
};

export default function AddTaskModal({ workspaceId, chatId, isRunning, onClose, onCreated, task, onUpdated, onStartNow }: Props) {
  const isEditing = Boolean(task);
  const [prompt, setPrompt] = useState(task?.prompt ?? '');
  const [files, setFiles] = useState<File[]>([]);
  const [mode, setMode] = useState<'immediate' | 'saved' | 'scheduled'>(task?.mode === 'scheduled' ? 'scheduled' : task ? 'saved' : 'immediate');
  const [scheduledAt, setScheduledAt] = useState(toLocalDateTime(task?.schedule?.nextRunAt));
  const [recurrence, setRecurrence] = useState<TaskRecurrence>(task?.schedule?.recurrence ?? 'none');
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  const addFiles = (incoming: FileList | null) => {
    if (!incoming) return;
    const selected = Array.from(incoming);
    const tooLarge = selected.some((file) => file.size > MAX_FILE_SIZE);
    if (tooLarge) setError('Files must be 20 MB or smaller.');
    setFiles((current) => [...current, ...selected.filter((file) => file.size <= MAX_FILE_SIZE && !current.some((item) => item.name === file.name && item.size === file.size))].slice(0, MAX_FILES));
  };

  const submit = async () => {
    const trimmed = prompt.trim();
    if (!trimmed && !files.length) return setError('Add a prompt or attachment first.');
    if (mode === 'immediate') {
      if (isRunning) return setError('Wait for the current run to finish before starting another task.');
      setSaving(true);
      try { await onStartNow(trimmed, files); onClose(); } catch (err) { setError(err instanceof Error ? err.message : 'Unable to start task.'); } finally { setSaving(false); }
      return;
    }
    if (!workspaceId || !chatId) return setError('Open a chat before saving a task.');
    let nextRunAt: string | undefined;
    if (mode === 'scheduled') {
      if (!scheduledAt) return setError('Choose when this task should run.');
      const date = new Date(scheduledAt);
      if (Number.isNaN(date.getTime()) || date.getTime() <= Date.now()) return setError('Scheduled time must be in the future.');
      nextRunAt = date.toISOString();
    }
    setSaving(true);
    try {
      let attachments: ChatAttachment[] = task?.attachments ?? [];
      if (files.length) attachments = await uploadChatAttachments(workspaceId, chatId, files);
      if (isEditing && task) {
        const updated = await updateTask(workspaceId, chatId, task.id, { prompt: trimmed, attachments, mode: mode === 'scheduled' ? 'scheduled' : 'saved', schedule: mode === 'scheduled' ? { nextRunAt, recurrence } : undefined });
        onUpdated?.(updated);
      } else {
        const created = await createTask(workspaceId, chatId, { prompt: trimmed, attachments, mode, schedule: mode === 'scheduled' ? { nextRunAt, recurrence } : undefined });
        onCreated(created);
      }
      onClose();
    } catch (err) { setError(err instanceof Error ? err.message : 'Unable to save task.'); } finally { setSaving(false); }
  };

  return <div className="task-modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="task-modal" role="dialog" aria-modal="true" aria-labelledby="add-task-title">
      <div className="task-modal-header"><div><span className="task-modal-eyebrow">TASK CONTROL</span><h2 id="add-task-title">{isEditing ? 'Edit task' : 'Add a task'}</h2></div><button type="button" className="task-modal-close" onClick={onClose} aria-label="Close"><X size={17} /></button></div>
      <label className="task-field-label" htmlFor="task-prompt">Prompt</label>
      <textarea id="task-prompt" className="task-prompt-input" value={prompt} onChange={(event) => setPrompt(event.target.value)} placeholder="Describe what the agent should do…" autoFocus />
      <div className="task-attachment-row"><button type="button" className="task-attach-button" onClick={() => inputRef.current?.click()}><Paperclip size={14} /> Add image or file</button><input ref={inputRef} type="file" multiple className="hidden" onChange={(event) => { addFiles(event.target.files); event.currentTarget.value = ''; }} /><span>{files.length ? `${files.length} attached` : 'Up to 20 MB each'}</span></div>
      {files.length > 0 && <div className="task-file-list">{files.map((file) => <div key={`${file.name}-${file.size}`}><span>{file.name}</span><button type="button" onClick={() => setFiles((current) => current.filter((item) => item !== file))}><X size={12} /></button></div>)}</div>}
      <div className="task-field-label">Run mode</div>
      <div className="task-mode-grid">{([['immediate', 'Start immediately', 'Run this task now.', Play], ['saved', 'Save for later', 'Start it manually when ready.', Save], ['scheduled', 'Schedule', 'Run automatically when due.', CalendarClock]] as const).filter(([value]) => !isEditing || value !== 'immediate').map(([value, label, detail, Icon]) => <button key={value} type="button" className={`task-mode-option ${mode === value ? 'is-selected' : ''}`} onClick={() => setMode(value)}><Icon size={15} /><span><b>{label}</b><small>{detail}</small></span></button>)}</div>
      {mode === 'scheduled' && <div className="task-schedule-fields"><label><span>First run</span><input type="datetime-local" value={scheduledAt} onChange={(event) => setScheduledAt(event.target.value)} /></label><label><span>Repeat</span><select value={recurrence} onChange={(event) => setRecurrence(event.target.value as TaskRecurrence)}><option value="none">Once</option><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="monthly">Monthly</option></select></label>{scheduledAt && <div className="task-schedule-summary"><Clock3 size={13} /> Runs {new Date(scheduledAt).toLocaleString()} {recurrence !== 'none' ? `· repeats ${recurrence}` : ''}</div>}</div>}
      {error && <div className="task-form-error">{error}</div>}
      <div className="task-modal-footer"><button type="button" className="task-cancel-button" onClick={onClose}>Cancel</button><button type="button" className="task-submit-button" onClick={submit} disabled={saving}>{saving ? 'Saving…' : isEditing ? 'Save changes' : mode === 'immediate' ? 'Start task' : mode === 'scheduled' ? 'Schedule task' : 'Save task'}</button></div>
    </div>
  </div>;
}
