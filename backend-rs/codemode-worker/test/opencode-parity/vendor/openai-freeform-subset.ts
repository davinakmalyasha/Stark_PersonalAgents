// Verbatim subset of OpenCode v2.4.1 `packages/ai/src/protocols/openai-freeform.ts`
// plus the `encodeJson` helper, with unused imports removed so the functions can
// run in the differential fixture generator. MIT, © OpenCode authors.

import { Option, Schema } from "effect"

export type FreeformLike = {
  readonly type: "text" | "lark" | "regex"
  readonly grammar?: string
  readonly pattern?: string
  readonly field?: string
}

export interface Custom {
  readonly field: string
  readonly format: FreeformLike
  readonly options?: {
    readonly fields: ReadonlyArray<OptionField>
    readonly description: string
    readonly grammar?: string
  }
}

interface OptionField {
  readonly name: string
  readonly schema: Record<string, any>
  readonly required: boolean
}

export type ToolDefinitionLike = {
  readonly name: string
  readonly description: string
  readonly inputSchema: Record<string, any>
  readonly freeform?: FreeformLike
  readonly native?: unknown
}

export const PREFIX = "// @options:"

const Json = Schema.fromJsonString(Schema.Unknown)
export const encodeJson = Schema.encodeSync(Json)

export const isRecord = (value: unknown): value is Record<string, unknown> =>
  typeof value === "object" && value !== null && !Array.isArray(value)

const freeformField = (schema: Record<string, any>, freeform?: FreeformLike) => {
  const properties = Object.entries(isRecord(schema.properties) ? schema.properties : {})
  if (freeform?.field === undefined && properties.length !== 1) return undefined
  const [name, property] = properties.find(([name]) => (freeform?.field ?? name) === name) ?? []
  if (name === undefined || !isRecord(property) || property.type !== "string") return undefined
  return Array.isArray(schema.required) && schema.required.includes(name) ? name : undefined
}

// Verbatim from OpenCode `openai-freeform.ts`; only the JsonSchema guard is
// replaced with a plain-record check for this fixture's valid schemas.
export const resolve = (tool: ToolDefinitionLike, formats?: ReadonlyArray<FreeformLike["type"]>): Custom | undefined => {
  const field = freeformField(tool.inputSchema, tool.freeform)
  if (tool.freeform === undefined || tool.native !== undefined || field === undefined) return undefined
  if (formats !== undefined && !formats.includes(tool.freeform.type)) return undefined
  const lark = formats === undefined || formats.includes("lark")
  const required = Array.isArray(tool.inputSchema.required) ? tool.inputSchema.required : []
  const fields = Object.entries(isRecord(tool.inputSchema.properties) ? tool.inputSchema.properties : {})
    .filter(([name]) => name !== field)
    .map(([name, schema]) => ({
      name,
      schema: isRecord(schema) ? schema : {},
      required: required.includes(name),
    }))
  if (fields.length === 0) return { field, format: tool.freeform }
  const rules = fields.flatMap((item) => valueRule(item.schema) ?? [])
  if (rules.length !== fields.length || (!lark && tool.freeform.type !== "text")) return undefined
  return {
    field,
    format: tool.freeform,
    options: {
      fields,
      description: describe(field, fields),
      grammar: lark ? lineGrammar(fields, rules, tool.freeform) : undefined,
    },
  }
}

const lineGrammar = (fields: ReadonlyArray<OptionField>, rules: ReadonlyArray<string>, format: FreeformLike) => {
  const body = format.type === "text" ? "ff_body" : "fb_start"
  const plain = format.type === "text" ? "ff_plain" : body
  return [
    `start: ff_line ${body}${fields.some((item) => item.required) ? "" : ` | ${plain}`}`,
    `ff_line: ${larkString(PREFIX)} " "* "{" " "* ff_pair (" "* "," " "* ff_pair)* " "* "}" "\\n"`,
    `ff_pair: ${fields.map((item, index) => `${larkString(encodeJson(item.name))} " "* ":" " "* ${rules[index]}`).join(" | ")}`,
    ...(format.type === "text"
      ? ["ff_body: /[\\s\\S]+/", `ff_plain: /${notStartingWith(PREFIX)}/`]
      : format.type === "regex"
        ? [`fb_start: /${format.pattern!.replace(/\\?\//g, "\\/")}/`]
        : [format.grammar!.replace(/^([ \t]*)start([ \t]*:)/m, "$1fb_start$2")]),
  ].join("\n")
}

const valueRule = (schema: Record<string, any>) => {
  if (Array.isArray(schema.enum))
    return schema.enum.length === 0
      ? undefined
      : `(${schema.enum.map((item: unknown) => larkString(encodeJson(item))).join(" | ")})`
  if (schema.type === "boolean") return '("true" | "false")'
  if (schema.type === "number") return "/-?(?:0|[1-9][0-9]*)(?:\\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/"
  if (schema.type === "string") return '/"(?:[^"\\\\\\x00-\\x1f]|\\\\["\\\\\\/bfnrt]|\\\\u[0-9a-fA-F]{4})*"/'
  if (schema.type !== "integer") return undefined
  const lowest = minimum(schema)
  if (lowest >= 1) return "/[1-9][0-9]*/"
  return lowest >= 0 ? "/0|[1-9][0-9]*/" : "/-?(?:0|[1-9][0-9]*)/"
}

