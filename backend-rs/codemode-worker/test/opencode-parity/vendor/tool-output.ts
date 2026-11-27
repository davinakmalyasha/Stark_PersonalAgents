// Verbatim subset of OpenCode v2.4.1 `packages/core/src/tool-output.ts`
// (MIT, © OpenCode authors), with the Effect/FS service replaced by a fixed
// spill path so the pure truncation decision can run in the differential
// fixture generator. The helpers and the single-text branch of `truncate` are
// copied from the source; no behavior is added.

export const MAX_LINES = 2_000
export const MAX_BYTES = 50 * 1024

export type Limits = { readonly maxLines: number; readonly maxBytes: number }

/** The model-visible text of one text part after OpenCode truncation. */
export function truncate(
  text: string,
  limits: Limits = { maxLines: MAX_LINES, maxBytes: MAX_BYTES },
  path = "tool-output/tool_FILE",
): { text: string; truncated: boolean } {
  const totalLines = countLines(text)
  if (totalLines <= limits.maxLines && new TextEncoder().encode(text).length <= limits.maxBytes)
    return { text, truncated: false }

  const end = Math.min(afterLines(text, limits.maxLines), byteBoundary(text, limits.maxBytes))
  const shown = text.slice(0, text.endsWith("\r\n", end) ? end - 2 : text.endsWith("\n", end) ? end - 1 : end)
  // The fixture's spill always succeeds.
  const failure = undefined
  const shownLines = countLines(text.slice(0, end))
  const lastLineCut =
    shownLines > 0 &&
    shown.length < text.length &&
    !text.startsWith("\n", shown.length) &&
    !text.startsWith("\r\n", shown.length)
  const range = shownLines === 0 ? "0 lines" : `lines 1-${shownLines}`
  const marker = `[showing ${range} of ${totalLines}${lastLineCut ? " (last line cut)" : ""}; ${
    failure === undefined ? `full output saved to ${path}` : `full output could not be saved: ${failure}`
  }]`
  // OpenCode returns [shown, marker] content parts; serializeToolContent
  // joins text parts with a newline.
  return { text: shown === "" ? marker : `${shown}\n${marker}`, truncated: true }
}

/** How many lines `text` has, not counting an empty one after a final line break. Counts without splitting. */
function countLines(text: string) {
  if (text === "") return 0
  let count = 1
  for (let index = text.indexOf("\n"); index !== -1 && index < text.length - 1; index = text.indexOf("\n", index + 1))
    count++
  return count
}

/** Where the line after the first `count` lines starts, or the end of `text`. */
function afterLines(text: string, count: number) {
  let index = 0
  for (let line = 0; line < count; line++) {
    const next = text.indexOf("\n", index)
    if (next === -1) return text.length
    index = next + 1
  }
  return index
}

const encoder = new TextEncoder()
const graphemes = new Intl.Segmenter(undefined, { granularity: "grapheme" })

/** The longest start of `text` that fits in `budget` UTF-8 bytes without splitting a character, emoji included. */
function byteBoundary(text: string, budget: number) {
  // encodeInto only encodes what fits in the buffer, and stops at a whole code point.
  const index = encoder.encodeInto(text, new Uint8Array(Math.max(0, budget))).read
  if (index === text.length || index === 0) return index
  const before = text.charCodeAt(index - 1)
  const after = text.charCodeAt(index)
  // Two ASCII characters always have a boundary between them, except the two halves of \r\n.
  if (before < 0x80 && after < 0x80) return before === 13 && after === 10 ? index - 1 : index
  // Otherwise back off to the start of the grapheme the cut is in. A line break always ends one, so segmenting from the
  // start of the line keeps flag pairs and long emoji sequences exact; one code point past the cut gives the context.
  const start = text.lastIndexOf("\n", index - 1) + 1
  return start + (graphemes.segment(text.slice(start, index + 2)).containing(index - start)?.index ?? index - start)
}
