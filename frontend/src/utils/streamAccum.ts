import type { StreamDelta } from '../types';

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

export interface StreamAccum {
  text: string;
  reasoning: string;
  agent: string;
  segments: StreamSegment[];
  toolEvents: LiveToolEvent[];
}

export interface RateAccum {
  lastTs: number;
  lastChars: number;
  ema: number;
}

export function emptyStreamAccum(): StreamAccum {
  return { text: '', reasoning: '', agent: '', segments: [], toolEvents: [] };
}

export function emptyRateAccum(): RateAccum {
  return { lastTs: 0, lastChars: 0, ema: 0 };
}

let toolSeq = 0;

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
  if (delta.kind === 'reasoning') {
    acc.reasoning += delta.text || '';
    appendSegment(acc, 'reasoning', delta.text || '', delta.agent);
  } else if (delta.kind === 'text') {
    const chars = (delta.text || '').length;
    if (rate.lastTs > 0) {
      const dt = Math.max(16, now - rate.lastTs) / 1000;
      const r = chars / 4 / dt; // ~4 chars per token
      rate.ema = rate.ema === 0 ? r : rate.ema * 0.8 + r * 0.2;
    }
    rate.lastTs = now;
    rate.lastChars = chars;
    acc.text += delta.text || '';
    appendSegment(acc, 'text', delta.text || '', delta.agent);
  } else if (delta.kind === 'end') {
    rate.ema = 0;
  } else if (delta.kind === 'tool_started') {
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
      };
      acc.toolEvents.push(ev);
      acc.segments.push({ type: 'tool', ev });
    }
  }
  if (delta.agent) acc.agent = delta.agent;
  return Math.round(rate.ema);
}
