/**
 * OpenCode `CodeModeBun.extension`: a subset of Bun's file APIs over the
 * session's files and permissions.
 *
 * Port of `packages/core/src/codemode/bun.ts` (MIT, OpenCode authors). The
 * FileAccess/FileWrite/FileMutation/Environment services become host RPCs, so
 * Stark's plan/read-only guards, external-directory approval, edit permission,
 * and diff metadata stay authoritative.
 */
import { Effect } from "effect"
import { Extension } from "@opencode/codemode"
import fs from "node:fs"
import { constants } from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { minimatch } from "minimatch"
import { Run, scheduleProgress, type RunShape } from "./run.ts"
import { HostCallError, type HostCall } from "./host.ts"

type Range = { readonly offset: number; readonly length: number }

// What a BunFile names: the path as the program wrote it, the opaque host
// handle, and the part of the file a slice covers.
type Handle = {
  readonly handle: string
  readonly name: string
  readonly size: number
  readonly lastModified: number
  readonly range?: Range
  readonly type?: string
}

// A BunFile reaches a program as a copy and comes back as another, but its
// functions come back as themselves: its `bytes` function says which file a
// value passed to `Bun.write` or `fetch` is, and its type.
const handles = new WeakMap<Function, Handle & { readonly type: string }>()

type HostFile = {
  handle: string
  name: string
  size: number
  lastModified: number
  exists: boolean
}

type HostDiff = {
  file: string
  status: "added" | "deleted" | "modified"
  additions?: number
  deletions?: number
  patch?: string
}

const messages: Record<string, string> = {
  ENOENT: "no such file or directory",
  EISDIR: "illegal operation on a directory",
  ENOTDIR: "not a directory",
}

function nodeError(error: HostCallError, syscall: string, file: string): Error {
  if (error.code) {
    const code = error.code
    return Object.assign(new Error(`${code}: ${messages[code] ?? error.message}, ${syscall} '${file}'`), {
      code,
      errno: -(constants.errno as Record<string, number>)[code]!,
      syscall,
      path: file,
    })
  }
  return error
}

async function callFile(
  call: HostCall,
  run: RunShape,
  name: string,
  args: unknown[],
  syscall: string,
  file: string,
): Promise<any> {
  try {
    return await call(run, name, args)
  } catch (error) {
    if (error instanceof HostCallError) throw nodeError(error, syscall, file)
    throw error
  }
}

const callEffect = (
  call: HostCall,
  run: RunShape,
  name: string,
  args: unknown[],
  syscall: string,
  file: string,
) =>
  Effect.tryPromise({
    try: () => callFile(call, run, name, args, syscall, file),
    catch: (error) => error,
  })

const later =
  <Args extends ReadonlyArray<unknown>, A>(run: (...args: Args) => Promise<A>) =>
  (...args: Args) =>
    Effect.tryPromise({
      try: () => run(...args),
      catch: (error) => error,
    })

