import type { RunShape } from "./run.ts"

/** Host RPC bound to the current run. Throws `HostCallError` on failure. */
export type HostCall = (run: RunShape, name: string, args: unknown[]) => Promise<unknown>

export class HostCallError extends Error {
  readonly code: string | undefined

  constructor(message: string, code?: string) {
    super(message)
    this.name = "HostCallError"
    this.code = code
  }
}
