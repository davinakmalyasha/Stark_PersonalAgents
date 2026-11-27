// @ts-nocheck -- fixture generator drives OpenCode's untyped runtime source
// Differential fixture generator: drives the real OpenCode v2.4.1
// `CodeModeInstructions` + `Instructions` state machine over the shared
// scenarios and writes the expected blocks for the Rust port's tests.
//
// Run from `backend-rs/codemode-worker`:
//   bun test/opencode-parity/generate.ts
//
// The vendored files are byte copies of the OpenCode source at dev 9964772
// (MIT), with import specifiers rewritten to this folder. Attribution:
// © OpenCode authors, MIT.

import { readFileSync, writeFileSync } from "fs"
import { dirname, join } from "path"
import { fileURLToPath } from "url"
import { Effect, Option, Schema } from "effect"
import { CodeMode } from "@opencode/codemode"
import {
  BACKGROUND_DESCRIPTION,
  TIMEOUT_DESCRIPTION,
  description as executeDescription,
} from "./vendor/codemode-tool.ts"

import { CodeModeInstructions } from "./vendor/codemode-instructions.ts"
import { Instructions } from "./vendor/instructions/index.ts"
import { wrapSystemUpdate } from "./vendor/system-update.ts"
import { truncate as truncateToolOutput } from "./vendor/tool-output.ts"
import {
  PREFIX,
  STREAM_END,
  description as customDescription,
  encodeJson,
  grammar as customGrammar,
  parse,
  resolve,
  streamDelta,
  streamStart,
  toRaw,
  type ToolDefinitionLike,
} from "./vendor/openai-freeform-subset.ts"

type Item = {
  path: string
  description: string
  signature: string
  pinned: boolean
}

type Step = Array<Item> | null

type Scenario = {
  name: string
  steps: Step[]
}

const here = dirname(fileURLToPath(import.meta.url))
const input = JSON.parse(readFileSync(join(here, "scenarios.json"), "utf8")) as {
  source: string
  scenarios: Scenario[]
}

const LONG = "x".repeat(20_000)

function expand(item: Item): Item {
  return { ...item, description: item.description.replaceAll("HUGEDESCRIPTION", LONG) }
}

// Flat Stark paths -> OpenCode's namespace tree.
function inventory(items: Item[]) {
  const namespaces = new Map<string, { type: "namespace"; name: string; tools: unknown[] }>()
  for (const item of items) {
    const [namespace, ...rest] = item.path.split(".")
    const entry = namespaces.get(namespace) ?? { type: "namespace" as const, name: namespace, tools: [] }
    entry.tools.push({
      type: "tool",
      name: rest.join("."),
      description: item.description,
      signature: item.signature,
      pinned: item.pinned,
    })
    namespaces.set(namespace, entry)
  }
  return { tools: [...namespaces.values()] }
}

function dereference(values: Record<string, string>, blobs: Record<string, unknown>) {
  return Object.fromEntries(Object.entries(values).map(([key, hash]) => [key, blobs[hash]]))
}

function dereferenceDelta(delta: Record<string, string>, blobs: Record<string, unknown>) {
  return Object.fromEntries(
    Object.entries(delta).map(([key, hash]) => [key, hash === "removed" ? Option.none() : Option.some(blobs[hash])]),
  )
}

