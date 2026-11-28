/**
 * Stark Code Mode worker: OpenCode v2 Code Mode runtime on psychoscript.
 *
 * Speaks line-delimited JSON-RPC over stdio (see protocol.ts). The Rust host
 * owns tool dispatch, permissions, and the filesystem/network boundary; this
 * process only runs the confined JS program and the OpenCode fetch/path/Bun
 * extension surface.
 *
 * MIT attribution: runtime shape and result formatting follow
 * `@opencode/codemode` and `packages/core/src/codemode` (MIT).
 */
import { Effect, Fiber } from "effect"
import { CodeMode, Tool } from "@opencode/codemode"
import type {
  ExecuteParams,
  ExecuteResult,
  ExtensionCallResult,
  RpcId,
  StarkToolDef,
  ToolCallResult,
} from "./protocol.ts"
import {
  clearProgress,
  finalize,
  flushProgress,
  makeRun,
  progressHooks,
  Run,
  scheduleProgress,
  setProgressSink,
  type RunShape,
} from "./opencode/run.ts"
import { makeFetch } from "./opencode/fetch.ts"
import { makeBun } from "./opencode/bun.ts"
import { extension as pathExtension } from "./opencode/path.ts"
import { HostCallError } from "./opencode/host.ts"

// ---------------------------------------------------------------------------
// RPC transport
// ---------------------------------------------------------------------------

type Pending = {
  resolve: (value: unknown) => void
  reject: (error: Error) => void
}

const pending = new Map<string, Pending>()
let rpcSeq = 1

function writeMessage(msg: unknown): void {
  process.stdout.write(JSON.stringify(msg) + "\n")
}

function nextId(): string {
  return `w_${rpcSeq++}`
}

function callRust(method: string, params: unknown): Promise<unknown> {
  const id = nextId()
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    writeMessage({ id, method, params })
  })
}

function respond(id: RpcId, result: unknown): void {
  writeMessage({ id, result })
}

function respondError(id: RpcId, message: string, kind = "Transport"): void {
  writeMessage({ id, error: { message, kind } })
}

function notify(method: string, params: unknown): void {
  writeMessage({ method, params })
}

// ---------------------------------------------------------------------------
// Extensions and host bridge
// ---------------------------------------------------------------------------

const callHost = async (run: RunShape, name: string, args: unknown[]): Promise<unknown> => {
  const raw = (await callRust("extension_call", {
    runId: run.runId,
    name,
    args,
  })) as ExtensionCallResult
  if (!raw.ok) {
    throw new HostCallError(
      raw.error?.message ?? `Code Mode host function ${name} failed`,
      raw.error?.code,
    )
  }
  return raw.value
}

const bun = makeBun(callHost)
const fetchExtension = makeFetch(callHost, bun)

const extensionDisplay = (
  extension: string,
  name: string,
  args: ReadonlyArray<unknown>,
) => {
  if (extension === "fetch") {
    const [input, init] = args
    const method =
      typeof init === "object" && init !== null && typeof (init as Record<string, unknown>)["method"] === "string"
        ? String((init as Record<string, unknown>)["method"]).toUpperCase()
        : "GET"
    return { tool: "fetch", input: { method, url: String(input) } }
  }
  if (extension === "bun") return { tool: "Bun", input: bun.display(name, args) }
  return undefined
}

const extensions = [fetchExtension, pathExtension, bun.extension] as const

setProgressSink((run) => {
  const files = run.diffs.files()
  notify("progress", {
    runId: run.runId,
    toolCalls: run.rows,
    ...(files.length > 0 ? { files } : {}),
  })
})

// ---------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------

let tools: StarkToolDef[] = []
type Runtime = ReturnType<typeof CodeMode.make>
// OpenCode caches one runtime per inventory. This worker serves every session
// in the harness, so it must key the cache the same way instead of keeping a
// mutable "last initialized" runtime. A small bound keeps two agents with
// different inventories from pinning an unbounded set of language runtimes.
const runtimeCache = new Map<string, Runtime>()
const RUNTIME_CACHE_LIMIT = 4
const liveRuns = new Map<string, { run: RunShape; fiber?: any }>()

function toolDefsKey(defs: StarkToolDef[]): string {
  return JSON.stringify(defs)
}

function ensureRuntime(defs: StarkToolDef[]): Runtime {
  const key = toolDefsKey(defs)
  const cached = runtimeCache.get(key)
  if (cached !== undefined) return cached
  const provided: Record<string, ReturnType<typeof Tool.make>> = {}
  for (const def of defs) {
    const execute = (input: unknown) =>
      Run.use((run) =>
        Effect.tryPromise({
          try: async () => {
            const key = `${run.runId}:${++run.callSeq}`
            const raw = (await callRust("tool_call", {
              runId: run.runId,
              key,
              name: def.name,
              input: input ?? {},
            })) as ToolCallResult
            if (raw.ok) {
              // OpenCode `CodeModeDiffs.changes`: only edit/patch results
              // carry file changes; other metadata file lists are ignored.
              if (def.name === "edit" || def.name === "apply_patch") {
                run.diffs.recordMetadata(raw.metadata)
              }
              scheduleProgress(run)
              return raw.output
            }
            throw new Error(raw.output || `tool ${def.name} failed`)
          },
          catch: (error) => error,
        }),
      )
    const made = Tool.make({
      description: def.description,
      input: def.input_schema as never,
      output: { type: ["string", "null"] } as never,
      freeform: def.freeform === true,
      execute: execute as never,
    })
    provided[def.name] = made as never
  }
  // Group dotted names ("github.get_issue") into nested namespaces so the
  // program calls tools.github.get_issue({...}) exactly like OpenCode.
  const nested: Record<string, unknown> = {}
  for (const [path, tool] of Object.entries(provided)) {
    const parts = path.split(".")
    let node: Record<string, unknown> = nested
    for (let i = 0; i < parts.length - 1; i++) {
      const part = parts[i]!
      if (typeof node[part] !== "object" || node[part] === null) node[part] = {}
      node = node[part] as Record<string, unknown>
    }
    node[parts[parts.length - 1]!] = tool
  }
  const runtime = CodeMode.make({
    tools: nested as never,
    extensions: extensions as never,
    hooks: progressHooks(extensionDisplay) as never,
  })
  runtimeCache.set(key, runtime)
  if (runtimeCache.size > RUNTIME_CACHE_LIMIT) {
    const oldest = runtimeCache.keys().next().value
    if (oldest !== undefined && oldest !== key) runtimeCache.delete(oldest)
  }
  return runtime
}