export function makeBun(call: HostCall) {
  const read = async (run: RunShape, handle: Handle): Promise<Uint8Array> => {
    const result = (await callFile(
      call,
      run,
      "file_read",
      [handle.handle, handle.range?.offset, handle.range?.length],
      "open",
      handle.name,
    )) as { data_base64: string }
    return new Uint8Array(Buffer.from(result.data_base64, "base64"))
  }

  const exists = async (run: RunShape, handle: Handle): Promise<boolean> => {
    const result = (await callFile(
      call,
      run,
      "file_exists",
      [handle.handle],
      "access",
      handle.name,
    )) as { exists: boolean }
    return result.exists
  }

  const stat = async (run: RunShape, handle: Handle) => {
    const info = (await callFile(
      call,
      run,
      "file_stat",
      [handle.handle],
      "stat",
      handle.name,
    )) as { size: number; mtimeMs: number; type: string }
    return {
      size: info.size,
      mtimeMs: info.mtimeMs,
      mtime: new Date(info.mtimeMs),
      isFile: () => info.type === "file",
      isDirectory: () => info.type === "directory",
      isSymbolicLink: () => info.type === "symlink",
    }
  }

  const remove = async (run: RunShape, handle: Handle): Promise<void> => {
    const result = (await callFile(
      call,
      run,
      "file_unlink",
      [handle.handle],
      "unlink",
      handle.name,
    )) as { diff?: HostDiff }
    if (result.diff) {
      run.diffs.record(result.diff)
      scheduleProgress(run)
    }
  }

  const put = async (
    run: RunShape,
    to: Handle,
    bytes: Uint8Array,
    options: { readonly parents?: boolean; readonly diff?: boolean; readonly append?: boolean },
  ): Promise<number> => {
    const result = (await callFile(
      call,
      run,
      "file_write",
      [
        to.handle,
        Buffer.from(bytes).toString("base64"),
        options.parents ?? true,
        options.append ?? false,
        options.diff ?? true,
      ],
      "open",
      to.name,
    )) as { bytes: number; diff?: HostDiff | null }
    if (result.diff) {
      run.diffs.record(result.diff)
      scheduleProgress(run)
    }
    return result.bytes
  }

  const contents = async (run: RunShape, data: unknown): Promise<Uint8Array> => {
    const from = handleOf(data)
    if (from) return read(run, from)
    const given = toBytes(data)
    if (given) return given
    const body = method(data, "bytes") ?? method(data, "arrayBuffer") ?? method(data, "text")
    if (!body) throw unwritable()
    const result = await body()
    const bytes = toBytes(result)
    if (!bytes) throw unwritable()
    return bytes
  }

  const write = async (
    run: RunShape,
    to: Handle,
    data: unknown,
    parents = true,
  ): Promise<number> => {
    const from = handleOf(data)
    const bytes = from ? await read(run, from) : await contents(run, data)
    return put(run, to, bytes, { parents, diff: from === undefined })
  }

  const file = (run: RunShape, handle: Handle, type: string) => {
    const bytes = later(async () => {
      return read(run, handle)
    })
    handles.set(bytes, { ...handle, type })
    const unlink = later(() => remove(run, handle))
    // As Blob.slice: a negative index counts from the end, and a string in any
    // position is the new type.
    const index = (value: number | string | undefined, fallback: number) => {
      if (typeof value !== "number") return fallback
      return value < 0 ? Math.max(handle.size + value, 0) : Math.min(value, handle.size)
    }
    return {
      name: handle.name,
      type,
      size: handle.size,
      lastModified: handle.lastModified,
      text: later(async () => decode(await read(run, handle))),
      json: later(async () => JSON.parse(decode(await read(run, handle)))),
      bytes,
      // The runtime has no ArrayBuffer: one crosses to a program as a Uint8Array.
      arrayBuffer: later(() => read(run, handle)),
      exists: later(() => exists(run, handle)),
      stat: later(() => stat(run, handle)),
      slice: (...args: ReadonlyArray<number | string | undefined>): unknown => {
        const start = index(args[0], 0)
        const length = Math.max(index(args[1], handle.size) - start, 0)
        return file(
          run,
          {
            ...handle,
            range: { offset: (handle.range?.offset ?? 0) + start, length },
            size: length,
          },
          args.find((item) => typeof item === "string") ?? type,
        )
      },
      write: later((data: unknown) => write(run, handle, data)),
      delete: unlink,
      unlink,
      writer: (options?: { readonly highWaterMark?: number }) => writer(run, handle, options),
    }
  }

  // A subset of Bun's FileSink. Chunks collect here until a flush: the first
  // replaces the file and later ones append to it, so a long write asks edit
  // once per flush, with what that flush adds, not once per chunk.
  const writer = (run: RunShape, handle: Handle, options?: { readonly highWaterMark?: number }) => {
    supports("A file writer", options, ["highWaterMark"])
    const chunks: Uint8Array[] = []
    let buffered = 0
    let started = false
    const flushAsync = async (): Promise<number> => {
      if (chunks.length === 0) return 0
      const bytes = Buffer.concat(chunks)
      const written = await put(run, handle, bytes, { diff: true, append: started })
      started = true
      chunks.length = 0
      buffered = 0
      return written
    }
    const flush = () =>
      Effect.tryPromise({
        try: () => flushAsync(),
        catch: (error) => error,
      })
    return {
      write: (chunk: unknown) => {
        const bytes = typeof chunk === "string" || chunk instanceof Uint8Array ? toBytes(chunk) : undefined
        if (!bytes) throw new TypeError("A file writer takes a string or a Uint8Array.")
        chunks.push(bytes)
        buffered += bytes.byteLength
        if (buffered < (options?.highWaterMark ?? Infinity)) return bytes.byteLength
        return Effect.map(flush(), () => bytes.byteLength)
      },
      flush,
      end: flush,
    }
  }

  const open = (input: string | URL, options?: { readonly type?: string }) =>
    Effect.gen(function* () {
      supports("Bun.file", options, ["type"])
      const run = yield* Run
      const name = toPath(input)
      const opened = (yield* callEffect(call, run, "file_open", [name], "stat", name)) as HostFile
      return file(
        run,
        {
          handle: opened.handle,
          name,
          size: opened.size,
          lastModified: opened.lastModified,
        },
        options?.type ?? mime(name),
      )
    })

  const writeTo = (destination: unknown, data: unknown, options?: { readonly createPath?: boolean }) =>
    Effect.gen(function* () {
      supports("Bun.write", options, ["createPath"])
      const run = yield* Run
      let to: Handle | undefined
      if (typeof destination === "string" || destination instanceof URL) {
        const name = toPath(destination)
        const target = (yield* callEffect(call, run, "file_target", [name], "open", name)) as {
          handle: string
          name: string
        }
        to = { handle: target.handle, name, size: 0, lastModified: 0 }
      } else {
        to = handleOf(destination)
      }
      if (to === undefined) throw new TypeError("Bun.write expects a path or a BunFile to write to.")
      const bytes = (yield* Effect.tryPromise({
        try: () => contents(run, data),
        catch: (error) => error,
      })) as Uint8Array
      yield* Effect.tryPromise({
        try: () =>
          put(run, to!, bytes, {
            parents: options?.createPath !== false,
            diff: handleOf(data) === undefined,
          }),
        catch: (error) => error,
      })
      return bytes.byteLength
    })

  const scan = (pattern: string, input?: string | ScanOptions) =>
    Effect.gen(function* () {
      const run = yield* Run
      const options = typeof input === "string" ? { cwd: input } : input
      const given = options?.cwd ?? "."
      const cwd = path.isAbsolute(given) ? given : path.resolve(run.cwd, given)
      const base = globBase(pattern, cwd)
      for (const target of new Set([cwd, base]))
        yield* callEffect(call, run, "glob_prepare", [target], "scandir", target)
      return scanLocal(pattern, options, run.cwd)
    })

  const makeGlob = (pattern: string) =>
    Effect.gen(function* () {
      if (typeof pattern !== "string") throw new TypeError("Bun.Glob expects a pattern string.")
      const run = yield* Run
      const roots: string[] = []
      for (const target of new Set([run.cwd, globBase(pattern, run.cwd)])) {
        const prepared = (yield* callEffect(call, run, "glob_prepare", [target], "scandir", target)) as {
          root: string
        }
        roots.push(prepared.root)
      }
      const approved = (target: string) => roots.some((root) => within(root, target))
      return {
        scan: (options?: string | ScanOptions) => scan(pattern, options),
        scanSync: (options?: string | ScanOptions) => {
          const opts = typeof options === "string" ? { cwd: options } : options
          const given = opts?.cwd ?? run.cwd
          const cwd = path.isAbsolute(given) ? given : path.resolve(run.cwd, given)
          const base = globBase(pattern, cwd)
          if (!approved(cwd) || !approved(base))
            throw new TypeError(
              `Bun.Glob scanSync cannot read ${cwd} without approval; use scan() for another directory.`,
            )
          return scanLocal(pattern, opts, run.cwd)
        },
        // As in Bun, `match` takes a dot file that a scan without `dot` leaves out.
        match: (name: string) => minimatch(name, pattern, { dot: true }),
      }
    })

  const extension = Extension.make({
    name: "bun",
    modules: { bun: "Bun" },
    globals: {
      Bun: {
        file: open,
        write: Extension.async(writeTo),
        Glob: Extension.constructible(makeGlob),
      },
    },
  })

  /** A BunFile given as a `fetch` body, read to a Blob of its type. */
  const body = async (run: RunShape, value: unknown): Promise<unknown> => {
    const from = handleOf(value)
    if (!from) return value
    const bytes = await read(run, from)
    return new Blob([new Uint8Array(bytes)], { type: from.type })
  }

  return {
    extension,
    body,
    display: (name: string, args: ReadonlyArray<unknown>) => {
      const from = name === "Bun.write" ? handleOf(args[1])?.name : undefined
      return {
        method: name.slice(name.lastIndexOf(".") + 1),
        path: handleOf(args[0])?.name ?? String(args[0]),
        ...(from === undefined ? {} : { from }),
      }
    },
  }
}