async function runScenario(scenario: Scenario) {
  const steps: unknown[] = []
  let values: Record<string, string> | undefined
  let initialValues: Record<string, string> = {}
  let initialText: string | null = null
  const blobs: Record<string, unknown> = {}

  for (const step of scenario.steps) {
    const items = step === null ? null : step.map(expand)
    const list = CodeModeInstructions.make(items === null ? undefined : inventory(items))
    const observed = await Effect.runPromise(Instructions.read(list))
    const admission = await Effect.runPromise(Instructions.diff(observed, values))
    Object.assign(blobs, admission.blobs)

    let update: string | null = null
    if (values === undefined) {
      initialValues = Instructions.applyHashDelta({}, admission.delta)
      const rendered = Instructions.renderInitial(list, dereference(initialValues, blobs))
      initialText = rendered.length === 0 ? null : Instructions.join(rendered)
      values = initialValues
    } else {
      const changed = Object.keys(admission.delta).length > 0
      if (changed) {
        const text = Instructions.renderUpdate(
          list,
          dereference(values, blobs),
          dereferenceDelta(admission.delta, blobs),
        )
        update = text.length === 0 ? null : text
      }
      values = Instructions.applyHashDelta(values, admission.delta)
    }

    steps.push({
      inventory: items,
      initial: initialText,
      update,
      wrapped: update === null ? null : wrapSystemUpdate([{ text: update }]),
    })
  }
  return { name: scenario.name, steps }
}

const scenarios = []
for (const scenario of input.scenarios) scenarios.push(await runScenario(scenario))

const executeContract = {
  source: "opencode v2.4.1 (dev 9964772) packages/core/src/codemode/tool.ts",
  description: { base: executeDescription(false), full: executeDescription(true) },
  timeout_description: TIMEOUT_DESCRIPTION,
  background_description: BACKGROUND_DESCRIPTION,
}
const executeContractTarget = join(
  here,
  "../../../crates/stark-tools/src/codemode/testdata/opencode_execute_contract.json",
)
writeFileSync(executeContractTarget, `${JSON.stringify(executeContract, null, 2)}\n`)
console.log(`wrote ${executeContractTarget}`)

const output = {
  source: input.source,
  generated_by: "backend-rs/codemode-worker/test/opencode-parity/generate.ts",
  scenarios,
}
const target = join(here, "../../../crates/stark-tools/src/codemode/testdata/opencode_instruction_sequence.json")
writeFileSync(target, `${JSON.stringify(output, null, 2)}\n`)
console.log(`wrote ${target} (${scenarios.length} scenarios)`)

// OpenCode `OpenAIFreeform` parse/replay/stream differential cases.
const full = {
  field: "code",
  format: { type: "text" },
  options: {
    fields: [
      { name: "timeout", schema: { type: "integer", minimum: 1 }, required: false },
      { name: "background", schema: { type: "boolean" }, required: false },
    ],
  },
}
const plain = { field: "code", format: { type: "text" } }

const parseCases = [
  { name: "options_and_code", custom: "full", raw: '// @options: {"timeout": 1}\nlet x' },
  { name: "spaced_options", custom: "full", raw: '// @options: { "background" : true }\nx' },
  { name: "crlf_options", custom: "full", raw: '// @options:{"background" :false,"timeout":2}\r\nx' },
  { name: "no_newline", custom: "full", raw: '// @options: {"timeout": 1}' },
  { name: "malformed_options", custom: "full", raw: "// @options: {timeout: 1}\nx" },
  { name: "array_options", custom: "full", raw: "// @options: [1]\nx" },
  { name: "plain_code", custom: "full", raw: "plain code" },
  { name: "plain_custom", custom: "plain", raw: '// @options: {"timeout": 1}\nx' },
  { name: "undeclared_custom", custom: "none", raw: '// @options: {"timeout": 1}\nx' },
].map((item) => {
  const custom = item.custom === "full" ? full : item.custom === "plain" ? plain : undefined
  return { name: item.name, custom: item.custom, raw: item.raw, ...parse(custom, item.raw) }
})

const replayCases = [
  { name: "exact_header", input: { code: "x", timeout: 1 }, header: '// @options: { "timeout" : 1}' },
  { name: "same_values_new_layout", input: { code: "x", timeout: 1 }, header: '// @options: {"timeout":1}' },
  { name: "changed_value", input: { code: "x", timeout: 2 }, header: '// @options: {"timeout":1}' },
  { name: "options_without_header", input: { code: "x", background: true } },
  { name: "code_only", input: { code: "x" } },
  { name: "non_string_code", input: { code: 3 } },
].map((item) => ({
  name: item.name,
  input: item.input,
  header: item.header,
  raw: toRaw(full, item.input, item.header),
}))

