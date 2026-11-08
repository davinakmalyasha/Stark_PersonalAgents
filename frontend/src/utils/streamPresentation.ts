import type { StreamSegment } from './streamAccum';
import type { TraceStep } from './logParser';

export interface StreamPresentation {
  /** Text that belongs in the normal assistant answer area. */
  finalText: string;
  /** Ordered reasoning, tool, and intermediate agent updates. */
  activitySegments: StreamSegment[];
}

function textOf(segments: StreamSegment[]): string {
  return segments
    .filter((segment): segment is Extract<StreamSegment, { type: 'text' }> => segment.type === 'text')
    .map((segment) => segment.text)
    .join('')
    .trim();
}

function containsText(segments: StreamSegment[], text: string): boolean {
  if (!text.trim()) return false;
  return segments
    .filter((segment): segment is Extract<StreamSegment, { type: 'text' }> => segment.type === 'text')
    .map((segment) => segment.text)
    .join('')
    .includes(text.trim());
}

/**
 * Old runs persisted the reconnect notice as answer text. It was transport
 * noise, not content, and stacked for every attempt. Strip it from history so
 * the transcript reads clean without rewriting stored messages.
 */
const RETRY_NOTICE_RE = /_{0,2}\s*Reconnecting\s*\(\s*\d+\s*\/\s*\d+\s*\)\s*[.…]*\s*_{0,2}/gi;

export function stripRetryNotices(text: string): string {
  if (!text || !/Reconnecting/i.test(text)) return text;
  return text
    .replace(RETRY_NOTICE_RE, '')
    .replace(/[ \t]+\n/g, '\n')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}

function cleanStreamSegments(segments: StreamSegment[]): StreamSegment[] {
  if (!segments.some((segment) => segment.type === 'text' && /Reconnecting/i.test(segment.text))) {
    return segments;
  }
  return segments
    .map((segment) =>
      segment.type === 'text'
        ? { ...segment, text: stripRetryNotices(segment.text) }
        : segment,
    )
    .filter((segment) => segment.type !== 'text' || segment.text.trim().length > 0);
}

/**
 * Some older runs persisted a single assistant string containing both the
 * handoff and the completion, for example:
 *
 *   Handed to General … ✓ Done — General says …
 *
 * Keep that data readable without pretending it was two backend messages.
 */
function splitHandoffText(text: string): { handoff: string; answer: string } | null {
  // Codex-style boundary: allow "Done —", "Completed: ..." etc.
  const match = text.match(/(?:✓\s*)?(?:done|completed)\b[^—–:\n]{0,30}[—–:-]/i);
  if (!match || typeof match.index !== 'number' || match.index <= 0) return null;

  const handoff = text.slice(0, match.index).trim();
  const answer = text.slice(match.index).trim();
  if (!handoff || !answer || !/(calling|asked|sent|handed|relay|answer|delegat|working|wait|job is out)/i.test(handoff)) return null;
  return { handoff, answer };
}

/**
 * Codex-lite sequence formatting (paragraph breaks, not inline).
 * The orchestrator often emits "Calling...✓ Asked...✓ Done..." as one
 * string with no newlines, which Markdown renders as a single <p>.
 * Insert blank lines at ✓ boundaries so each turn reads as its own
 * paragraph instead of a run-on sentence. Idempotent: never adds
 * breaks where a newline already exists.
 */
