import { useState } from 'react';
import { Check, ChevronDown, HelpCircle, X } from 'lucide-react';
import type { AskAnswer, AskBatch, AskOption, AskQuestion } from '../../services/api';
import { getChatState, submitAskAnswers } from '../../services/api';
import './askDock.css';

interface Props {
  batch: AskBatch;
  compact?: boolean;
  onClose: () => void;
}

function optionLabel(option: string | AskOption): string {
  return typeof option === 'string' ? option : option.label;
}

/**
 * Inline ask_user_question panel. It replaces the prompt dock for the chat
 * that owns the question instead of taking over the whole screen.
 */
export default function AskQuestionPanel({ batch, compact = false, onClose }: Props) {
  const [answers, setAnswers] = useState<Record<string, { selected: string[]; custom: string }>>({});
  const [customOpen, setCustomOpen] = useState<Record<string, boolean>>({});
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState(false);

  const toggleOption = (question: AskQuestion, label: string) => {
    setAnswers((previous) => {
      const current = previous[question.id] ?? { selected: [], custom: '' };
      let selected: string[];
      if (question.multiSelect ?? question.multi_select) {
        selected = current.selected.includes(label)
          ? current.selected.filter((entry) => entry !== label)
          : [...current.selected, label];
      } else {
        selected = [label];
      }
      return { ...previous, [question.id]: { ...current, selected } };
    });
  };

  const setCustom = (question: AskQuestion, text: string) => {
    setAnswers((previous) => {
      const current = previous[question.id] ?? { selected: [], custom: '' };
      return { ...previous, [question.id]: { ...current, custom: text } };
    });
  };

  const isAnswered = (question: AskQuestion) => {
    const answer = answers[question.id];
    return !!(answer && (answer.selected.length > 0 || answer.custom.trim()));
  };

  const answeredCount = batch.questions.filter(isAnswered).length;
  const allAnswered = answeredCount === batch.questions.length;

  const submit = async () => {
    if (submitting || !allAnswered) return;
    const payload: AskAnswer[] = batch.questions.map((question) => {
      const answer = answers[question.id] ?? { selected: [], custom: '' };
      return { id: question.id, selected: answer.selected, custom: answer.custom.trim() };
    });
    setSubmitting(true);
    setError(null);
    const ok = await submitAskAnswers(batch.batch_id, payload);
    setSubmitting(false);
    if (!ok) {
      setError('Failed to deliver your answer. The agent may have timed out.');
      return;
    }
    const isPlanReview = batch.questions.some((question) => question.intent?.kind === 'plan-review');
    if (isPlanReview && batch.chat_id) {
      try {
        const chatState = await getChatState(batch.chat_id);
        window.dispatchEvent(new CustomEvent('stark:mode-changed', { detail: chatState.mode }));
      } catch {
        // Non-fatal: the badge refreshes on the next chat fetch.
      }
    }
    onClose();
  };

  return (
    <div className={`ask-panel ${compact ? 'is-compact' : ''}`} role="dialog" aria-label="Question">
      <div className="ask-panel-head">
        <HelpCircle size={compact ? 13 : 16} className="ask-panel-icon" />
        <div className="ask-panel-heading">
          <div className="ask-panel-title">Question</div>
          <div className="ask-panel-subtitle">The agent is waiting for your answer.</div>
        </div>
        <button
          type="button"
          onClick={() => setCollapsed((value) => !value)}
          className="ask-panel-icon-button"
          aria-label={collapsed ? 'Expand question' : 'Collapse question'}
        >
          <ChevronDown size={compact ? 13 : 15} className={collapsed ? 'is-collapsed' : ''} />
        </button>
        <button
          type="button"
          onClick={onClose}
          className="ask-panel-icon-button"
          aria-label="Dismiss"
        >
          <X size={compact ? 13 : 15} />
        </button>
      </div>

      {!collapsed && (
        <div className="ask-panel-body">
          {batch.questions.map((question) => {
            const answer = answers[question.id] ?? { selected: [], custom: '' };
            const options = question.options ?? [];
            const multi = question.multiSelect ?? question.multi_select;
            const custom = customOpen[question.id] || !!answer.custom;

            return (
              <div className="ask-question" key={question.id}>
                <div className="ask-question-title">{question.question}</div>
                <div className="ask-question-hint">
                  {multi ? 'Select one or more answers' : 'Select one answer'}
                </div>
                {question.detail && <div className="ask-question-detail">{question.detail}</div>}

                <div className="ask-options">
                  {options.map((option, index) => {
                    const label = optionLabel(option);
                    const description = typeof option === 'object' ? option.description : undefined;
                    const selected = answer.selected.includes(label);
                    return (
                      <button
                        key={`${label}-${index}`}
                        type="button"
                        onClick={() => toggleOption(question, label)}
                        className={`ask-option ${selected ? 'is-selected' : ''}`}
                      >
                        <span className={`ask-radio ${multi ? 'is-multi' : ''} ${selected ? 'is-selected' : ''}`}>
                          {selected && <Check size={10} />}
                        </span>
                        <span className="ask-option-copy">
                          <span className="ask-option-label">{label}</span>
                          {description && <span className="ask-option-desc">{description}</span>}
                        </span>
                        <span className="ask-option-index">{index + 1}</span>
                      </button>
                    );
                  })}

                  <button
                    type="button"
                    onClick={() => {
                      setCustomOpen((previous) => ({ ...previous, [question.id]: true }));
                      if (!custom) {
                        setAnswers((previous) => ({
                          ...previous,
                          [question.id]: { selected: [], custom: '' },
                        }));
                      }
                    }}
                    className={`ask-option ${custom ? 'is-selected' : ''}`}
                  >
                    <span className={`ask-radio ${multi ? 'is-multi' : ''} ${custom ? 'is-selected' : ''}`}>
                      {custom && <Check size={10} />}
                    </span>
                    <span className="ask-option-copy">
                      <span className="ask-option-label">Type your own answer…</span>
                    </span>
                    <span className="ask-option-index">{options.length + 1}</span>
                  </button>

                  {custom && (
                    <input
                      autoFocus
                      value={answer.custom}
                      onChange={(event) => setCustom(question, event.target.value)}
                      placeholder="Type your answer…"
                      className="ask-custom-input"
                      onKeyDown={(event) => {
                        if (event.key === 'Enter' && !event.shiftKey && allAnswered) {
                          event.preventDefault();
                          void submit();
                        }
                      }}
                    />
                  )}
                </div>
              </div>
            );
          })}

          {error && <div className="ask-error">{error}</div>}
        </div>
      )}

      <div className="ask-panel-foot">
        <button type="button" onClick={onClose} className="ask-dismiss">
          Dismiss
        </button>
        <div className="ask-submit-wrap">
          <span className="ask-progress">{answeredCount}/{batch.questions.length} answered</span>
          <button
            type="button"
            onClick={() => void submit()}
            disabled={submitting || !allAnswered}
            className="ask-submit"
          >
            {submitting ? 'Sending…' : 'Submit'}
          </button>
        </div>
      </div>
    </div>
  );
}