const streamCases = [
  { name: "two_chunks", chunks: ["let ", "x = 1"] },
  { name: "empty_chunk", chunks: [""] },
  { name: "escapes", chunks: ['a"b\\c\n', "<x>"] },
].map((item) => {
  const fragments =
    item.chunks.length === 0
      ? []
      : [
          streamStart(full, item.chunks[0]),
          ...item.chunks.slice(1).map((chunk) => streamDelta(chunk)),
          STREAM_END,
        ]
  return { name: item.name, chunks: item.chunks, fragments }
})

const jsonCases = [
  '{"file_path":"a.txt", "limit": 2}',
  '{"text":"a\\/b\\u00e9"}',
  '{"n":1.0}',
  '{"n":1e2}',
  '{"n":1.5e-3}',
  '{"n":1e21}',
  '{"n":-0.0}',
  '{"b":true,"a":false}',
  '{"nested":[1,{"x":"y"}],"empty":{}}',
  '{"ctrl":"\\u0001\\n"}',
].map((raw) => ({ raw, normalized: encodeJson(JSON.parse(raw)) }))

// Complete custom-tool conversion for Stark's two freeform tools, derived by
// the real `resolve`/`grammar`/`description` functions.
const patchGrammar = readFileSync(
  join(here, "../../../crates/stark-providers/src/patch_grammar.lark"),
  "utf8",
)

const executeSchema = {
  type: "object",
  properties: {
    code: { type: "string" },
    timeout: {
      type: "integer",
      maximum: 9_007_199_254_740_991,
      exclusiveMinimum: 0,
      description:
        "Optional time in milliseconds to wait for the script before moving it to the background (default 120000). The script keeps running in the background after that; it is never stopped. With `background: true` the call returns immediately instead.",
    },
    background: {
      type: "boolean",
      description:
        "Run the script in the background and return immediately with its job ID (useful for long-running work). You will be notified when it completes. DO NOT poll for completion.",
    },
  },
  required: ["code"],
  additionalProperties: false,
}

const definitions: ToolDefinitionLike[] = [
  {
    name: "execute",
    description: "desc",
    inputSchema: executeSchema,
    freeform: { type: "text", field: "code" },
  },
  {
    name: "apply_patch",
    description: "Apply a unified patch.",
    inputSchema: {
      type: "object",
      properties: {
        input: {
          type: "string",
          description: "Patch text. Start with *** Begin Patch and end with *** End Patch.",
        },
      },
      required: ["input"],
    },
    freeform: { type: "lark", grammar: patchGrammar },
  },
]

const functionTool = (tool: ToolDefinitionLike) => ({
  type: "function",
  function: {
    name: tool.name,
    description: tool.description,
    parameters: tool.inputSchema,
  },
})

const lowerTool = (tool: ToolDefinitionLike, formats: string[]) => {
  const custom = resolve(tool, formats as never)
  if (!custom) return functionTool(tool)
  const grammar = customGrammar(custom)
  return {
    type: "custom",
    custom: {
      name: tool.name,
      description: customDescription(tool, custom),
      format: grammar ? { type: "grammar", grammar } : { type: "text" },
    },
  }
}

const toolCases = [
  { name: "text_and_lark", formats: ["text", "lark"] },
  { name: "text_only", formats: ["text"] },
  { name: "lark_only", formats: ["lark"] },
  { name: "json_only", formats: [] },
].map((item) => ({
  name: item.name,
  formats: item.formats,
  tools: definitions.map(functionTool),
  expected: definitions.map((tool) => lowerTool(tool, item.formats)),
}))

