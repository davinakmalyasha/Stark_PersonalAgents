import { useEffect, useRef, useState } from 'react';
import { Check, Copy, RotateCcw } from 'lucide-react';
import type { ChatPreviewMessage } from '../types';
import type { ChatActor } from './canvasActors';

interface Props {
  message: ChatPreviewMessage;
  actor: ChatActor;
  /** Optimistic state shown before the server row exists. */
  pending?: string;
  /** Animated agent bubble while the run is streaming. */
  live?: boolean;
  canRewind?: boolean;
  onRewind?: (message: ChatPreviewMessage) => void;
}

function clock(iso: string): string {
  if (!iso) return '';
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

export default function CanvasChatBubble({
  message,
  actor,
  pending,
  live = false,
  canRewind = true,
  onRewind,
}: Props) {
  const [copied, setCopied] = useState(false);
  const [rewindArmed, setRewindArmed] = useState(false);
  const copyTimer = useRef<number | null>(null);
  const rewindTimer = useRef<number | null>(null);

  useEffect(() => () => {
    if (copyTimer.current) window.clearTimeout(copyTimer.current);
    if (rewindTimer.current) window.clearTimeout(rewindTimer.current);
  }, []);

  const isUser = message.role === 'user';
  const isError = message.role === 'error';

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(message.content);
      setCopied(true);
      if (copyTimer.current) window.clearTimeout(copyTimer.current);
      copyTimer.current = window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable */
    }
  };

  const armRewind = () => {
    setRewindArmed(true);
    if (rewindTimer.current) window.clearTimeout(rewindTimer.current);
    rewindTimer.current = window.setTimeout(() => setRewindArmed(false), 3200);
  };

  return (
    <div className={`canvas-bubble-row ${isUser ? 'is-user' : 'is-agent'} ${isError ? 'is-error' : ''} ${live ? 'is-live' : ''}`}>
      {!isUser && <span className="canvas-bubble-avatar" aria-hidden="true">{actor.letter}</span>}
      <div className="canvas-bubble">
        <div className="canvas-bubble-head">
          <span className="canvas-bubble-name">{isUser ? 'You' : isError ? 'Error' : 'Agent'}</span>
          {!isUser && !isError && <span className="canvas-bubble-badge">auto</span>}
          {live && (
            <span className="canvas-bubble-typing" aria-label="Working">
              <i /><i /><i />
            </span>
          )}
        </div>
        <div className="canvas-bubble-text">{message.content}</div>
        <div className="canvas-bubble-foot">
          <span className="canvas-bubble-actions">
            <button
              type="button"
              onClick={() => { void copy(); }}
              className="canvas-bubble-action"
              title={copied ? 'Copied' : 'Copy message'}
              aria-label="Copy message"
            >
              {copied ? <Check size={12} className="text-emerald-400" /> : <Copy size={12} />}
            </button>
            {isUser && onRewind && (
              rewindArmed ? (
                <button
                  type="button"
                  onClick={() => {
                    setRewindArmed(false);
                    onRewind(message);
                  }}
                  className="canvas-bubble-action is-confirm"
                  title="Click again to confirm — this removes this message and everything after it"
                  aria-label="Confirm rewind"
                >
                  <RotateCcw size={11} /> Rewind?
                </button>
              ) : (
                <button
                  type="button"
                  onClick={armRewind}
                  disabled={!canRewind}
                  className="canvas-bubble-action is-rewind"
                  title={canRewind ? 'Rewind to before this message' : 'Wait for this run to finish'}
                  aria-label="Rewind to this message"
                >
                  <RotateCcw size={12} />
                </button>
              )
            )}
          </span>
          {pending && <span className="canvas-bubble-pending">{pending}</span>}
          <span className="canvas-bubble-time">{clock(message.timestamp)}</span>
        </div>
      </div>
      {isUser && <span className="canvas-bubble-avatar is-user" aria-hidden="true">U</span>}
    </div>
  );
}
