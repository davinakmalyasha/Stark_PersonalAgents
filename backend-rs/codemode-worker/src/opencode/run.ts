/**
 * Per-execution Code Mode state and the exact OpenCode presentation layer:
 * progress rows, file-diff budget, `formatResult`, and `ExecuteOutput`.
 *
 * Ports `packages/core/src/codemode/{tool,diffs}.ts` (MIT, OpenCode authors).
 * The substitution point is the host: tool execution is an Effect over the
 * worker's `Run` service instead of OpenCode's `Run` service.
 */
import { Context, Effect } from "effect"
import type { ExecuteCall, ExecuteResult, FileChange } from "../protocol.ts"

export type RunShape = {
  readonly runId: string
  readonly cwd: string
  readonly rows: ExecuteCall[]
  readonly diffs: DiffCollector
  /** Aborts the run's in-flight `fetch` calls when Stop interrupts it. */
  readonly abort: AbortController
  /** Monotonic idempotency-key sequence for this run. */
  callSeq: number
  /** OpenCode progress throttling state. */
  lastPublished: number
  trailing: boolean
  timer: ReturnType<typeof setTimeout> | undefined
}

type ProgressSink = (run: RunShape) => void
let progressSink: ProgressSink | undefined

/** Installed by the worker's stdio loop. */
export function setProgressSink(sink: ProgressSink | undefined): void {
  progressSink = sink
}

/** OpenCode publishes at most one progress update per 100 ms, with a trailing update. */
export function scheduleProgress(run: RunShape): void {
  const wait = run.lastPublished + 100 - Date.now()
  if (wait <= 0) {
    run.lastPublished = Date.now()
    progressSink?.(run)
    return
  }
  if (run.trailing) return
  run.trailing = true
  run.timer = setTimeout(() => {
    run.trailing = false
    run.lastPublished = Date.now()
    progressSink?.(run)
  }, wait)
}

export function clearProgress(run: RunShape): void {
  if (run.timer !== undefined) clearTimeout(run.timer)
  run.timer = undefined
  run.trailing = false
}

/** OpenCode sends pending rows before a failed or interrupted call settles. */
export function flushProgress(run: RunShape): void {
  clearProgress(run)
  run.lastPublished = Date.now()
  progressSink?.(run)
}

export class Run extends Context.Service<Run, RunShape>()("@stark/CodeModeRun") {}

export type DiffCollector = ReturnType<typeof makeDiffs>

/** OpenCode `MAX_PATCH_BYTES` and `MAX_FILES`. */
const MAX_PATCH_BYTES = 200_000
const MAX_FILES = 50

export function makeDiffs() {
  const entries: FileChange[] = []
  let budget = MAX_PATCH_BYTES
  const record = (change: FileChange) => {
    if (entries.length >= MAX_FILES) return
    const { patch, ...counts } = change
    const size = patch === undefined ? 0 : Buffer.byteLength(patch)
    if (patch === undefined || size > budget) {
      entries.push(counts)
      return
    }
    budget -= size
    entries.push(change)
  }
  return {
    record,
    /** Records the diffs an inner edit/patch returned in `metadata.files`. */
    recordMetadata: (metadata: unknown) => {
      if (typeof metadata !== "object" || metadata === null) return
      const files = (metadata as Record<string, unknown>)["files"]
      if (!Array.isArray(files)) return
      for (const value of files) {
        if (typeof value !== "object" || value === null) continue
        const item = value as Record<string, unknown>
        if (typeof item["file"] !== "string") continue
        const status =
          item["status"] === "added" || item["status"] === "deleted" || item["status"] === "modified"
            ? item["status"]
            : "modified"
        record({
          file: item["file"],
          status,
          ...(typeof item["additions"] === "number" ? { additions: item["additions"] } : {}),
          ...(typeof item["deletions"] === "number" ? { deletions: item["deletions"] } : {}),
          ...(typeof item["patch"] === "string" ? { patch: item["patch"] } : {}),
        })
      }
    },
    files: () => entries.slice(),
  }
}

export function makeRun(runId: string, cwd: string): RunShape {
  return {
    runId,
    cwd,
    rows: [],
    diffs: makeDiffs(),
    abort: new AbortController(),
    callSeq: 0,
    lastPublished: Number.NEGATIVE_INFINITY,
    trailing: false,
    timer: undefined,
  }
}

