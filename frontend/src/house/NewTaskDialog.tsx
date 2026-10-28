import { useEffect, useState } from 'react';
import { CalendarClock, Check, FolderKanban, MessageSquare, X } from 'lucide-react';

export type NewTaskKind = 'single' | 'group' | 'scheduled';

export interface NewTaskRequest {
  kind: NewTaskKind;
  instructions: string;
  runAt?: string;
  repeat?: 'once' | 'daily' | 'weekly' | 'monthly';
}

interface Props {
  onClose: () => void;
  onCreate: (request: NewTaskRequest) => Promise<void>;
}

function localInputValue(date: Date): string {
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60_000);
  return local.toISOString().slice(0, 16);
}

const choices: { id: NewTaskKind; title: string; description: string; icon: typeof MessageSquare }[] = [
  { id: 'single', title: 'Single', description: 'Start one chat with a ready to send prompt.', icon: MessageSquare },
  { id: 'group', title: 'Group', description: 'Create a durable project with a shared board.', icon: FolderKanban },
  { id: 'scheduled', title: 'Scheduled', description: 'Run this prompt later or on a repeat schedule.', icon: CalendarClock },
];

export default function NewTaskDialog({ onClose, onCreate }: Props) {
  const [kind, setKind] = useState<NewTaskKind>('single');
  const [instructions, setInstructions] = useState('');
  const [runAt, setRunAt] = useState(() => localInputValue(new Date(Date.now() + 60 * 60_000)));
  const [repeat, setRepeat] = useState<NewTaskRequest['repeat']>('once');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => { if (event.key === 'Escape' && !busy) onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [busy, onClose]);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    const prompt = instructions.trim();
    if (!prompt) { setError('Add instructions to continue.'); return; }
    if (kind === 'scheduled' && !runAt) { setError('Choose a date and time.'); return; }
    setBusy(true);
    setError('');
    try {
      await onCreate({ kind, instructions: prompt, ...(kind === 'scheduled' ? { runAt, repeat } : {}) });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : 'Could not create the task.');
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 z-[120] grid place-items-center bg-black/65 p-4 backdrop-blur-sm" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}>
      <form
        role="dialog"
        aria-modal="true"
        aria-labelledby="new-task-title"
        onSubmit={submit}
        className="w-full max-w-[560px] overflow-hidden rounded-2xl border border-white/[0.1] bg-[#131317] shadow-[0_28px_100px_rgba(0,0,0,.7)]"
      >
        <header className="flex items-center justify-between border-b border-white/[0.07] px-5 py-4">
          <div>
            <h2 id="new-task-title" className="text-[15px] font-semibold text-zinc-100">New task</h2>
            <p className="mt-1 text-[11px] text-zinc-500">Choose how Stark should work on it.</p>
          </div>
          <button type="button" onClick={onClose} disabled={busy} aria-label="Close new task dialog" className="grid h-8 w-8 place-items-center rounded-lg text-zinc-500 hover:bg-white/[0.07] hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70 disabled:opacity-40">
            <X size={16} />
          </button>
        </header>

        <div className="grid grid-cols-3 gap-2 p-4 pb-2">
          {choices.map(({ id, title, description, icon: Icon }) => (
            <button
              key={id}
              type="button"
              aria-pressed={kind === id}
              onClick={() => { setKind(id); setError(''); }}
              className={`relative min-h-[92px] rounded-xl border p-3 text-left transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70 ${kind === id ? 'border-white/30 bg-white/[0.08]' : 'border-white/[0.07] bg-white/[0.02] hover:bg-white/[0.05]'}`}
            >
              <span className="flex items-center gap-2 text-[11px] font-semibold text-zinc-200"><Icon size={14} />{title}</span>
              <span className="mt-2 block text-[10px] leading-4 text-zinc-500">{description}</span>
              {kind === id && <Check size={13} className="absolute right-2 top-2 text-zinc-300" />}
            </button>
          ))}
        </div>

        <div className="px-5 pb-4 pt-3">
          <label htmlFor="new-task-instructions" className="mb-2 block text-[11px] font-medium text-zinc-300">Instructions</label>
          <textarea
            id="new-task-instructions"
            autoFocus
            value={instructions}
            onChange={(event) => { setInstructions(event.target.value); setError(''); }}
            rows={5}
            placeholder={kind === 'group' ? 'Describe the project and the outcome you want…' : 'What should Stark do?'}
            className="w-full resize-y rounded-xl border border-white/[0.09] bg-[#0d0d10] px-3.5 py-3 text-[12px] leading-5 text-zinc-200 outline-none placeholder:text-zinc-600 focus:border-white/25 focus:ring-2 focus:ring-white/10"
          />

          {kind === 'scheduled' && (
            <div className="mt-3 grid grid-cols-2 gap-3">
              <label className="text-[11px] font-medium text-zinc-300">
                Time <span className="mt-1.5 block"><input type="datetime-local" value={runAt} onChange={(event) => setRunAt(event.target.value)} className="h-9 w-full rounded-lg border border-white/[0.09] bg-[#0d0d10] px-2.5 text-[11px] text-zinc-200 outline-none focus:border-white/25 focus:ring-2 focus:ring-white/10" /></span>
              </label>
              <label className="text-[11px] font-medium text-zinc-300">
                Repeat <span className="mt-1.5 block"><select value={repeat} onChange={(event) => setRepeat(event.target.value as NewTaskRequest['repeat'])} className="h-9 w-full rounded-lg border border-white/[0.09] bg-[#0d0d10] px-2.5 text-[11px] text-zinc-200 outline-none focus:border-white/25 focus:ring-2 focus:ring-white/10">
                  <option value="once">Once</option><option value="daily">Daily</option><option value="weekly">Weekly</option><option value="monthly">Monthly</option>
                </select></span>
              </label>
              <p className="col-span-2 text-[10px] text-zinc-600">Time uses this device’s local timezone.</p>
            </div>
          )}

          {error && <p role="alert" className="mt-3 rounded-lg border border-rose-400/20 bg-rose-400/[0.07] px-3 py-2 text-[11px] text-rose-200">{error}</p>}
        </div>

        <footer className="flex justify-end gap-2 border-t border-white/[0.07] px-5 py-3.5">
          <button type="button" onClick={onClose} disabled={busy} className="h-9 rounded-lg px-3 text-[11px] font-medium text-zinc-400 hover:bg-white/[0.06] hover:text-white focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70 disabled:opacity-40">Cancel</button>
          <button type="submit" disabled={busy || !instructions.trim()} className="h-9 rounded-lg bg-white px-4 text-[11px] font-semibold text-[#0a0a0d] hover:bg-zinc-200 focus-visible:outline focus-visible:outline-2 focus-visible:outline-white/70 disabled:cursor-not-allowed disabled:opacity-45">
            {busy ? 'Creating…' : kind === 'single' ? 'Create chat' : kind === 'group' ? 'Create project' : 'Schedule task'}
          </button>
        </footer>
      </form>
    </div>
  );
}