const minimum = (schema: Record<string, any>) =>
  Math.max(
    typeof schema.minimum === "number" ? schema.minimum : -Infinity,
    typeof schema.exclusiveMinimum === "number" ? schema.exclusiveMinimum + 1 : -Infinity,
  )

const describe = (field: string, fields: ReadonlyArray<OptionField>) => {
  const required = fields.filter((item) => item.required)
  const example = required.length > 0 ? required : [fields.find((item) => item.schema.type === "boolean") ?? fields[0]]
  return [
    required.length === 0
      ? `To set the parameters below, start with a \`${PREFIX}\` line holding a JSON object of them, then write \`${field}\` from the next line. Leave the line out to use the defaults.`
      : `Start with a \`${PREFIX}\` line holding a JSON object of the parameters below, then write \`${field}\` from the next line.`,
    ...fields.map(
      (item) =>
        `- \`${item.name}\` (${Array.isArray(item.schema.enum) ? `one of ${item.schema.enum.map((value) => encodeJson(value)).join(", ")}` : String(item.schema.type)}${item.required ? ", required" : ""})${typeof item.schema.description === "string" ? `: ${item.schema.description}` : ""}`,
    ),
    "For example:",
    `${PREFIX} {${example.map((item) => `${encodeJson(item.name)}: ${encodeJson(sample(item.schema))}`).join(", ")}}`,
    `<${field}>`,
  ].join("\n")
}

const sample = (schema: Record<string, any>) => {
  if (Array.isArray(schema.enum)) return schema.enum[0]
  if (schema.type === "boolean") return true
  if (schema.type === "string") return "..."
  return Math.max(1, minimum(schema))
}

const larkString = (text: string) => JSON.stringify(text)

const notStartingWith = (prefix: string) => {
  const chars = [...prefix]
  const head = (end: number) => chars.slice(0, end).map(escapeRegex).join("")
  const differs = chars.map((char, index) => `${head(index)}[^${escapeClass(char)}]`)
  const shorter = chars.slice(1).map((_, index) => head(index + 1))
  return [`(?:${differs.join("|")})[\\s\\S]*`, ...shorter].join("|")
}

const escapeRegex = (char: string) => char.replace(/[\\^$.*+?()[\]{}|/]/g, "\\$&")
const escapeClass = (char: string) => char.replace(/[\\\]^\-/]/g, "\\$&")

export const grammar = (custom: Custom) => {
  if (custom.options)
    return custom.options.grammar === undefined
      ? undefined
      : { syntax: "lark" as const, definition: custom.options.grammar }
  const format = custom.format
  if (format.type === "text") return undefined
  return { syntax: format.type, definition: format.type === "regex" ? format.pattern : format.grammar }
}

export const description = (tool: ToolDefinitionLike, custom: Custom) =>
  custom.options ? `${tool.description}\n\n${custom.options.description}` : tool.description

export const toRaw = (custom: Custom, input: unknown, header?: unknown) => {
  const value = isRecord(input) ? input[custom.field] : undefined
  if (typeof value !== "string" || !isRecord(input)) return encodeJson(input ?? {})
  const fields = custom.options?.fields ?? []
  const options = encodeJson(pick(fields, input))
  if (options === "{}") return value
  const saved = typeof header === "string" ? Option.getOrUndefined(decodeLine(header)) : undefined
  const same = saved !== undefined && encodeJson(pick(fields, saved)) === options
  return `${same ? header : `${PREFIX} ${options}`}\n${value}`
}

const pick = (fields: ReadonlyArray<OptionField>, input: Readonly<Record<string, unknown>>) =>
  Object.fromEntries(fields.flatMap((item) => (input[item.name] === undefined ? [] : [[item.name, input[item.name]]])))

export const parse = (custom: Custom | undefined, raw: string) => {
  const end = raw.indexOf("\n")
  if (!custom?.options || !raw.startsWith(PREFIX) || end === -1) return { input: encodeJson({ [field(custom)]: raw }) }
  const header = raw.slice(0, end)
  const options = Option.getOrUndefined(decodeLine(header))
  if (options === undefined) return { input: raw }
  return { input: encodeJson({ ...options, [custom.field]: raw.slice(end + 1) }), header }
}

const decodeLine = (header: string) => decodeArguments(header.slice(PREFIX.length))

const field = (custom: Custom | undefined) => custom?.field ?? "input"

export const streamStart = (custom: Custom | undefined, raw = "") =>
  `{${encodeJson(field(custom))}:"${streamDelta(raw)}`
export const streamDelta = (raw: string) => encodeJson(raw).slice(1, -1)
export const STREAM_END = '"}'

const decodeArguments = Schema.decodeUnknownOption(
  Schema.fromJsonString(Schema.Record(Schema.String, Schema.Unknown)),
)