type ExtensionDisplay = (
  extension: string,
  name: string,
  args: ReadonlyArray<unknown>,
) => { tool: string; input?: Record<string, unknown> } | undefined

/** OpenCode `progressHooks`: rows appear in start order and settle in place. */
export function progressHooks(display: ExtensionDisplay) {
  const rows = new WeakMap<object, number>()
  const start = (run: RunShape, call: object, entry: ExecuteCall) => {
    rows.set(call, run.rows.length)
    run.rows.push(entry)
  }
  const settle = (run: RunShape, call: object, result: { status: string }) => {
    const index = rows.get(call)
    if (index === undefined) return
    const current = run.rows[index]
    if (!current) return
    run.rows[index] = {
      ...current,
      status: result.status === "success" ? "completed" : "error",
    }
  }
  return {
    "tool.before": (call: { name: string; input: unknown }) =>
      Run.use((run) =>
        Effect.sync(() => {
          const shown = displayInput(call.input)
          start(run, call, {
            tool: call.name,
            status: "running",
            ...(shown ? { input: shown } : {}),
          })
          scheduleProgress(run)
        }),
      ),
    "tool.after": (call: object, result: { status: string }) =>
      Run.use((run) =>
        Effect.sync(() => {
          settle(run, call, result)
          scheduleProgress(run)
        }),
      ),
    "extension.before": (call: { extension: string; name: string; args: ReadonlyArray<unknown> }) =>
      Run.use((run) =>
        Effect.sync(() => {
          const shown = display(call.extension, call.name, call.args)
          if (!shown) return
          start(run, call, {
            tool: shown.tool,
            status: "running",
            ...(shown.input ? { input: shown.input } : {}),
          })
          scheduleProgress(run)
        }),
      ),
    "extension.after": (call: object, result: { status: string }) =>
      Run.use((run) =>
        Effect.sync(() => {
          settle(run, call, result)
          scheduleProgress(run)
        }),
      ),
  }
}

/** OpenCode `displayInput`: parsed JSON crosses back as itself. */
export function displayInput(input: unknown): Record<string, unknown> | undefined {
  if (input === null || input === undefined) return
  if (typeof input !== "object" || Array.isArray(input)) return { input: input as never }
  if (Object.keys(input).length === 0) return
  return input as Record<string, unknown>
}

export function finalize(
  run: RunShape,
  outcome: {
    ok: boolean
    value?: unknown
    error?: { message: string; suggestions?: ReadonlyArray<string> }
    logs?: ReadonlyArray<string>
    warnings?: ReadonlyArray<{ kind: string; message: string }>
  },
): ExecuteResult {
  if (outcome.ok) clearProgress(run)
  else flushProgress(run)
  const text = formatResult(outcome)
  const files = run.diffs.files()
  return {
    ok: outcome.ok,
    text,
    error: !outcome.ok,
    toolCalls: run.rows,
    files,
  }
}

/** OpenCode `formatResult`, byte-for-byte. */
function formatResult(result: {
  ok: boolean
  value?: unknown
  error?: { message: string; suggestions?: ReadonlyArray<string> }
  logs?: ReadonlyArray<string>
  warnings?: ReadonlyArray<{ kind: string; message: string }>
}): string {
  const output = result.ok
    ? formatValue(result.value)
    : [
        result.error?.message ?? "the program failed",
        ...(result.error?.suggestions ?? []).filter(
          (hint) => !(result.error?.message ?? "").includes(hint),
        ),
      ]
        .join("\n")
        .trim()
  const warnings =
    result.ok && result.warnings && result.warnings.length > 0
      ? `Warnings:\n${result.warnings.map((item) => `- [${item.kind}] ${item.message}`).join("\n")}`
      : undefined
  const logs =
    result.logs && result.logs.length > 0 ? `Logs:\n${result.logs.join("\n")}` : undefined
  // A program that only logs returns undefined (sent as null); a "null" line
  // before its logs says nothing.
  const value =
    result.ok && result.value === null && (warnings || logs) ? undefined : output
  return [value, warnings, logs].filter((part) => part !== undefined && part !== "").join("\n\n")
}

/** Compact JSON: indentation adds tokens on every later step without helping the model. */
function formatValue(value: unknown): string {
  if (typeof value === "string") return value
  return JSON.stringify(value) ?? String(value)
}
