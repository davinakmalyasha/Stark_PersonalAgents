import type { ChildReference, RunStatusNotice, StreamDelta } from '../types';

/**
 * Shared stream accumulator — single implementation of the SSE delta fold
 * used by useChat / usePipeline / useSessionRunner (they previously each
 * carried a copy that drifted: reasoning dropped, tool frames unhandled).
 *
 * Hooks keep their own refs/state but delegate the fold here so live
 * behavior (tok/s, thinking, Codex-style tool pills) stays identical on
 * every surface.
 */

export interface LiveToolEvent {
  id: string;
  name: string;
  args?: string;
  status: 'running' | 'done';
  ok?: boolean;
  ms?: number;
  preview?: string;
  agent?: string;
  children?: ChildReference[];
}

/**
 * Ordered live timeline. Reasoning/answer text and tool happenings arrive
 * interleaved (pydantic-ai: thinking deltas → tool call → tool result → text),
 * and Codex-style UI keeps that order so each reasoning burst reads as its
 * own step instead of one continuous blob (the "everything in one line" bug).
 */
export type StreamSegment =
  | { type: 'reasoning'; text: string; agent?: string }
  | { type: 'text'; text: string; agent?: string }
  | { type: 'tool'; ev: LiveToolEvent };

export interface RetryNotice {
  attempt: number;
  max: number;
  /** When the notice arrived — lets the UI keep it visible until real output. */
  at: number;
}

export interface StreamAccum {
  text: string;
  reasoning: string;
  agent: string;
  segments: StreamSegment[];
  toolEvents: LiveToolEvent[];
  /** Ephemeral transport state; never persisted into the answer. */
  retry: RetryNotice | null;
  /** Compaction/context notices are transient; the completed marker persists. */
  status: RunStatusNotice | null;
}

export interface RateAccum {
  lastTs: number;
  lastChars: number;
  ema: number;
}

export function emptyStreamAccum(): StreamAccum {
  return { text: '', reasoning: '', agent: '', segments: [], toolEvents: [], retry: null, status: null };
}

export function emptyRateAccum(): RateAccum {
  return { lastTs: 0, lastChars: 0, ema: 0 };
}

function legacyChildRefs(text: string): ChildReference[] {
  const refs: ChildReference[] = [];
  for (const line of text.split(/\r?\n/)) {
    const delegated = line.match(/^\s*-\s*(del-[\w-]+)\s*[·|]\s*([^·|]+?)\s*[·|]\s*(.+?)\s*[—-].*?\(chat:\s*([^)]+)\)/i);
    if (delegated) {
      refs.push({ chatId: delegated[4].trim(), jobId: delegated[1], agentName: delegated[2].trim(), label: delegated[3].trim() });
      continue;
    }
    const spawned = line.match(/\[SPAWNED\]\s+child=([^\s;]+)(?:\s+name=([^;]+))?/i);
    if (spawned) refs.push({ chatId: spawned[1], agentName: spawned[2]?.trim(), label: spawned[2]?.trim() });
  }
  return refs;
}

let toolSeq = 0;

/**
 * The backend currently emits reconnect notices as assistant text:
 * `_Reconnecting (2/5)…_`. They are transport state, not an answer. Strip
 * them out of the content stream and surface the latest attempt separately so
 * retries update one status row instead of stacking five italic sentences.
 */
const RETRY_NOTICE_RE = /_{0,2}\s*Reconnecting\s*\(\s*(\d+)\s*\/\s*(\d+)\s*\)\s*[.…]*\s*_{0,2}/gi;

export function extractRetryNotices(text: string, now: number): { retry: RetryNotice | null; rest: string } {
  if (!text || !/Reconnecting/i.test(text)) return { retry: null, rest: text };
  let retry: RetryNotice | null = null;
  const rest = text.replace(RETRY_NOTICE_RE, (_match, attempt: string, max: string) => {
    retry = { attempt: Number(attempt), max: Number(max), at: now };
    return '';
  });
  return { retry, rest };
}

