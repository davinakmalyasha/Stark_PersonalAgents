// Verbatim excerpts from OpenCode v2.4.1
// `packages/core/src/codemode/tool.ts` (MIT, © OpenCode authors): the
// invariant `execute` description, its full-mode variant, and the
// `FullInput` parameter descriptions.

export const description = (full: boolean) =>
  [
    full
      ? "Run JavaScript in a confined Code Mode runtime to work with files and data, script tool calls and HTTP requests, and compose their results."
      : "Run JavaScript in a confined Code Mode runtime to script tool calls and HTTP requests and compose their results.",
    "Write plain JavaScript and send it exactly as written: no code fence, and no JSON or other wrapper around it.",
    "Each call is a fresh program; nothing carries over from earlier calls.",
    full
      ? "`fetch` is available for HTTP requests, and Bun's file APIs (`Bun.file`, `Bun.write`, `Bun.Glob`) for files, in place of Node's `fs`. Imports and timers are unavailable: there is no `setTimeout` or sleep. All other external access goes through `tools`."
      : "`fetch` is available for HTTP requests. Imports, direct filesystem access, and timers are unavailable: there is no `setTimeout` or sleep. All other external access goes through `tools`.",
    "Inside `execute`, the only callable tools are those explicitly listed in the Code Mode catalog instructions or returned by the `search` function. Ignore tools shown outside the Code Mode catalog. They are not available in the Code Mode runtime.",
    'Call tools through `tools` using only exact paths and signatures from the catalog. Do not infer or normalize tool names; preserve bracket notation such as `tools.<namespace>["tool-name"](input)`.',
    "Prefer an explicit `return`; if omitted, the final top-level expression becomes the result.",
    "Await every call whose completion matters; pending calls are interrupted when execution ends. Run independent calls concurrently with `Promise.allSettled`.",
    ...(full
      ? [
          "Scripts still running after their timeout move to the background and keep running. Background scripts return a job ID immediately, and you will be notified when they complete; do not poll for them.",
        ]
      : []),
  ].join("\n")

export const TIMEOUT_DESCRIPTION = `Optional time in milliseconds to wait for the script before moving it to the background (default ${2 * 60 * 1_000}). The script keeps running in the background after that; it is never stopped. With \`background: true\` the call returns immediately instead.`

export const BACKGROUND_DESCRIPTION =
  "Run the script in the background and return immediately with its job ID (useful for long-running work). You will be notified when it completes. DO NOT poll for completion."
