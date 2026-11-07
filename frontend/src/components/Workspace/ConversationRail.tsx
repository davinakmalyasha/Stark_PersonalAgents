import { useCallback, useMemo, useState } from 'react';
import type { RefObject } from 'react';
import type { ChatMessage } from '../../types';

interface Props {
  messages: ChatMessage[];
  scrollRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
}

interface TurnMarker {
  id: string;
  role: ChatMessage['role'];
  title: string;
  snippet: string;
  timestamp: string;
  aria: string;
}

const MAX_MARKERS = 6;

function plainPreview(content: string): string {
  return content
    .replace(/```[\s\S]*?```/g, ' code ')
    .replace(/[#>*_`~[\]]/g, ' ')
    .replace(/\s+/g, ' ')
    .trim();
}

function timeLabel(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return '';
  return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function markerForTurn(turn: ChatMessage[], fallbackRole: ChatMessage['role'] = 'agent'): TurnMarker {
  const first = turn[0];
  const user = turn.find((message) => message.role === 'user');
  const answer = [...turn].reverse().find((message) => message.role === 'agent' || message.role === 'error');
  const titleSource = user || first;
  const title = plainPreview(titleSource.content).split(/(?<=[.!?])\s/)[0]?.slice(0, 58) || 'Turn';
  const snippet = plainPreview(answer?.content || user?.content || first.content).slice(0, 190);
  const role = user?.role || answer?.role || fallbackRole;
  return {
    id: first.id,
    role,
    title: title || 'Turn',
    snippet: snippet || 'No preview available.',
    timestamp: timeLabel(titleSource.timestamp),
    aria: `${title} — ${snippet.slice(0, 80)}`,
  };
}

function buildMarkers(messages: ChatMessage[]): TurnMarker[] {
  const turns: ChatMessage[][] = [];
  let current: ChatMessage[] = [];
  for (const message of messages) {
    if (!message.content.trim()) continue;
    if (message.role === 'user' && current.length) {
      turns.push(current);
      current = [];
    }
    current.push(message);
  }
  if (current.length) turns.push(current);
  if (turns.length === 0) return [];

  const markers = turns.map((turn) => markerForTurn(turn));
  if (markers.length <= MAX_MARKERS) return markers;

  // Keep the newest five turns individually and fold all older history into
  // one compact "earlier" marker. The rail always has six quiet dashes.
  const earlierTurns = turns.slice(0, turns.length - (MAX_MARKERS - 1));
  const earlierMarkers = earlierTurns.map((turn) => markerForTurn(turn));
  const first = earlierMarkers[0];
  const earlier: TurnMarker = {
    id: first.id,
    role: 'system',
    title: 'Earlier conversation',
    snippet: `${earlierMarkers.length} earlier turns · ${first.title}`,
    timestamp: first.timestamp,
    aria: `Earlier conversation — ${earlierMarkers.length} turns`,
  };
  return [earlier, ...markers.slice(-(MAX_MARKERS - 1))];
}

/**
 * Six evenly spaced turn markers, like a Codex conversation rail: a single
 * column of short dashes, no track and no scrollbar. Hover/focus extends the
 * dash and shows a preview; clicking jumps to that turn.
 */
export default function ConversationRail({ messages, scrollRef, contentRef }: Props) {
  const [hovered, setHovered] = useState<string | null>(null);
  const markers = useMemo(() => buildMarkers(messages), [messages]);

  const jump = useCallback((id: string) => {
    const content = contentRef.current;
    if (!content) return;
    const anchor = content.querySelector<HTMLElement>(`[data-message-anchor="${CSS.escape(id)}"]`);
    if (!anchor) return;
    anchor.scrollIntoView({ behavior: 'smooth', block: 'center' });
    if (scrollRef.current) scrollRef.current.focus({ preventScroll: true });
    setHovered(null);
  }, [contentRef, scrollRef]);

  if (markers.length < 2) return null;

  const activeIndex = markers.findIndex((marker) => marker.id === hovered);
  const active = activeIndex >= 0 ? markers[activeIndex] : null;

  return (
    <div className="conversation-rail" aria-label="Turn timeline">
      <div className="conversation-rail-marks">
        {markers.map((marker, index) => {
          const isActive = marker.id === hovered;
          const top = markers.length === 1 ? 50 : 10 + (index / (markers.length - 1)) * 80;
          return (
            <button
              key={`${marker.id}-${index}`}
              type="button"
              className={`conversation-rail-mark is-${marker.role} ${isActive ? 'is-active' : ''}`}
              style={{ top: `${top}%` }}
              onPointerEnter={() => setHovered(marker.id)}
              onPointerLeave={() => setHovered((current) => (current === marker.id ? null : current))}
              onFocus={() => setHovered(marker.id)}
              onBlur={() => setHovered((current) => (current === marker.id ? null : current))}
              onClick={() => jump(marker.id)}
              aria-label={`Jump to turn: ${marker.aria}`}
            >
              <span />
            </button>
          );
        })}
      </div>
      {active && (
        <div
          className="conversation-rail-preview"
          style={{ top: `${markers.length === 1 ? 50 : 10 + (activeIndex / (markers.length - 1)) * 80}%` }}
          role="tooltip"
        >
          <div className="conversation-rail-preview-head">
            <span className={`conversation-rail-preview-dot is-${active.role}`} />
            <span className="conversation-rail-preview-title">{active.title}</span>
            <span className="conversation-rail-preview-time">{active.timestamp}</span>
          </div>
          <div className="conversation-rail-preview-snippet">{active.snippet}</div>
          <div className="conversation-rail-preview-hint">Click to jump</div>
        </div>
      )}
    </div>
  );
}