const handleOf = (value: unknown) => {
  const bytes = method(value, "bytes")
  return bytes && handles.get(bytes)
}

const unwritable = () =>
  new TypeError("Bun.write expects a string, a Uint8Array, a BunFile, or a Response as the data to write.")

// A value from a program is a plain copy, so its functions are among its own entries.
const method = (value: unknown, key: string) => {
  if (typeof value !== "object" || value === null) return undefined
  const found: unknown = Object.entries(value).find(([name]) => name === key)?.[1]
  return typeof found === "function" ? found : undefined
}

// Like Bun, a number or a boolean is written as its text.
const toBytes = (value: unknown): Uint8Array | undefined => {
  if (typeof value === "string") return new Uint8Array(Buffer.from(value))
  if (typeof value === "number" || typeof value === "boolean") return new Uint8Array(Buffer.from(String(value)))
  if (value instanceof Uint8Array) return value
  if (value instanceof ArrayBuffer) return new Uint8Array(value)
}

// As `text()` does in Bun: UTF-8 without a leading byte order mark.
const decode = (bytes: Uint8Array) => new TextDecoder().decode(bytes)

type ScanOptions = {
  readonly cwd?: string
  readonly dot?: boolean
  readonly absolute?: boolean
  readonly onlyFiles?: boolean
}