// ---------------------------------------------------------------------------
// Execute / cancel
// ---------------------------------------------------------------------------

async function runExecute(params: ExecuteParams): Promise<ExecuteResult> {
  // Use the inventory carried by this call. Falling back to `tools` keeps the
  // old protocol usable for tests and ad-hoc peers, but a real execute always
  // binds its own prepared inventory.
  const rt = ensureRuntime(params.tools ?? tools)
  const run = makeRun(params.runId, params.cwd)
  const live: { run: RunShape; fiber?: any } = { run }
  liveRuns.set(params.runId, live)
  try {
    const program = rt.program(params.code, undefined, {
      runId: params.runId,
      ...(params.full ? {} : { extensions: [fetchExtension] }),
    } as never)
    const fiber = Effect.runFork(
      (program.run as () => Effect.Effect<unknown>)().pipe(Effect.provideService(Run, run)),
    )
    live.fiber = fiber
    const outcome = (await Effect.runPromise(Fiber.join(fiber))) as any
    if (outcome !== null && typeof outcome === "object" && outcome.ok === "suspended") {
      flushProgress(run)
      return {
        ok: false,
        text: "The program suspended, which isn't supported here.",
        error: true,
        toolCalls: run.rows,
        files: run.diffs.files(),
      }
    }
    return finalize(run, outcome)
  } catch (error) {
    flushProgress(run)
    const message = error instanceof Error ? error.message : String(error)
    return {
      ok: false,
      text: message,
      error: true,
      toolCalls: run.rows,
      files: run.diffs.files(),
    }
  } finally {
    liveRuns.delete(params.runId)
  }
}

async function runCancel(runId: string): Promise<boolean> {
  const live = liveRuns.get(runId)
  if (!live) return false
  live.run.abort.abort()
  if (live.fiber) await Effect.runPromise(Fiber.interrupt(live.fiber))
  return true
}

// ---------------------------------------------------------------------------
// Request dispatch (Rust -> Worker)
// ---------------------------------------------------------------------------

async function handleRequest(id: RpcId, method: string, params: unknown): Promise<void> {
  try {
    switch (method) {
      case "initialize": {
        const p = params as { tools?: StarkToolDef[] }
        tools = p.tools ?? []
        const rt = ensureRuntime(tools)
        respond(id, {
          catalog: rt.catalog.map((item) => ({
            path: item.path,
            description: item.description,
            signature: item.signature,
          })),
        })
        return
      }
      case "execute": {
        const p = params as ExecuteParams
        if (!p.code || !p.runId) throw new Error("execute requires code and runId")
        respond(id, await runExecute(p))
        return
      }
      case "cancel": {
        const p = params as { runId?: string }
        if (!p.runId) throw new Error("cancel requires runId")
        respond(id, { cancelled: await runCancel(p.runId) })
        return
      }
      case "shutdown": {
        respond(id, { ok: true })
        await new Promise((r) => setTimeout(r, 10))
        process.exit(0)
        return
      }
      default:
        respondError(id, `unknown method: ${method}`, "Args")
    }
  } catch (error) {
    respondError(id, error instanceof Error ? error.message : String(error))
  }
}

// ---------------------------------------------------------------------------
// Main loop
// ---------------------------------------------------------------------------

async function main(): Promise<void> {
  notify("ready", { pid: process.pid })
  process.stdin.setEncoding("utf8")
  let buffer = ""
  for await (const chunk of process.stdin as unknown as AsyncIterable<string>) {
    buffer += chunk
    let nl: number
    while ((nl = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, nl).trim()
      buffer = buffer.slice(nl + 1)
      if (!line) continue
      let msg: Record<string, unknown>
      try {
        msg = JSON.parse(line) as Record<string, unknown>
      } catch {
        continue
      }
      // Response to a worker->Rust call (tool_call / extension_call).
      if (("result" in msg || "error" in msg) && !("method" in msg)) {
        const key = String(msg["id"])
        const entry = pending.get(key)
        if (!entry) continue
        pending.delete(key)
        if ("error" in msg && msg["error"] !== undefined) {
          const err = msg["error"] as { message?: string }
          entry.reject(new Error(typeof err.message === "string" ? err.message : "host call failed"))
        } else {
          entry.resolve(msg["result"])
        }
        continue
      }
      // Request from Rust.
      if ("method" in msg && "id" in msg) {
        const reqId = msg["id"] as RpcId
        const method = String(msg["method"])
        // Don't block the read loop: each request runs independently.
        void handleRequest(reqId, method, msg["params"])
        continue
      }
    }
  }
}

if (import.meta.main) {
  main().catch((error) => {
    process.stderr.write(
      `codemode-worker fatal: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`,
    )
    process.exit(1)
  })
}
