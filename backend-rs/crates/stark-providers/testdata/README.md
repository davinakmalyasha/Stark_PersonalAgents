These fixtures are byte-for-byte output from OpenCode's provider layer at
`anomalyco/opencode` dev commit `9964772`:

- `opencode_freeform_execute.json` — `resolve` + `lineGrammar` from
  `packages/ai/src/protocols/openai-freeform.ts` for the full `execute` input
  (`code`, `timeout`, `background`) with `freeform: ["text", "lark"]`.
- `opencode_freeform_execute_text.json` — the `{ code }`-only `execute` custom
  tool.
- `opencode_freeform_sequence.json` — parse/replay/stream cases from the real
  `OpenAIFreeform` helpers, `JSON.stringify` argument normalization, the full
  custom-tool conversion matrix for `execute` + `apply_patch`, and the
  Effect-emitted `execute` schema.

`stark-providers` tests compare Stark's wire values against these files.
Regenerate with `bun test/opencode-parity/generate.ts` from
`backend-rs/codemode-worker`.