function appendSegment(acc: StreamAccum, type: 'reasoning' | 'text', text: string, agent?: string) {
  if (!text) return;
  const last = acc.segments[acc.segments.length - 1];
  if (last && last.type === type) {
    last.text += text;
    if (agent) last.agent = agent;
    return;
  }
  acc.segments.push({ type, text, agent: agent || undefined });
}

/** Fold one delta into the accumulators. Returns the rounded tok/s rate. */
export function applyDeltaToAccum(
  acc: StreamAccum,
  rate: RateAccum,
  delta: StreamDelta,
  now: number,
): number {
  if (!delta) return Math.round(rate.ema);
  // A finished compaction notice is a one-shot event; let it clear as soon as
  // the run produces its next output instead of pinning it for the whole turn.
  if (
    acc.status
    && acc.status.kind !== 'compaction_started'
    && (delta.kind === 'text' || delta.kind === 'reasoning' || delta.kind === 'tool_started' || delta.kind === 'tool_finished')
  ) {
    acc.status = null;
  }
  if (delta.kind === 'reasoning') {
    acc.retry = null;
    acc.reasoning += delta.text || '';
    appendSegment(acc, 'reasoning', delta.text || '', delta.agent);
  } else if (delta.kind === 'text') {
    const parsed = extractRetryNotices(delta.text || '', now);
    if (parsed.retry) acc.retry = parsed.retry;
    const text = parsed.rest;
    if (!text.trim()) {
      if (delta.agent) acc.agent = delta.agent;
      return Math.round(rate.ema);
    }
    // Real output means the stream reconnected; drop the retry row.
    acc.retry = null;
    const chars = text.length;
    if (rate.lastTs > 0) {
      const dt = Math.max(16, now - rate.lastTs) / 1000;
      const r = chars / 4 / dt; // ~4 chars per token
      rate.ema = rate.ema === 0 ? r : rate.ema * 0.8 + r * 0.2;
    }
    rate.lastTs = now;
    rate.lastChars = chars;
    acc.text += text;
    appendSegment(acc, 'text', text, delta.agent);
  } else if (delta.kind === 'status' && delta.status) {
    acc.status = delta.status;
  } else if (delta.kind === 'end') {
    acc.retry = null;
    rate.ema = 0;
  } else if (delta.kind === 'tool_started') {
    acc.retry = null;
    toolSeq += 1;
    const ev: LiveToolEvent = {
      id: `${Date.now()}-${toolSeq}`,
      name: delta.tool || 'tool',
      args: delta.args,
      status: 'running',
      agent: delta.agent,
    };
    acc.toolEvents.push(ev);
    if (acc.toolEvents.length > 50) acc.toolEvents.splice(0, acc.toolEvents.length - 50);
    acc.segments.push({ type: 'tool', ev });
    if (acc.segments.length > 300) acc.segments.splice(0, acc.segments.length - 300);
  } else if (delta.kind === 'tool_finished') {
    acc.retry = null;
    const name = delta.tool || 'tool';
    let open = -1;
    for (let i = acc.toolEvents.length - 1; i >= 0; i--) {
      if (acc.toolEvents[i].status === 'running' && acc.toolEvents[i].name === name) {
        open = i;
        break;
      }
    }
    if (open >= 0) {
      const ev = acc.toolEvents[open];
      ev.status = 'done';
      ev.ok = delta.ok;
      ev.ms = delta.ms;
      ev.preview = delta.text || undefined;
      ev.children = delta.children?.length ? delta.children : legacyChildRefs(delta.text || '');
    } else {
      // Finish without a recorded start (mirror attach, reconnect): keep it.
      toolSeq += 1;
      const ev: LiveToolEvent = {
        id: `${Date.now()}-${toolSeq}`,
        name,
        args: delta.args,
        status: 'done',
        ok: delta.ok,
        ms: delta.ms,
        preview: delta.text || undefined,
        agent: delta.agent,
        children: delta.children?.length ? delta.children : legacyChildRefs(delta.text || ''),
      };
      acc.toolEvents.push(ev);
      acc.segments.push({ type: 'tool', ev });
    }
  }
  if (delta.agent) acc.agent = delta.agent;
  return Math.round(rate.ema);
}
