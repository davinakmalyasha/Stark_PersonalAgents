import { useEffect, useState } from 'react';
import { Check, Plus, StickyNote, Trash2, X } from 'lucide-react';
import type { AgentNote } from '../../types';
import { createNote, deleteNote, fetchNotes, updateNote } from '../../services/api';

interface Props {
  scopeType: 'chat' | 'group';
  scopeId: string;
  className?: string;
}

export default function NotesButton({ scopeType, scopeId, className = '' }: Props) {
  const [open, setOpen] = useState(false);
  const [notes, setNotes] = useState<AgentNote[]>([]);
  const [error, setError] = useState('');
  const [adding, setAdding] = useState(false);
  const [title, setTitle] = useState('');
  const [body, setBody] = useState('');
  const [todos, setTodos] = useState('');

  const refresh = () => {
    if (!scopeId) return;
    fetchNotes(scopeType, scopeId)
      .then((next) => { setNotes(next); setError(''); })
      .catch((err) => setError(err instanceof Error ? err.message : 'Could not load notes.'));
  };

  useEffect(() => {
    if (!scopeId) return;
    let cancelled = false;
    fetchNotes(scopeType, scopeId)
      .then((next) => { if (!cancelled) { setNotes(next); setError(''); } })
      .catch((err) => { if (!cancelled) setError(err instanceof Error ? err.message : 'Could not load notes.'); });
    return () => { cancelled = true; };
  }, [open, scopeType, scopeId]);

  const submit = async () => {
    if (!title.trim() && !body.trim() && !todos.trim()) return;
    try {
      await createNote({
        scope_type: scopeType,
        scope_id: scopeId,
        title: title.trim() || 'Note',
        body: body.trim(),
        todos: todos.split('\n').map((item) => item.trim()).filter(Boolean),
      });
      setTitle('');
      setBody('');
      setTodos('');
      setAdding(false);
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not create note.');
    }
  };

  const toggleTodo = async (note: AgentNote, index: number) => {
    const next = note.todos.map((item, i) => i === index ? { ...item, done: !item.done } : item);
    try {
      await updateNote(note.id, { todos: next });
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not update note.');
    }
  };

  const remove = async (note: AgentNote) => {
    if (!window.confirm(`Delete note "${note.title}"?`)) return;
    try {
      await deleteNote(note.id);
      refresh();
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Could not delete note.');
    }
  };

  return (
    <>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className={`relative grid h-8 w-8 place-items-center rounded-xl border border-zinc-800/80 bg-zinc-900/95 text-zinc-400 transition-all hover:border-zinc-700/80 hover:bg-zinc-800 hover:text-zinc-100 ${className}`}
        title="Sticky notes"
        aria-label="Open sticky notes"
      >
        <StickyNote size={14} />
        {notes.length > 0 && (
          <span className="absolute -right-1 -top-1 grid h-[14px] min-w-[14px] place-items-center rounded-full bg-white px-0.5 text-[9px] font-bold text-[#0a0a0d]">
            {notes.length}
          </span>
        )}
      </button>

      {open && (
        <div
          className="fixed inset-0 z-[240] grid place-items-center bg-black/60 p-4 backdrop-blur-sm"
          onMouseDown={(event) => { if (event.target === event.currentTarget) setOpen(false); }}
        >
          <div className="flex max-h-[80vh] w-full max-w-lg flex-col overflow-hidden rounded-2xl border border-zinc-800 bg-[#141417] shadow-2xl">
            <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-3">
              <StickyNote size={14} className="text-amber-300" />
              <span className="flex-1 text-[12px] font-semibold uppercase tracking-widest text-zinc-400">Notes</span>
              <button
                type="button"
                onClick={() => setAdding((value) => !value)}
                className="flex h-7 items-center gap-1 rounded-lg bg-white px-2.5 text-[11px] font-semibold text-[#0a0a0d] hover:bg-zinc-200"
              >
                <Plus size={12} /> Add
              </button>
              <button type="button" onClick={() => setOpen(false)} className="grid h-7 w-7 place-items-center rounded-lg text-zinc-500 hover:bg-white/[0.07] hover:text-white" aria-label="Close">
                <X size={14} />
              </button>
            </div>

            <div className="flex-1 space-y-3 overflow-y-auto p-4">
              {adding && (
                <div className="space-y-2 rounded-xl border border-amber-400/20 bg-amber-400/[0.04] p-3">
                  <input
                    value={title}
                    onChange={(event) => setTitle(event.target.value)}
                    placeholder="Title"
                    className="w-full rounded-lg border border-zinc-700/70 bg-zinc-900 px-2.5 py-1.5 text-[12px] text-zinc-100 placeholder:text-zinc-600 focus:border-amber-300/40 focus:outline-none"
                  />
                  <textarea
                    value={body}
                    onChange={(event) => setBody(event.target.value)}
                    placeholder="Note"
                    rows={2}
                    className="w-full resize-none rounded-lg border border-zinc-700/70 bg-zinc-900 px-2.5 py-1.5 text-[12px] text-zinc-100 placeholder:text-zinc-600 focus:border-amber-300/40 focus:outline-none"
                  />
                  <textarea
                    value={todos}
                    onChange={(event) => setTodos(event.target.value)}
                    placeholder="Checklist, one item per line"
                    rows={3}
                    className="w-full resize-none rounded-lg border border-zinc-700/70 bg-zinc-900 px-2.5 py-1.5 text-[12px] text-zinc-100 placeholder:text-zinc-600 focus:border-amber-300/40 focus:outline-none"
                  />
                  <div className="flex justify-end gap-2">
                    <button type="button" onClick={() => setAdding(false)} className="h-7 rounded-lg px-2.5 text-[11px] text-zinc-400 hover:bg-white/[0.06]">Cancel</button>
                    <button type="button" onClick={submit} className="h-7 rounded-lg bg-amber-300 px-2.5 text-[11px] font-semibold text-[#0a0a0d] hover:bg-amber-200">Save note</button>
                  </div>
                </div>
              )}

              {error && <div className="text-[11px] text-rose-400">{error}</div>}
              {notes.length === 0 && !adding && (
                <div className="rounded-xl border border-dashed border-zinc-800 px-4 py-8 text-center text-[12px] text-zinc-500">
                  No notes yet. Ask Stark to leave a note for this {scopeType === 'group' ? 'project' : 'chat'}.
                </div>
              )}

              {notes.map((note) => (
                <div key={note.id} className="rounded-xl border border-amber-300/15 bg-amber-300/[0.035] p-3">
                  <div className="flex items-start gap-2">
                    <div className="min-w-0 flex-1">
                      <div className="text-[12.5px] font-semibold text-zinc-100">{note.title}</div>
                      {note.body && <div className="mt-1 whitespace-pre-wrap text-[11.5px] leading-5 text-zinc-400">{note.body}</div>}
                    </div>
                    <button type="button" onClick={() => remove(note)} className="grid h-6 w-6 place-items-center rounded text-zinc-600 hover:bg-rose-500/10 hover:text-rose-300" title="Delete note">
                      <Trash2 size={12} />
                    </button>
                  </div>
                  {note.todos.length > 0 && (
                    <div className="mt-2 space-y-1">
                      {note.todos.map((todo, index) => (
                        <button
                          key={`${note.id}-${index}`}
                          type="button"
                          onClick={() => toggleTodo(note, index)}
                          className="flex w-full items-start gap-2 text-left text-[11.5px] text-zinc-300"
                        >
                          <span className={`mt-0.5 grid h-3.5 w-3.5 flex-none place-items-center rounded border ${todo.done ? 'border-emerald-400 bg-emerald-400 text-[#0a0a0d]' : 'border-zinc-600'}`}>
                            {todo.done && <Check size={9} />}
                          </span>
                          <span className={todo.done ? 'text-zinc-600 line-through' : ''}>{todo.text}</span>
                        </button>
                      ))}
                    </div>
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
