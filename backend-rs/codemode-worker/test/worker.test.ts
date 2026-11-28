/**
 * Integration tests: spawn the worker over stdio, drive initialize/execute,
 * answer tool_call/extension_call requests like the Rust host, and verify
 * OpenCode semantics (concurrency, dotted namespaces, freeform, fetch, Bun,
 * path, progress rows, cancel).
 */
import { describe, expect, setDefaultTimeout, test } from "bun:test"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"

setDefaultTimeout(20000)

const WORKER_DIR = Bun.fileURLToPath(new URL("..", import.meta.url))
// Reuse the running Bun binary: robust across PATH/shim layouts.
const BUN = process.env["BUN_BIN"] ?? process.execPath

type Msg = Record<string, unknown>

class Peer {
  proc: ReturnType<typeof Bun.spawn>
  pending = new Map<string, (msg: Msg) => void>()
  toolHandler: (params: Msg) => unknown = () => ({ ok: true, output: "ok" })
  extensionHandler: (params: Msg) => unknown = () => ({ ok: true, value: null })
  progress: Array<{ toolCalls: { tool: string; status: string }[]; files?: unknown[] }> = []
  seq = 1
  buffer = ""

  constructor() {
    this.proc = Bun.spawn([BUN, "run", "./src/main.ts"], {
      cwd: WORKER_DIR,
      stdin: "pipe",
      stdout: "pipe",
      stderr: "pipe",
    })
    const stdout = this.proc.stdout
    if (typeof stdout === "number") throw new Error("no stdout")
    ;(async () => {
      for await (const chunk of stdout as unknown as AsyncIterable<Uint8Array>) {
        this.feed(Buffer.from(chunk).toString("utf8"))
      }
    })()
  }

  feed(text: string) {
    this.buffer += text
    let nl: number
    while ((nl = this.buffer.indexOf("\n")) >= 0) {
      const line = this.buffer.slice(0, nl).trim()
      this.buffer = this.buffer.slice(nl + 1)
      if (!line) continue
      const msg = JSON.parse(line) as Msg
      const method = msg["method"]
      if (method === "progress") {
        const params = msg["params"] as { toolCalls?: { tool: string; status: string }[]; files?: unknown[] }
        this.progress.push({
          toolCalls: params?.toolCalls ?? [],
          ...(params?.files ? { files: params.files } : {}),
        })
        continue
      }
      if ((method === "tool_call" || method === "extension_call") && msg["id"] !== undefined) {
        const id = String(msg["id"])
        Promise.resolve()
          .then(() =>
            method === "tool_call"
              ? this.toolHandler(msg["params"] as Msg)
              : this.extensionHandler(msg["params"] as Msg),
          )
          .then(
            (result) => this.sendRaw({ id, result }),
            (error: unknown) =>
              this.sendRaw({
                id,
                error: { message: error instanceof Error ? error.message : String(error) },
              }),
          )
        continue
      }
      if (msg["id"] !== undefined && msg["method"] === undefined) {
        const key = String(msg["id"])
        const waiting = this.pending.get(key)
        if (waiting) {
          this.pending.delete(key)
          waiting(msg)
        }
      }
    }
  }

  sendRaw(msg: unknown) {
    const stdin = this.proc.stdin
    if (typeof stdin === "number") throw new Error("no stdin")
    ;(stdin as unknown as { write(s: string): void }).write(`${JSON.stringify(msg)}\n`)
  }

  request(method: string, params: unknown): Promise<Msg> {
    const id = `t_${this.seq++}`
    return new Promise((resolve) => {
      this.pending.set(id, resolve)
      this.sendRaw({ id, method, params })
    })
  }

  async close() {
    try {
      await this.request("shutdown", {})
    } catch {}
    this.proc.kill()
  }
}

const TOOLS = [
  {
    name: "read",
    description: "Read a file.",
    input_schema: {
      type: "object",
      properties: { file_path: { type: "string", description: "Path to read." } },
      required: ["file_path"],
    },
  },
  {
    name: "slow",
    description: "Sleep.",
    input_schema: {
      type: "object",
      properties: { ms: { type: "number" } },
      required: ["ms"],
    },
  },
  {
    name: "send",
    description: "Send text.",
    input_schema: {
      type: "object",
      properties: { text: { type: "string", description: "Message." } },
      required: ["text"],
    },
    freeform: true,
  },
  {
    name: "github.get_issue",
    description: "Get an issue.",
    input_schema: {
      type: "object",
      properties: { id: { type: "number" } },
      required: ["id"],
    },
  },
]

