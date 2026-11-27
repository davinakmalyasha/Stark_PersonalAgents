/** JSON-RPC over stdio (line-delimited). Shared by worker and Rust client. */

export type RpcId = string | number

export interface RpcRequest {
  id: RpcId
  method: string
  params?: unknown
}

export interface RpcResponse {
  id: RpcId
  result?: unknown
  error?: { message: string; kind?: string; data?: unknown }
}

export interface RpcNotification {
  method: string
  params?: unknown
}

export type RpcMessage = RpcRequest | RpcResponse | RpcNotification

export function isResponse(msg: unknown): msg is RpcResponse {
  if (typeof msg !== "object" || msg === null) return false
  const value = msg as Record<string, unknown>
  return "id" in value && ("result" in value || "error" in value) && !("method" in value)
}

export function isRequest(msg: unknown): msg is RpcRequest {
  if (typeof msg !== "object" || msg === null) return false
  return "id" in msg && "method" in msg
}

export interface StarkToolDef {
  /** Dotted path as the program calls it, e.g. "read" or "github.get_issue". */
  name: string
  description: string
  /** JSON Schema (OpenCode render-only subset). */
  input_schema: Record<string, unknown>
  /** OpenCode `Tool.make({ freeform })`: one required string calls directly. */
  freeform?: boolean
}

export interface ExecuteParams {
  code: string
  runId: string
  /** Session working directory for `path` and relative Bun paths. */
  cwd: string
  /** OpenCode `full_codemode`: fetch + path + Bun extensions. */
  full: boolean
  /**
   * The inventory this call was prepared against. A worker serves every
   * session in the harness, so this call must not depend on whichever
   * `initialize` happened last. Omitted only by older callers/tests.
   */
  tools?: StarkToolDef[]
}

export interface ToolCallParams {
  runId: string
  key: string
  name: string
  input: unknown
}

export interface ToolCallResult {
  ok: boolean
  /** Model-facing text (tool stdout / file content / error message). */
  output: string
  metadata?: Record<string, unknown>
  /** Failure category for harness repair hints. */
  kind?: string
}

export interface ExtensionCallParams {
  runId: string
  name: string
  args: unknown[]
}

export interface ExtensionCallResult {
  ok: boolean
  value?: unknown
  error?: { message: string; code?: string }
}

export interface ExecuteCall {
  tool: string
  status: "running" | "completed" | "error"
  input?: Record<string, unknown>
}

export interface FileChange {
  file: string
  status: "added" | "deleted" | "modified"
  additions?: number
  deletions?: number
  patch?: string
}

/** What the worker returns for one execute. Mirrors OpenCode `ExecuteOutput`. */
export interface ExecuteResult {
  /** The program itself succeeded. */
  ok: boolean
  /** `formatResult(result)`, byte-identical to OpenCode. */
  text: string
  /** OpenCode `metadata.error` (`!result.ok`). */
  error: boolean
  toolCalls: ExecuteCall[]
  files: FileChange[]
}
