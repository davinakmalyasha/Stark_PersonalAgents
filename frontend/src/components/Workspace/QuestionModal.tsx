import { useEffect, useRef, useState } from 'react';
import { Check, ChevronDown, HelpCircle, X } from 'lucide-react';
import type { AskAnswer, AskBatch, AskOption, AskQuestion } from '../../services/api';
import { fetchPendingQuestions, submitAskAnswers, getChatState } from '../../services/api';

interface Props {
  /** Mounted globally; polling pauses while the tab is hidden. */
  enabled?: boolean;
}

/** Human-in-the-loop: renders pending ask_user batches (agents block until
 *  answered). Global mount so the overlay appears on Home, Coding, and rooms —
 *  not just the route that happened to import it. */
export default function QuestionModal({ enabled = true }: Props) {
  const [batches, setBatches] = useState<AskBatch[]>([]);
  const [dismissed, setDismissed] = useState<Set<string>>(new Set());
  const [answers, setAnswers] = useState<Record<string, { selected: string[]; custom: string }>>({});
  const [customOpen, setCustomOpen] = useState<Record<string, boolean>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(false);
  const pollingRef = useRef<number | null>(null);
  const batchIdsRef = useRef('');

  const visibleBatches = batches.filter((b) => !dismissed.has(b.batch_id));
  const activeBatch = visibleBatches[0] ?? null;

  useEffect(() => {
    if (!enabled) return;
    const tick = async () => {
      if (document.hidden) return;
      try {
        const next = await fetchPendingQuestions();
        const nextIds = next.map((b) => b.batch_id).join(',');
        if (nextIds !== batchIdsRef.current) {
          batchIdsRef.current = nextIds;
          setAnswers({});
          setCustomOpen({});
          setError(null);
          setCollapsed(false);
        }
        setBatches(next);
      } catch {
        /* backend briefly unavailable */
      }
    };
    void tick();
    const id = window.setInterval(tick, 1500);
    pollingRef.current = id;
    return () => {
      if (pollingRef.current) window.clearInterval(pollingRef.current);
    };
  }, [enabled]);

  if (!enabled || !activeBatch) return null;

  const optionLabel = (opt: string | AskOption) => (typeof opt === 'string' ? opt : opt.label);

  const toggleOption = (q: AskQuestion, label: string) => {
    setAnswers((prev) => {
      const cur = prev[q.id] ?? { selected: [], custom: '' };
      let selected: string[];
      if (q.multiSelect ?? q.multi_select) {
        selected = cur.selected.includes(label) ? cur.selected.filter((s) => s !== label) : [...cur.selected, label];
      } else {
        selected = [label];
      }
      return { ...prev, [q.id]: { ...cur, selected } };
    });
  };

  const setCustom = (q: AskQuestion, text: string) => {
    setAnswers((prev) => {
      const cur = prev[q.id] ?? { selected: [], custom: '' };
      return { ...prev, [q.id]: { ...cur, custom: text } };
    });
  };

  const isAnswered = (q: AskQuestion) => {
    const a = answers[q.id];
    return !!(a && (a.selected.length > 0 || a.custom.trim()));
  };

  const answeredCount = activeBatch.questions.filter(isAnswered).length;
  const allAnswered = answeredCount === activeBatch.questions.length;

  const dismiss = () => {
    setDismissed((prev) => new Set(prev).add(activeBatch.batch_id));
  };

  const submit = async () => {
    if (!activeBatch || submitting || !allAnswered) return;
    const payload: AskAnswer[] = activeBatch.questions.map((q) => {
      const a = answers[q.id] ?? { selected: [], custom: '' };
      return { id: q.id, selected: a.selected, custom: a.custom.trim() };
    });
    setSubmitting(true);
    setError(null);
    const ok = await submitAskAnswers(activeBatch.batch_id, payload);
    setSubmitting(false);
    if (!ok) {
      setError('Failed to deliver your answer. The agent may have timed out.');
      return;
    }
    const isPlanReview = activeBatch.questions.some((q) => q.intent?.kind === 'plan-review');
    if (isPlanReview && activeBatch.chat_id) {
      try {
        const state = await getChatState(activeBatch.chat_id);
        window.dispatchEvent(new CustomEvent('stark:mode-changed', { detail: state.mode }));
      } catch {
        // Non-fatal: the badge refreshes on the next chat fetch.
      }
    }
    setDismissed((prev) => new Set(prev).add(activeBatch.batch_id));
  };

  return (
    <div className="fixed inset-0 z-[200] flex items-end md:items-center justify-center bg-black/60 backdrop-blur-sm p-0 md:p-4 modal-backdrop-enter">
      <div
        className="modal-panel-enter w-full md:max-w-[620px] max-h-[92dvh] md:max-h-[85dvh] flex flex-col overflow-hidden rounded-t-2xl md:rounded-2xl border border-zinc-800 bg-[#141417] shadow-2xl"
        style={{ paddingBottom: 'env(safe-area-inset-bottom)' }}
      >
        <div className="flex items-center gap-2.5 px-4 md:px-5 py-3.5 border-b border-zinc-800 shrink-0">
          <HelpCircle size={16} className="text-zinc-400 flex-shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="text-[12.5px] font-semibold text-zinc-100">Question</div>
            <div className="text-[10.5px] text-zinc-500">The agent is waiting for your answer.</div>
          </div>
          <button
            type="button"
            onClick={() => setCollapsed((v) => !v)}
            className="p-1.5 text-zinc-500 hover:text-zinc-300 rounded-md"
            aria-label={collapsed ? 'Expand question' : 'Collapse question'}
          >
            <ChevronDown size={15} className={`transition-transform ${collapsed ? '-rotate-90' : ''}`} />
          </button>
          <button type="button" onClick={dismiss} className="p-1.5 text-zinc-500 hover:text-zinc-300 rounded-md" aria-label="Dismiss">
            <X size={15} />
          </button>
        </div>

        {!collapsed && (
          <div className="flex-1 min-h-0 overflow-y-auto px-4 md:px-5 py-4 space-y-5">
            {activeBatch.questions.map((q) => {
              const a = answers[q.id] ?? { selected: [], custom: '' };
              const options = q.options ?? [];
              const multi = q.multiSelect ?? q.multi_select;
              const custom = customOpen[q.id] || !!a.custom;

              return (
                <div key={q.id}>
                  <div className="text-[14px] font-semibold text-zinc-100 leading-snug">{q.question}</div>
                  <div className="text-[11px] text-zinc-500 mt-0.5">
                    {multi ? 'Select one or more answers' : 'Select one answer'}
                  </div>
                  {q.detail && <div className="text-[12px] text-zinc-500 mt-1.5 leading-relaxed whitespace-pre-wrap">{q.detail}</div>}

                  <div className="mt-2.5 flex flex-col gap-1.5">
                    {options.map((opt, index) => {
                      const label = optionLabel(opt);
                      const desc = typeof opt === 'object' ? opt.description : undefined;
                      const selected = a.selected.includes(label);
                      return (
                        <button
                          key={`${label}-${index}`}
                          type="button"
                          onClick={() => toggleOption(q, label)}
                          className={`flex items-center gap-2.5 px-3 py-2.5 rounded-xl border text-left transition-colors cursor-pointer ${
                            selected
                              ? 'border-indigo-500/70 bg-indigo-500/[0.08]'
                              : 'border-zinc-800 bg-[#18181b] hover:border-zinc-700 hover:bg-[#1c1c20]'
                          }`}
                        >
                          <span className={`flex-shrink-0 grid place-items-center w-4 h-4 border ${
                            multi ? 'rounded-[5px]' : 'rounded-full'
                          } ${selected ? 'border-indigo-400 bg-indigo-500/30' : 'border-zinc-600'}`}>
                            {selected && <Check size={10} className="text-indigo-300" />}
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block text-[13px] text-zinc-100">{label}</span>
                            {desc && <span className="block text-[11.5px] text-zinc-500 mt-0.5">{desc}</span>}
                          </span>
                          <span className="flex-shrink-0 grid place-items-center w-5 h-5 rounded-md bg-white/[0.06] border border-white/10 text-[9.5px] text-zinc-500 tabular-nums">
                            {index + 1}
                          </span>
                        </button>
                      );
                    })}

                    <button
                      type="button"
                      onClick={() => {
                        setCustomOpen((prev) => ({ ...prev, [q.id]: true }));
                        if (!custom) setAnswers((prev) => ({ ...prev, [q.id]: { selected: [], custom: '' } }));
                      }}
                      className={`flex items-center gap-2.5 px-3 py-2.5 rounded-xl border text-left transition-colors cursor-pointer ${
                        custom ? 'border-indigo-500/70 bg-indigo-500/[0.08]' : 'border-zinc-800 bg-[#18181b] hover:border-zinc-700 hover:bg-[#1c1c20]'
                      }`}
                    >
                      <span className={`flex-shrink-0 grid place-items-center w-4 h-4 border ${multi ? 'rounded-[5px]' : 'rounded-full'} ${custom ? 'border-indigo-400 bg-indigo-500/30' : 'border-zinc-600'}`}>
                        {custom && <Check size={10} className="text-indigo-300" />}
                      </span>
                      <span className="min-w-0 flex-1 text-[13px] text-zinc-100">Type your own answer…</span>
                      <span className="flex-shrink-0 grid place-items-center w-5 h-5 rounded-md bg-white/[0.06] border border-white/10 text-[9.5px] text-zinc-500 tabular-nums">
                        {options.length + 1}
                      </span>
                    </button>

                    {custom && (
                      <input
                        autoFocus
                        value={a.custom}
                        onChange={(e) => setCustom(q, e.target.value)}
                        placeholder="Type your answer…"
                        className="w-full px-3 py-2.5 rounded-xl bg-zinc-900 border border-zinc-700 text-[13px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none focus:border-indigo-500/60"
                        onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && allAnswered) { e.preventDefault(); void submit(); } }}
                      />
                    )}
                  </div>
                </div>
              );
            })}

            {error && <div className="text-[12px] text-red-400">{error}</div>}
          </div>
        )}

        <div className="flex items-center justify-between gap-2 px-4 md:px-5 py-3 border-t border-zinc-800 shrink-0">
          <button
            type="button"
            onClick={dismiss}
            className="px-3 py-2 rounded-lg text-[12px] text-zinc-400 hover:text-zinc-200 hover:bg-white/[0.05] transition-colors cursor-pointer"
          >
            Dismiss
          </button>
          <div className="flex items-center gap-2.5">
            <span className="text-[10.5px] text-zinc-600 tabular-nums">
              {answeredCount}/{activeBatch.questions.length} answered
            </span>
            <button
              type="button"
              onClick={() => void submit()}
              disabled={submitting || !allAnswered}
              className="px-4 py-2 rounded-lg bg-zinc-200 hover:bg-white text-[#0a0a0d] text-[12.5px] font-semibold transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed"
            >
              {submitting ? 'Sending…' : 'Submit'}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
