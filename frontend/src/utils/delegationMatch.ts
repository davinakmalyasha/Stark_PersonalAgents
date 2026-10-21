import { delegationTarget, isDelegationToolName, type TraceStep } from './logParser';
import type { LiveToolEvent, StreamSegment } from './streamAccum';

export type ToolGroupEntry = {
  kind: 'tools';
  events: LiveToolEvent[];
};

export type MessageEntry = {
  kind: 'reasoning' | 'message';
  segment: Extract<StreamSegment, { type: 'reasoning' | 'text' }>;
};

export type ActivityEntry = ToolGroupEntry | MessageEntry;

export function buildEntries(segments: StreamSegment[]): ActivityEntry[] {
  const entries: ActivityEntry[] = [];
  for (const segment of segments) {
    if (segment.type === 'tool') {
      const previous = entries[entries.length - 1];
      if (previous?.kind === 'tools') {
        previous.events.push(segment.ev);
      } else {
        entries.push({ kind: 'tools', events: [segment.ev] });
      }
      continue;
    }
    entries.push({ kind: segment.type === 'reasoning' ? 'reasoning' : 'message', segment });
  }
  return entries;
}

/**
 * Child steps for one delegation tool event, in call order. The first child
 * attaches to the delegation tool step itself; extra children from a parallel
 * `delegate` fan-out (and legacy orphans with no tool anchor) follow it as
 * standalone steps and are absorbed here — so every card renders inline at
 * its call site instead of being hoisted to the top.
 */
export function matchingDelegations(event: LiveToolEvent, steps: TraceStep[] | undefined, cursor: number): TraceStep[] {
  if (!steps || !isDelegationToolName(event.name)) return [];
  const attached = steps.filter((step) => step.childId && isDelegationToolName(step.toolName));
  const requestedName = delegationTarget(event.args).name.toLowerCase();
  const primary = attached.find((step) => requestedName && (step.childName || '').toLowerCase() === requestedName)
    || attached[cursor];
  if (!primary) {
    // No attached step (e.g. legacy trace) — fall back to the orphan pool in order.
    const orphans = steps.filter((step) => step.childId && !step.toolName);
    const fallback = orphans[cursor];
    return fallback ? [fallback] : [];
  }
  const result = [primary];
  // Absorb following orphans up to the next delegation call.
  const primaryIdx = steps.indexOf(primary);
  for (let i = primaryIdx + 1; i < steps.length; i++) {
    const next = steps[i];
    if (next.childId && !next.toolName) {
      result.push(next);
      continue;
    }
    if (isDelegationToolName(next.toolName)) break;
  }
  return result;
}

export function resolveDelegations(entries: ActivityEntry[], steps?: TraceStep[]): Map<string, TraceStep[]> {
  const resolved = new Map<string, TraceStep[]>();
  let cursor = 0;
  for (const entry of entries) {
    if (entry.kind !== 'tools') continue;
    for (const event of entry.events) {
      resolved.set(event.id, matchingDelegations(event, steps, cursor));
      if (isDelegationToolName(event.name)) cursor += 1;
    }
  }
  return resolved;
}

/**
 * Child steps that no timeline tool event claimed — the legacy fallback only.
 * Rendering these (and only these) outside the timeline guarantees a child
 * card is never shown twice and never hoisted above its call site.
 */
export function unmatchedDelegationSteps(segments: StreamSegment[], steps?: TraceStep[]): TraceStep[] {
  if (!steps?.length) return [];
  const resolved = resolveDelegations(buildEntries(segments), steps);
  const matched = new Set<string>();
  for (const group of resolved.values()) for (const step of group) matched.add(step.id);
  return steps.filter((step) => !!step.childId && !matched.has(step.id));
}