// `scan` and `scanSync` both give an array, which `for` and `for await` both
// take. As in Bun without `followSymlinks`, a symlink is neither a file nor a
// directory to look inside: only `onlyFiles: false` lists one.
const scanLocal = (
  pattern: string,
  input: string | ScanOptions | undefined,
  runCwd: string,
): string[] => {
  const options = typeof input === "string" ? { cwd: input } : input
  supports("A Bun.Glob scan", options, ["cwd", "dot", "absolute", "onlyFiles"])
  const given = options?.cwd ?? "."
  const cwd = path.isAbsolute(given) ? given : path.resolve(runCwd, given)
  const entries = glob(pattern, cwd, options?.dot === true).filter(
    (entry) => options?.onlyFiles === false || entry.type === "file",
  )
  if (options?.absolute !== true) return entries.map((entry) => entry.path)
  return entries.map((entry) => path.resolve(cwd, entry.path))
}

type Entry = { readonly path: string; readonly type: string }

const globBase = (given: string, cwd: string): string => {
  const pattern = given.replace(/^(\.\/)+/, "")
  const segments = pattern.split("/")
  const wild = segments.findIndex((segment) => /[*?[\]{}()!+@]/.test(segment))
  const fixed =
    (wild < 0 ? segments : segments.slice(0, wild)).join("/") || (path.isAbsolute(pattern) ? "/" : ".")
  return path.resolve(cwd, fixed)
}