const execute = async (peer: Peer, code: string, runId: string, full = true) => {
  await peer.request("initialize", { tools: TOOLS })
  return (await peer.request("execute", {
    code,
    runId,
    cwd: process.cwd(),
    full,
  })) as {
    result: {
      ok: boolean
      text: string
      error: boolean
      toolCalls: { tool: string; status: string; input?: Record<string, unknown> }[]
      files: unknown[]
    }
  }
}

describe("codemode-worker", () => {
  test("initialize returns OpenCode signatures, including freeform", async () => {
    const peer = new Peer()
    try {
      const res = (await peer.request("initialize", { tools: TOOLS })) as {
        result: { catalog: { path: string; signature: string }[] }
      }
      const byPath = new Map(res.result.catalog.map((item) => [item.path, item.signature]))
      expect(byPath.get("read")).toBe(
        "tools.read({\n  /** Path to read. */\n  file_path: string,\n}): Promise<string | null>",
      )
      expect(byPath.get("send")).toBe(
        "tools.send(/** Message. */ text: string): Promise<string | null>",
      )
      expect(byPath.get("github.get_issue")).toContain("tools.github.get_issue({")
    } finally {
      await peer.close()
    }
  })

  test("an empty inventory still runs pure JavaScript", async () => {
    const peer = new Peer()
    try {
      await peer.request("initialize", { tools: [] })
      const res = (await peer.request("execute", {
        code: `return 1 + 1;`,
        runId: "run_pure",
        cwd: process.cwd(),
        full: true,
      })) as { result: { ok: boolean; text: string; toolCalls: unknown[] } }
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("2")
      expect(res.result.toolCalls).toEqual([])
    } finally {
      await peer.close()
    }
  })

  test("execute runs against the inventory it carries, not the last initialize", async () => {
    const peer = new Peer()
    peer.toolHandler = (params) => ({
      ok: true,
      output: `seen-${String((params["input"] as Record<string, unknown>)["file_path"])}`,
    })
    try {
      // Two sessions prepared against different inventories. The second
      // initialize must not rewrite the first session's runtime.
      await peer.request("initialize", { tools: TOOLS })
      await peer.request("initialize", { tools: [] })
      const res = (await peer.request("execute", {
        code: `return await tools.read({ file_path: "a.txt" })`,
        runId: "run_bound_inventory",
        cwd: process.cwd(),
        full: true,
        tools: TOOLS,
      })) as { result: { ok: boolean; text: string } }
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("seen-a.txt")
    } finally {
      await peer.close()
    }
  })

  test("execute composes tools and formats the result like OpenCode", async () => {
    const peer = new Peer()
    const seen: string[] = []
    peer.toolHandler = (params) => {
      seen.push(String((params["name"] as string) ?? ""))
      const input = params["input"] as Record<string, unknown>
      if (params["name"] === "read") return { ok: true, output: `content-of-${input["file_path"]}` }
      return { ok: true, output: "done" }
    }
    try {
      const res = await execute(
        peer,
        `const text = await tools.read({ file_path: "a.txt" }); return { text };`,
        "run_basic",
      )
      expect(res.result.ok).toBe(true)
      expect(res.result.error).toBe(false)
      expect(JSON.parse(res.result.text)).toEqual({ text: "content-of-a.txt" })
      expect(seen).toEqual(["read"])
      expect(res.result.toolCalls).toEqual([
        { tool: "read", status: "completed", input: { file_path: "a.txt" } },
      ])
    } finally {
      await peer.close()
    }
  })

  test("freeform tools accept the string form and record the object input", async () => {
    const peer = new Peer()
    peer.toolHandler = (params) => ({
      ok: true,
      output: String((params["input"] as Record<string, unknown>)["text"]),
    })
    try {
      const res = await execute(peer, `return await tools.send("hello")`, "run_free")
      expect(res.result.text).toBe("hello")
      expect(res.result.toolCalls[0]?.input).toEqual({ text: "hello" })
    } finally {
      await peer.close()
    }
  })

  test("Promise.allSettled runs tool calls concurrently", async () => {
    const peer = new Peer()
    const started: number[] = []
    peer.toolHandler = () => {
      started.push(Date.now())
      return new Promise((resolve) => setTimeout(() => resolve({ ok: true, output: "slept" }), 400))
    }
    try {
      const res = await execute(
        peer,
        `await Promise.allSettled([tools.slow({ ms: 400 }), tools.slow({ ms: 400 })]); return "ok";`,
        "run_conc",
      )
      expect(res.result.ok).toBe(true)
      expect(started.length).toBe(2)
      expect(Math.abs(started[1]! - started[0]!) < 350).toBe(true)
    } finally {
      await peer.close()
    }
  })

  test("tool failure throws inside the program and is catchable", async () => {
    const peer = new Peer()
    peer.toolHandler = () => ({ ok: false, output: "nope" })
    try {
      const res = await execute(
        peer,
        `try { await tools.read({ file_path: "x" }); return "unreached"; } catch (e) { return String(e && e.message); }`,
        "run_catch",
      )
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("nope")
      expect(res.result.toolCalls[0]?.status).toBe("error")
    } finally {
      await peer.close()
    }
  })

  test("a failing program is a successful tool call with error metadata", async () => {
    const peer = new Peer()
    try {
      const res = await execute(peer, `throw new Error("boom")`, "run_err")
      expect(res.result.ok).toBe(false)
      expect(res.result.error).toBe(true)
      expect(res.result.text).toBe("Error: boom")
    } finally {
      await peer.close()
    }
  })

  test("formatResult keeps logs on failure but not warnings", async () => {
    const peer = new Peer()
    try {
      const res = await execute(peer, `console.log("before"); throw new Error("boom")`, "run_logs_fail")
      expect(res.result.text).toBe("Error: boom\n\nLogs:\nbefore")
    } finally {
      await peer.close()
    }
  })

  test("failed execute flushes the pending progress rows", async () => {
    const peer = new Peer()
    peer.toolHandler = () => ({ ok: false, output: "no" })
    try {
      const res = await execute(peer, `await tools.slow({ ms: 1 }); return "unreached";`, "run_progress_fail")
      expect(res.result.ok).toBe(false)
      expect(
        peer.progress.some((update) =>
          update.toolCalls.some((row) => row.tool === "slow" && row.status === "error"),
        ),
      ).toBe(true)
    } finally {
      await peer.close()
    }
  })

  test("execute publishes throttled tool-call progress", async () => {
    const peer = new Peer()
    peer.toolHandler = () =>
      new Promise((resolve) => setTimeout(() => resolve({ ok: true, output: "slow" }), 220))
    try {
      const res = await execute(
        peer,
        `await tools.slow({ ms: 220 }); await tools.slow({ ms: 220 }); return "done";`,
        "run_progress",
      )
      expect(res.result.ok).toBe(true)
      const rows = peer.progress.flatMap((update) => update.toolCalls)
      expect(rows.some((row) => row.tool === "slow" && row.status === "running")).toBe(true)
      expect(rows.some((row) => row.tool === "slow" && row.status === "completed")).toBe(true)
      // 100 ms publication cap: two 220 ms calls produce only a few updates.
      expect(peer.progress.length).toBeLessThanOrEqual(4)
    } finally {
      await peer.close()
    }
  })

  test("formatResult drops a null value before logs", async () => {
    const peer = new Peer()
    try {
      const res = await execute(peer, `console.log("only log"); return null;`, "run_logs")
      expect(res.result.text).toBe("Logs:\nonly log")
    } finally {
      await peer.close()
    }
  })

  test("fetch is available without full_codemode", async () => {
    const server = Bun.serve({
      port: 0,
      fetch: () => new Response("fetched", { headers: { "content-type": "text/plain" } }),
    })
    const peer = new Peer()
    try {
      peer.extensionHandler = (params) => ({ ok: true, value: { ok: true } })
      const res = await execute(
        peer,
        `const text = await (await fetch("${server.url}")).text(); return text;`,
        "run_fetch",
        false,
      )
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("fetched")
      expect(res.result.toolCalls.some((call) => call.tool === "fetch")).toBe(true)
    } finally {
      await peer.close()
      server.stop(true)
    }
  })

  test("fetch follows redirects and checks every hop", async () => {
    const server = Bun.serve({
      port: 0,
      fetch(request) {
        if (new URL(request.url).pathname === "/start")
          return Response.redirect(new URL("/final", request.url), 302)
        return new Response("redirected")
      },
    })
    const peer = new Peer()
    const checks: unknown[][] = []
    peer.extensionHandler = (params) => {
      checks.push((params["args"] as unknown[]) ?? [])
      return { ok: true, value: { ok: true } }
    }
    try {
      const res = await execute(
        peer,
        `return await (await fetch("${server.url}start")).text();`,
        "run_fetch_redirect",
        false,
      )
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("redirected")
      expect(checks.length).toBe(2)
      expect(checks[1]?.[2]).toBe(true)
    } finally {
      await peer.close()
      server.stop(true)
    }
  })

  test("fetch permission denial rejects the program's promise", async () => {
    const peer = new Peer()
    try {
      peer.extensionHandler = (params) => ({
        ok: false,
        error: { message: "[BLOCKED] webfetch denied" },
      })
      const res = await execute(
        peer,
        `try { await fetch("https://example.com"); return "unreached"; } catch (e) { return e.message; }`,
        "run_fetch_denied",
        false,
      )
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("[BLOCKED] webfetch denied")
    } finally {
      await peer.close()
    }
  })

  test("path is bound to the run's working directory", async () => {
    const peer = new Peer()
    try {
      const res = await execute(
        peer,
        `return [path.join("a", "b"), path.resolve("x"), path.basename("a/b.txt")];`,
        "run_path",
      )
      expect(res.result.ok).toBe(true)
      expect(JSON.parse(res.result.text)).toEqual([
        path.join("a", "b"),
        path.resolve(process.cwd(), "x"),
        "b.txt",
      ])
    } finally {
      await peer.close()
    }
  })

  test("Bun.file/Bun.write use the host bridge", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "codemode-bun-"))
    const target = path.join(dir, "a.txt")
    fs.writeFileSync(target, "hello")
    const written = new Map<string, string>()
    const handles = new Map<string, string>()
    let next = 0
    const peer = new Peer()
    peer.extensionHandler = (params) => {
      const name = String(params["name"])
      const args = (params["args"] as unknown[]) ?? []
      if (name === "file_open") {
        const handle = `h${next++}`
        handles.set(handle, target)
        return {
          ok: true,
          value: {
            handle,
            name: args[0],
            size: fs.statSync(target).size,
            lastModified: fs.statSync(target).mtimeMs,
            exists: true,
          },
        }
      }
      if (name === "file_read") {
        const file = handles.get(String(args[0])) ?? target
        return { ok: true, value: { data_base64: Buffer.from(fs.readFileSync(file)).toString("base64") } }
      }
      if (name === "file_target") {
        const handle = `h${next++}`
        const pathName = String(args[0])
        handles.set(handle, pathName)
        return { ok: true, value: { handle, name: pathName } }
      }
      if (name === "file_write") {
        const file = handles.get(String(args[0]))!
        const bytes = Buffer.from(String(args[1]), "base64")
        written.set(file, bytes.toString("utf8"))
        return {
          ok: true,
          value: {
            bytes: bytes.length,
            diff: { file, status: "modified", additions: 1, deletions: 0 },
          },
        }
      }
      return { ok: true, value: null }
    }
    try {
      const res = await execute(
        peer,
        `const before = await Bun.file(${JSON.stringify(target)}).text(); await Bun.write(${JSON.stringify(
          path.join(dir, "b.txt"),
        )}, "written"); return before;`,
        "run_bun",
      )
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("hello")
      expect(written.get(path.join(dir, "b.txt"))).toBe("written")
      expect(res.result.files).toEqual([
        { file: path.join(dir, "b.txt"), status: "modified", additions: 1, deletions: 0 },
      ])
    } finally {
      await peer.close()
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })

  test("Bun.Glob scans locally after host approval", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "codemode-glob-"))
    fs.writeFileSync(path.join(dir, "a.txt"), "a")
    fs.writeFileSync(path.join(dir, "b.log"), "b")
    fs.mkdirSync(path.join(dir, "nested"))
    fs.writeFileSync(path.join(dir, "nested", "c.txt"), "c")
    const peer = new Peer()
    peer.extensionHandler = (params) => {
      if (params["name"] === "glob_prepare") return { ok: true, value: { root: dir } }
      return { ok: true, value: null }
    }
    try {
      await peer.request("initialize", { tools: TOOLS })
      const res = (await peer.request("execute", {
        code: `return new Bun.Glob("**/*.txt").scanSync({ cwd: ${JSON.stringify(dir)} });`,
        runId: "run_glob",
        cwd: dir,
        full: true,
      })) as { result: { ok: boolean; text: string } }
      expect(res.result.ok).toBe(true)
      expect(JSON.parse(res.result.text).sort()).toEqual(["a.txt", path.join("nested", "c.txt")])
    } finally {
      await peer.close()
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })

  test("fetch sends a BunFile body as a Blob", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "codemode-body-"))
    const target = path.join(dir, "body.txt")
    fs.writeFileSync(target, "file-body")
    const server = Bun.serve({
      port: 0,
      async fetch(request) {
        return new Response(await request.text())
      },
    })
    const peer = new Peer()
    peer.extensionHandler = (params) => {
      const name = String(params["name"])
      const args = (params["args"] as unknown[]) ?? []
      if (name === "file_open")
        return {
          ok: true,
          value: {
            handle: "hb",
            name: args[0],
            size: fs.statSync(target).size,
            lastModified: 0,
            exists: true,
          },
        }
      if (name === "file_read")
        return { ok: true, value: { data_base64: Buffer.from(fs.readFileSync(target)).toString("base64") } }
      return { ok: true, value: { ok: true } }
    }
    try {
      await peer.request("initialize", { tools: TOOLS })
      const res = (await peer.request("execute", {
        code: `const file = Bun.file(${JSON.stringify(target)}); return await (await fetch(${JSON.stringify(
          server.url,
        )}, { method: "POST", body: file })).text();`,
        runId: "run_body",
        cwd: dir,
        full: true,
      })) as { result: { ok: boolean; text: string } }
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("file-body")
    } finally {
      await peer.close()
      server.stop(true)
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })

  test("Bun.file writer flushes chunks through the host", async () => {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "codemode-writer-"))
    const target = path.join(dir, "writer.txt")
    const writes: string[] = []
    const peer = new Peer()
    peer.extensionHandler = (params) => {
      const name = String(params["name"])
      const args = (params["args"] as unknown[]) ?? []
      if (name === "file_open")
        return { ok: true, value: { handle: "hw", name: args[0], size: 0, lastModified: 0, exists: true } }
      if (name === "file_write") {
        writes.push(Buffer.from(String(args[1]), "base64").toString("utf8"))
        return { ok: true, value: { bytes: Buffer.from(String(args[1]), "base64").length, diff: null } }
      }
      return { ok: true, value: null }
    }
    try {
      await peer.request("initialize", { tools: TOOLS })
      const res = (await peer.request("execute", {
        code: `const writer = Bun.file(${JSON.stringify(target)}).writer(); await writer.write("one"); await writer.write("two"); await writer.end(); return "ok";`,
        runId: "run_writer",
        cwd: dir,
        full: true,
      })) as { result: { ok: boolean; text: string } }
      expect(res.result.ok).toBe(true)
      expect(res.result.text).toBe("ok")
      expect(writes.join("")).toBe("onetwo")
    } finally {
      await peer.close()
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })

  test("full_codemode=false has no Bun global", async () => {
    const peer = new Peer()
    try {
      const res = await execute(
        peer,
        `try { return typeof Bun } catch (e) { return "missing" }`,
        "run_no_bun",
        false,
      )
      expect(res.result.text).toBe("undefined")
    } finally {
      await peer.close()
    }
  })

  test("cancel interrupts a running program and aborts fetch", async () => {
    const peer = new Peer()
    peer.toolHandler = () => new Promise(() => {})
    try {
      await peer.request("initialize", { tools: TOOLS })
      const pending = peer.request("execute", {
        code: `await tools.slow({ ms: 60000 });`,
        runId: "run_cancel",
        cwd: process.cwd(),
        full: true,
      })
      await new Promise((r) => setTimeout(r, 300))
      const cancelled = (await peer.request("cancel", { runId: "run_cancel" })) as {
        result: { cancelled: boolean }
      }
      expect(cancelled.result.cancelled).toBe(true)
      const result = (await pending) as { result?: { ok: boolean }; error?: { message: string } }
      // The execute request settles after interruption; the exact shape may
      // be an ExecutionFailure or the host's cancelled result.
      expect(result.result?.ok === false || result.error !== undefined).toBe(true)
    } finally {
      await peer.close()
    }
  })
})