export function formatFinalSequence(text: string): string {
  if (!text || text.includes('\n\n')) {
    // Still fix inline ✓ joins even when some breaks exist.
    if (!text) return text;
  }
  let out = (text || '').trim();
  if (!out) return out;
  // "Asked.✓ Done" / "Working.✓ Done" -> blank line before ✓
  out = out.replace(/(?<!\n)\s*(✓\s*(?:asked|done|completed|handed|sent|relayed|working|waiting)\b)/gi, '\n\n$1');
  // Fallback when no ✓: "..., done. Done — ..." -> break before Done/Completed
  if (!/✓/.test(out)) {
    out = out.replace(/([.!?]["”']?\s+)(Done\b[^.!?]*?[—–:-])/g, '$1\n\n$2');
  }
  // Collapse 3+ newlines, trim.
  out = out.replace(/\n{3,}/g, '\n\n').trim();
  return out;
}

function splitLeadingHandoffStatus(text: string): { status: string; answer: string } | null {
  const lines = text.split(/\r?\n/);
  const statusLines: string[] = [];
  let cursor = 0;
  let sawStatus = false;
  while (cursor < lines.length) {
    const line = lines[cursor].trim();
    if (!line && sawStatus) {
      cursor += 1;
      continue;
    }
    if (!line || !/^(?:sent\b|handed\b|delegat(?:ed|ing)\b|the delegation\b|general\b.*(?:on it|will report|running)|i['’]ll pass\b|waiting\b|the job\b.*(?:running|handed|done))/i.test(line)) {
      break;
    }
    statusLines.push(line);
    sawStatus = true;
    cursor += 1;
  }
  const answer = lines.slice(cursor).join('\n').trim();
  if (!statusLines.length || !answer) return null;
  return { status: statusLines.join('\n'), answer };
}

/**
 * Turn the raw stream into the two visual lanes used by the chat:
 * chronological activity and the final assistant answer.
 *
 * The stream is authoritative when it contains a tool event. Text after the
 * last tool is the answer from the final model turn; text before it is an
 * intermediate update and stays in the activity feed.
 *
 * Text segments that duplicate the final answer are NEVER activity — they
 * would render the same answer twice (once inside the "Worked for" block,
 * once below it) and hide a copy of the answer when the block is closed.
 */
export function presentStream(segments: StreamSegment[], fallbackText: string, streaming = false): StreamPresentation {
  const safeSegments = cleanStreamSegments(Array.isArray(segments) ? segments : []);

  // While a run is live, render everything in strict chronological order:
  // reasoning, text, tools, text… Hoisting the last text into a separate
  // bottom block makes it look "stuck" when more reasoning arrives after it.
  if (streaming && safeSegments.length > 0) {
    return { finalText: '', activitySegments: safeSegments };
  }

  // Messages without a timeline (legacy persisted runs) only have the
  // flattened assistant string.
  let finalText = safeSegments.length === 0 ? stripRetryNotices((fallbackText || '').trim()) : '';
  let activitySegments = safeSegments;

  // The final answer is the *last contiguous text run*, and only when it is
  // actually at the end of the stream. Text followed by reasoning or a tool is
  // an intermediate update and stays in the activity lane.
  if (safeSegments.length > 0) {
    const last = safeSegments[safeSegments.length - 1];
    if (last.type === 'text') {
      let start = safeSegments.length - 1;
      while (start > 0 && safeSegments[start - 1].type === 'text') start -= 1;
      const trailing = textOf(safeSegments.slice(start));
      if (trailing) {
        finalText = trailing;
        activitySegments = safeSegments.slice(0, start);
      }
    }
  }

  const split = splitHandoffText(finalText);
  if (split) {
    finalText = split.answer;
    if (!containsText(activitySegments, split.handoff)) {
      const firstToolIndex = activitySegments.findIndex((segment) => segment.type === 'tool');
      const handoffSegment: StreamSegment = { type: 'text', text: split.handoff, agent: undefined };
      activitySegments = firstToolIndex < 0
        ? [...activitySegments, handoffSegment]
        : [...activitySegments.slice(0, firstToolIndex), handoffSegment, ...activitySegments.slice(firstToolIndex)];
  }

  // Older persisted runs sometimes put the pre-result handoff and the child
  // answer in one assistant message. Keep the handoff in chronological
  // activity, where it reads as a normal status update, instead of letting it
  // compete with the final response.
  const statusSplit = splitLeadingHandoffStatus(finalText);
  if (statusSplit) {
    finalText = statusSplit.answer;
    if (!containsText(activitySegments, statusSplit.status)) {
      activitySegments = [...activitySegments, { type: 'text', text: statusSplit.status }];
    }
  }
  }

  finalText = formatFinalSequence(stripRetryNotices(stripMailboxEnvelope(finalText)));
  const cleanedActivity = activitySegments.map((segment) =>
    segment.type === 'text'
      ? { ...segment, text: stripRetryNotices(stripMailboxEnvelope(segment.text)) }
      : segment,
  ).filter((segment) => segment.type !== 'text' || segment.text.trim().length > 0);
  return { finalText, activitySegments: stripFinalAnswerDupes(cleanedActivity, finalText) };
}

/** Collapse whitespace so stream/persisted copies of one answer compare equal. */
function normalizeForCompare(text: string): string {
  return (text || '').replace(/\s+/g, ' ').trim();
}

/**
 * Drop `text` segments that are just the final answer echoed in the stream.
 * Reasoning and tool events are always kept — only plain answer text is
 * deduplicated. Short status blurbs (<40 chars) are kept: they are transport
 * noise, not answer copies, even when a word happens to appear in the reply.
 */
function stripFinalAnswerDupes(segments: StreamSegment[], finalText: string): StreamSegment[] {
  const finalNorm = normalizeForCompare(finalText);
  if (!finalNorm) return segments;
  return segments.filter((segment) => {
    if (segment.type !== 'text') return true;
    const segNorm = normalizeForCompare(segment.text);
    if (segNorm.length < 40) return true;
    return !(finalNorm.includes(segNorm) || segNorm.includes(finalNorm));
  });
}

/**
 * Delegation announcements are transport/status text, not the child agent's
 * answer. Once a persisted trace has a child session, render those
 * announcements as clickable delegation cards instead of leaving sentences
 * such as "Created Confucius with the instructions…" in the parent answer.
 */
/**
 * Internal wake prompts are transport, never user content. The backend no
 * longer persists them, but legacy rows and other surfaces can still carry
 * them — hide them everywhere in the UI.
 */
export function isInternalTransport(text: string): boolean {
  const trimmed = (text || '').trimStart();
  return (
    trimmed.startsWith('[DELEGATION RESULT]')
    || trimmed.startsWith('[AGENT RESULT]')
    || trimmed.startsWith('[GROUP RESULT]')
  );
}

export function stripMailboxEnvelope(text: string): string {
  if (!text) return text;
  let cleaned = text;
  cleaned = cleaned.replace(/^\s*\[[^\]\n]*\sfrom\s+[^\]\n]+\]\s*$/gim, '');
  cleaned = cleaned.replace(/^\s*\[TASK\][^\n]*$/gim, '');
  cleaned = cleaned.replace(/\n{3,}/g, '\n\n').trim();
  return cleaned;
}

export function stripDelegationNarration(text: string, steps: TraceStep[]): string {
  if (!text) return text;
  let cleaned = stripMailboxEnvelope(text);
  if (!steps.some((step) => !!step.childId)) return cleaned;
  const linePatterns = [
    /(^|\n)\s*Created\s+an\s+agent[^\n]*(?:\n|$)/gi,
    /(^|\n)\s*Created\s+[^.\n]+?\s+with\s+(?:the\s+)?instructions?:[^\n]*(?:\n|$)/gi,
    /(^|\n)\s*Sub-?agent\s+spawning\s+is\s+available\.[^\n]*(?:\n|$)/gi,
    /(^|\n)\s*Spawned\s+\d+\s+sub-?agents?:[^\n]*(?:\n(?:\s*[-•]\s*[^\n]*)*)?/gi,
  ];
  for (const pattern of linePatterns) cleaned = cleaned.replace(pattern, '$1');

  // A pre-fix parent trace could contain the child's full arithmetic working
  // after its concise answer. Keep the answer paragraph in the parent and let
  // the clickable child card lead to the complete transcript.
  const detailMarker = cleaned.search(/(?:^|\n)\s*(?:worked breakdown|column check|units\s*:|split\s+\d+\s+into|```)/i);
  if (detailMarker > 0) {
    const concise = cleaned.slice(0, detailMarker).trim();
    if (concise) cleaned = concise;
  }
  return cleaned.replace(/\n{3,}/g, '\n\n').trim();
}
