import { useEffect, useRef, useState } from 'react';
import { X, HelpCircle, Check } from 'lucide-react';
import type { AskAnswer, AskBatch, AskOption, AskQuestion } from '../../services/api';
import { fetchPendingQuestions, submitAskAnswers } from '../../services/api';

interface Props {
  visible: boolean;
  onClose?: () => void;
}

/** Human-in-the-loop: renders pending ask_user batches (agents block until
 * answered). Backend /api/ask/pending + /api/ask/answer already exist — this
 * is the missing frontend wiring (P10/P2). */
export default function QuestionModal({ visible, onClose }: Props) {
  const [batches, setBatches] = useState<AskBatch[]>([]);
  const [answers, setAnswers] = useState<Record<string, { selected: string[]; custom: string }>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const pollingRef = useRef<number | null>(null);

  const activeBatch = batches[0] ?? null;

  // Poll pending batches while the modal is visible (or has data to show).
  useEffect(() => {
    if (!visible) return;
    const tick = async () => {
      const next = await fetchPendingQuestions();
      setBatches((prev) => {
        if (next.length === 0 && prev.length === 0) return prev;
        if (JSON.stringify(next.map((b) => b.batch_id)) !== JSON.stringify(prev.map((b) => b.batch_id))) {
          setAnswers({});
          setError(null);
        }
        return next;
      });
    };
    tick();
    const id = window.setInterval(tick, 1500);
    pollingRef.current = id;
    return () => { if (pollingRef.current) window.clearInterval(pollingRef.current); };
  }, [visible]);

  if (!visible || !activeBatch) return null;

  const optionLabel = (opt: string | AskOption) => (typeof opt === 'string' ? opt : opt.label);

  const toggleOption = (q: AskQuestion, label: string) => {
    setAnswers((prev) => {
      const cur = prev[q.id] ?? { selected: [], custom: '' };
      let selected: string[];
      if (q.multiSelect) {
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

  const submit = async () => {
    if (!activeBatch || submitting) return;
    const payload: AskAnswer[] = activeBatch.questions.map((q) => {
      const a = answers[q.id] ?? { selected: [], custom: '' };
      return { id: q.id, selected: a.selected, custom: a.custom.trim() };
    });
    if (activeBatch.questions.length > 0 && payload.some((p) => p.selected.length === 0 && !p.custom)) {
      setError('Answer all questions before submitting.');
      return;
    }
    setSubmitting(true);
    setError(null);
    const ok = await submitAskAnswers(activeBatch.batch_id, payload);
    setSubmitting(false);
    if (!ok) {
      setError('Failed to deliver your answer. The agent may have timed out.');
      return;
    }
    setBatches((prev) => prev.filter((b) => b.batch_id !== activeBatch.batch_id));
    setAnswers({});
    onClose?.();
  };

  return (
    <div className="fixed inset-0 z-[200] flex items-center justify-center bg-black/60 backdrop-blur-sm p-4">
      <div className="modal-panel-enter w-full max-w-lg rounded-2xl border border-zinc-800 bg-[#141417] shadow-2xl overflow-hidden">
        <div className="flex items-center gap-2.5 px-5 py-4 border-b border-zinc-800">
          <HelpCircle size={17} className="text-indigo-400 flex-shrink-0" />
          <div className="flex-1 min-w-0">
            <div className="text-[13px] font-bold text-zinc-100">Question from the agent</div>
            <div className="text-[11px] text-zinc-500">The agent is waiting for your answer to continue.</div>
          </div>
          <button type="button" onClick={onClose} className="p-1.5 text-zinc-500 hover:text-zinc-300 rounded-md" aria-label="Dismiss">
            <X size={15} />
          </button>
        </div>

        <div className="px-5 py-4 max-h-[60vh] overflow-y-auto space-y-5">
          {activeBatch.questions.map((q) => {
            const a = answers[q.id] ?? { selected: [], custom: '' };
            return (
              <div key={q.id}>
                <div className="text-[14px] font-semibold text-zinc-100 leading-snug">{q.question}</div>
                {q.header && <div className="text-[11.5px] text-zinc-400 mt-0.5">{q.header}</div>}
                {q.detail && <div className="text-[12.5px] text-zinc-500 mt-1 leading-relaxed">{q.detail}</div>}

                {Array.isArray(q.options) && q.options.length > 0 && (
                  <div className="mt-2.5 flex flex-col gap-1.5">
                    {q.options.map((opt) => {
                      const label = optionLabel(opt);
                      const desc = typeof opt === 'object' ? opt.description : undefined;
                      const selected = a.selected.includes(label);
                      return (
                        <button
                          key={label}
                          type="button"
                          onClick={() => toggleOption(q, label)}
                          className={`flex items-center gap-2.5 px-3 py-2 rounded-lg border text-left transition-colors cursor-pointer ${
                            selected
                              ? 'border-indigo-500/60 bg-indigo-500/10'
                              : 'border-zinc-800 bg-zinc-900/40 hover:border-zinc-700'
                          }`}
                        >
                          <span className={`flex-shrink-0 w-4 h-4 rounded ${q.multiSelect ? 'rounded-md' : 'rounded-full'} border flex items-center justify-center ${
                            selected ? 'border-indigo-400 bg-indigo-500/30' : 'border-zinc-600'
                          }`}>
                            {selected && <Check size={11} className="text-indigo-300" />}
                          </span>
                          <span className="min-w-0">
                            <span className="block text-[13px] text-zinc-200">{label}</span>
                            {desc && <span className="block text-[11.5px] text-zinc-500">{desc}</span>}
                          </span>
                        </button>
                      );
                    })}
                  </div>
                )}

                <input
                  value={a.custom}
                  onChange={(e) => setCustom(q, e.target.value)}
                  placeholder={q.options?.length ? 'Or type your own answer…' : 'Type your answer…'}
                  className="mt-2.5 w-full px-3 py-2 rounded-lg bg-zinc-900 border border-zinc-800 text-[13px] text-zinc-200 placeholder:text-zinc-600 focus:outline-none focus:border-indigo-500/50"
                  onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); } }}
                />
              </div>
            );
          })}

          {error && <div className="text-[12px] text-red-400">{error}</div>}
        </div>

        <div className="px-5 py-3.5 border-t border-zinc-800 flex items-center justify-between">
          <span className="text-[11px] text-zinc-600">
            {activeBatch.questions.filter(isAnswered).length}/{activeBatch.questions.length} answered
          </span>
          <button
            type="button"
            onClick={submit}
            disabled={submitting}
            className="pressable px-4 py-2 rounded-lg bg-indigo-500 hover:bg-indigo-400 text-white text-[12.5px] font-semibold transition-colors cursor-pointer disabled:opacity-50"
          >
            {submitting ? 'Sending…' : 'Submit answer'}
          </button>
        </div>
      </div>
    </div>
  );
}
