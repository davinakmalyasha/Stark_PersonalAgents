/**
 * OpenCode `CodeModePath.extension`: Node's `path` module with `resolve` and
 * `relative` bound to the session's working directory.
 *
 * Port of `packages/core/src/codemode/path.ts` (MIT, OpenCode authors). The
 * host environment lookup becomes the worker's `Run` service.
 */
import { Effect } from "effect"
import path from "node:path"
import { Extension } from "@opencode/codemode"
import { Run } from "./run.ts"

// Only functions and separators cross; `posix` and `win32` point back at their
// module, which a global can't hold, so each becomes a copy of its own
// functions. Programs have no `process.cwd()`: the default flavour resolves
// from the session's effective working directory. The other flavour can't read
// that directory as absolute (`C:\\proj` isn't a POSIX path), so it resolves
// from its own root instead of falling back to the server's working directory.
const flavor = (module: any) => {
  const { posix: _posix, win32: _win32, ...functions } = module
  const context = Effect.map(Run, (run) => ({
    directory: module === path ? run.cwd : module.sep,
    rules: module,
  }))
  return {
    ...functions,
    resolve: (...parts: Array<string>) =>
      Effect.map(context, (current) => current.rules.resolve(current.directory, ...parts)),
    relative: (from: string, to: string) =>
      Effect.map(context, (current) =>
        current.rules.relative(
          current.rules.resolve(current.directory, from),
          current.rules.resolve(current.directory, to),
        ),
      ),
  }
}

export const extension = Extension.make({
  name: "path",
  globals: { path: { ...flavor(path), posix: flavor(path.posix), win32: flavor(path.win32) } },
})