const glob = (given: string, cwd: string, dot: boolean): Entry[] => {
  const pattern = given.replace(/^(\.\/)+/, "")
  const segments = pattern.split("/")
  const wild = segments.findIndex((segment) => /[*?[\]{}()!+@]/.test(segment))
  // The fixed part of the pattern is where listing starts. An absolute pattern
  // keeps its root and gives absolute paths.
  const fixed = (wild < 0 ? segments : segments.slice(0, wild)).join("/") || (path.isAbsolute(pattern) ? "/" : ".")
  const base = path.isAbsolute(pattern) ? fixed : path.join(cwd, fixed)
  if (wild < 0) {
    const type = typeOf(base)
    return type === undefined ? [] : [{ path: pattern, type }]
  }
  let entries: Entry[]
  try {
    entries = walk(base, {
      depth: pattern.includes("**") ? Infinity : segments.length - wild,
      hidden: dot || segments.slice(wild).some((segment) => /(^|[{,(|])\./.test(segment)),
    })
  } catch (error) {
    // As in Bun, a fixed part that isn't a directory matches nothing.
    if (wild > 0) return []
    throw error
  }
  return entries
    .map((entry) => ({
      path: path.isAbsolute(pattern)
        ? path.join(base, entry.path)
        : path.join(path.relative(cwd, base), entry.path),
      type: entry.type,
    }))
    .filter((entry) => minimatch(entry.path, pattern, { dot }))
}

// Lists `depth` levels below `directory`: 1 is its own entries, Infinity
// everything. It goes into a directory whose name starts with a dot only when
// `hidden` is set.
const walk = (
  directory: string,
  options: { readonly depth: number; readonly hidden: boolean },
  prefix = "",
): Entry[] => {
  const entries = fs.readdirSync(path.join(directory, prefix), { withFileTypes: true })
  return entries.flatMap((entry) => {
    const item = {
      path: path.join(prefix, entry.name),
      type: entry.isFile()
        ? "file"
        : entry.isDirectory()
          ? "directory"
          : entry.isSymbolicLink()
            ? "symlink"
            : "other",
    }
    if (options.depth <= 1 || item.type !== "directory" || (!options.hidden && entry.name.startsWith(".")))
      return [item]
    return [item, ...walk(directory, { ...options, depth: options.depth - 1 }, item.path)]
  })
}

const typeOf = (target: string): string | undefined => {
  try {
    const info = fs.statSync(target)
    return info.isFile() ? "file" : info.isDirectory() ? "directory" : "other"
  } catch {
    return undefined
  }
}

const within = (root: string, target: string) => {
  const relative = path.relative(path.resolve(root), path.resolve(target))
  return relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))
}

// Only part of Bun's options are implemented. Naming another fails, since
// ignoring it would give a wrong result.
const supports = (api: string, options: object | undefined, supported: ReadonlyArray<string>) => {
  const unsupported = Object.keys(options ?? {}).find((key) => !supported.includes(key))
  if (unsupported === undefined) return
  throw new TypeError(
    `${api} does not support the "${unsupported}" option in Code Mode. Supported options: ${supported.join(", ")}.`,
  )
}

const toPath = (input: string | URL) =>
  typeof input === "string" && !input.startsWith("file:") ? input : fileURLToPath(input)

// Bun's MIME types for common extensions, with the charset Bun adds to some
// text types.
const types = Object.fromEntries(
  Object.entries({
    "text/plain;charset=utf-8": ["txt", "log", "ini"],
    "text/javascript;charset=utf-8": ["js", "mjs", "cjs", "jsx", "ts", "mts", "cts", "tsx"],
    "application/json;charset=utf-8": ["json", "map"],
    "text/html;charset=utf-8": ["html", "htm"],
    "text/css;charset=utf-8": ["css"],
    "text/markdown": ["md"],
    "text/csv": ["csv"],
    "text/yaml": ["yaml", "yml"],
    "application/toml": ["toml"],
    "application/xml": ["xml"],
    "application/pdf": ["pdf"],
    "application/zip": ["zip"],
    "application/gzip": ["gz"],
    "application/x-tar": ["tar"],
    "application/wasm": ["wasm"],
    "image/svg+xml": ["svg"],
    "image/png": ["png"],
    "image/jpeg": ["jpg", "jpeg"],
    "image/gif": ["gif"],
    "image/webp": ["webp"],
    "image/avif": ["avif"],
    "image/x-icon": ["ico"],
    "audio/mpeg": ["mp3"],
    "audio/x-wav": ["wav"],
    "video/mp4": ["mp4"],
    "video/webm": ["webm"],
    "font/woff": ["woff"],
    "font/woff2": ["woff2"],
    "font/ttf": ["ttf"],
    "font/otf": ["otf"],
  }).flatMap(([type, extensions]) => extensions!.map((extension) => [extension, type])),
)

const mime = (name: string) => types[path.extname(name).slice(1).toLowerCase()] ?? "application/octet-stream"
