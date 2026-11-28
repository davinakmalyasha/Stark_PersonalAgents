/**
 * OpenCode `CodeModeFetch.extension`: the `fetch` global.
 *
 * Port of `packages/core/src/codemode/fetch.ts` (MIT, OpenCode authors). The
 * permission service and `opencode://` API become a host RPC; the request
 * itself runs in the Bun sidecar with the real `Response` object.
 */
import { Effect } from "effect"
import { Extension } from "@opencode/codemode"
import { isRecord } from "./record.ts"
import { Run, type RunShape } from "./run.ts"
import type { HostCall } from "./host.ts"

const TIMEOUT_MS = 30_000
export const MAX_REDIRECTS = 20
const redirectStatuses = new Set([301, 302, 303, 307, 308])

type Init = {
  readonly method?: string
  readonly headers?: Record<string, string> | Array<[string, string]>
  readonly body?: string | Uint8Array<ArrayBuffer> | URLSearchParams | Blob
}

export type BunBody = {
  readonly body: (run: RunShape, value: unknown) => Promise<unknown>
}

const DISABLED =
  'opencode:// URLs are off. In a local or dev build, enable them with OPENCODE_FLAGS=opencode_api or "experimental": { "opencode_api": true }.'

export function makeFetch(call: HostCall, bun: BunBody) {
  const fetch = (input: string | URL, given: Init = {}) =>
    Run.use((run) =>
      Effect.tryPromise({
        try: () => execute(run, input, given),
        catch: (error) => error,
      }),
    )

  const execute = async (run: RunShape, input: string | URL, given: Init): Promise<unknown> => {
    // A BunFile reaches here as a plain copy, which `globalThis.fetch` would
    // send as "[object Object]", as it would any other object.
    const init: Init = { ...given, body: (await bun.body(run, given.body)) as Init["body"] }
    if (
      typeof init.body === "object" &&
      init.body !== null &&
      !(init.body instanceof Uint8Array) &&
      !(init.body instanceof URLSearchParams) &&
      !(init.body instanceof Blob)
    )
      throw new TypeError(
        "A fetch body must be a string, a Uint8Array, URLSearchParams, or a file from Bun.file().",
      )
    // Rules match the URL as written. The fragment is never sent, and would
    // keep a rule for the exact URL from matching.
    const url = String(input).split("#")[0]!
    const parsed = URL.parse(url)
    if (parsed?.protocol === "file:") throw new Error("fetch cannot be used for filesystem access.")
    // Rules match the URL, so `https://user@evil.com` would slip past a rule
    // for `https://evil.com/*`.
    if (parsed?.username || parsed?.password)
      throw new TypeError("fetch cannot be used with credentials in the URL.")
    if (parsed?.protocol === "opencode:") {
      if (!url.startsWith("opencode:///") || !parsed.pathname.startsWith("/api/"))
        throw new TypeError(
          `Invalid opencode URI "${url}": use opencode:///api/..., for example "opencode:///api/session".`,
        )
      throw new Error(DISABLED)
    }

    const method = (init.method ?? "GET").toUpperCase()
    await call(run, "fetch_check", [url, method, false])
    // Redirects are followed here so each new URL meets the same deny rules.
    const check = (next: string) => call(run, "fetch_check", [next, method, true])
    const signal = AbortSignal.any([run.abort.signal, AbortSignal.timeout(TIMEOUT_MS)])
    let target = url
    let request = init
    for (let redirects = 0; ; redirects++) {
      const response = await globalThis.fetch(target, { ...request, redirect: "manual", signal })
      const location = response.headers.get("location")
      if (!redirectStatuses.has(response.status) || location === null)
        return new Extension.StreamingResponse(response, redirects > 0)
      await response.body?.cancel().catch(() => {})
      if (redirects >= MAX_REDIRECTS)
        throw new Error(`Too many redirects (more than ${MAX_REDIRECTS})`)
      const next = new URL(location, target)
      if (next.protocol !== "http:" && next.protocol !== "https:")
        throw new Error(`fetch cannot follow a redirect to ${next.protocol} URLs.`)
      if (next.username || next.password)
        throw new Error("fetch cannot follow a redirect with credentials in the URL.")
      next.hash = ""
      await check(next.href)
      request = redirected(request, response.status, target, next)
      target = next.href
    }
  }

  return Extension.make({ name: "fetch", globals: { fetch: Extension.async(fetch) } })
}

// The request a browser sends after a redirect: 303, and 301/302 after a POST,
// become a GET without a body, and credentials are not sent to another origin.
const redirected = (request: Init, status: number, from: string, to: URL): Init => {
  const method = request.method?.toUpperCase() ?? "GET"
  const toGet = status === 303 ? method !== "HEAD" : (status === 301 || status === 302) && method === "POST"
  const headers = new Headers(request.headers)
  if (toGet)
    ["content-type", "content-length", "content-encoding", "content-language", "content-location"].forEach(
      (name) => headers.delete(name),
    )
  if (new URL(from).origin !== to.origin)
    ["authorization", "cookie", "proxy-authorization"].forEach((name) => headers.delete(name))
  return {
    method: toGet ? "GET" : request.method,
    headers: Array.from(headers),
    body: toGet ? undefined : request.body,
  }
}

/** What to show for a fetch call: its method and URL. */
export const display = (args: ReadonlyArray<unknown>) => {
  const [input, init] = args
  const method = isRecord(init) && typeof init.method === "string" ? init.method.toUpperCase() : "GET"
  return { method, url: String(input) }
}