// OpenCode's provider-facing execute schema, generated by Effect exactly as
// `packages/core/src/tool/runtime.ts` does (`onExcessProperty: "error"`).
const PositiveInt = Schema.Int.check(Schema.isGreaterThan(0))
const fullInput = Schema.Struct({
  ...CodeMode.Input.fields,
  timeout: Schema.optionalKey(PositiveInt).annotate({ description: TIMEOUT_DESCRIPTION }),
  background: Schema.optionalKey(Schema.Boolean).annotate({ description: BACKGROUND_DESCRIPTION }),
})
const toJsonSchema = (schema: any) => {
  const document = Schema.toJsonSchemaDocument(schema, { onExcessProperty: "error" })
  return Object.keys(document.definitions).length === 0
    ? document.schema
    : { ...document.schema, $defs: document.definitions }
}
const executeJsonSchema = {
  base: toJsonSchema(CodeMode.Input),
  full: toJsonSchema(fullInput),
}
// The tool-conversion cases must use the exact Effect-emitted schema.
if (JSON.stringify(executeSchema) !== JSON.stringify(executeJsonSchema.full)) {
  throw new Error("execute tool schema drift between the conversion fixture and Effect")
}

const freeform = {
  source: "opencode v2.4.1 (dev 9964772) packages/ai/src/protocols/openai-freeform.ts",
  prefix: PREFIX,
  parse: parseCases,
  replay: replayCases,
  stream: streamCases,
  json: jsonCases,
  tools: toolCases,
  execute_schema: executeJsonSchema,
}
const freeformTarget = join(here, "../../../crates/stark-providers/testdata/opencode_freeform_sequence.json")
writeFileSync(freeformTarget, `${JSON.stringify(freeform, null, 2)}\n`)
console.log(
  `wrote ${freeformTarget} (${parseCases.length + replayCases.length + streamCases.length + toolCases.length} cases)`,
)

// OpenCode `ToolOutput.truncate` over the single text part Stark tools return.
const toolOutputCases = [
  { name: "short", text: "hello\nworld" },
  { name: "empty", text: "" },
  { name: "only_newline", text: "\n" },
  { name: "trailing_newline", text: "a\n" },
  { name: "exactly_bytes", text: "a".repeat(50 * 1024) },
  { name: "one_byte_over", text: "a".repeat(50 * 1024 + 1) },
  { name: "line_boundary", text: Array.from({ length: 2_000 }, (_, line) => `line ${line}`).join("\n") },
  { name: "line_over", text: Array.from({ length: 2_001 }, (_, line) => `line ${line}`).join("\n") },
  {
    name: "many_lines",
    text: Array.from({ length: 3_000 }, (_, line) => `line ${line}`).join("\n"),
  },
  {
    name: "long_line",
    text: "x".repeat(60_000),
  },
  {
    name: "emoji",
    text: "😀".repeat(20_000),
  },
  {
    name: "flag_emoji",
    text: "🇺🇸".repeat(10_000),
  },
  {
    name: "zwj_emoji",
    text: "👨‍👩‍👧‍👦".repeat(5_000),
  },
  {
    name: "emoji_line_start",
    text: `😀${"x".repeat(50_000)}`,
  },
  {
    name: "crlf_boundary",
    text: `${"a".repeat(50 * 1024 - 1)}\r\nsecond`,
  },
  {
    name: "multibyte_boundary",
    text: `${"é".repeat(25_000)}tail`,
  },
].map((item) => ({
  name: item.name,
  text: item.text,
  // OpenCode's spill path is random; the fixture substitutes a fixed name.
  expected: truncateToolOutput(item.text, undefined, "tool-output/tool_FILE"),
}))

const toolOutput = {
  source: "opencode v2.4.1 (dev 9964772) packages/core/src/tool-output.ts",
  max_lines: 2_000,
  max_bytes: 50 * 1024,
  cases: toolOutputCases,
}
const toolOutputTarget = join(
  here,
  "../../../crates/stark-tools/src/codemode/testdata/opencode_tool_output.json",
)
writeFileSync(toolOutputTarget, `${JSON.stringify(toolOutput, null, 2)}\n`)
console.log(`wrote ${toolOutputTarget} (${toolOutputCases.length} cases)`)
