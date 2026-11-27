# OpenCode parity fixtures

Generated fixtures for the Rust Code Mode port, produced by running OpenCode
v2.4.1 source (dev `9964772`) directly under Bun.

- `vendor/` contains byte copies of the relevant OpenCode modules (MIT,
  © OpenCode authors) with only import specifiers rewritten to this folder.
  `openai-freeform-subset.ts`, `system-update.ts`, and `tool-output.ts` are
  verbatim function excerpts of their source modules, with unused imports and
  service wiring removed.
- `scenarios.json` lists catalog/instruction observations shared by both
  implementations.
- `generate.ts` drives the real `CodeModeInstructions`, `Instructions` state
  machine, `OpenAIFreeform.parse`/`toRaw`/stream helpers, and the
  `<system-update>` wrapper, then writes the expected fixtures consumed by
  Rust tests.

Regenerate from `backend-rs/codemode-worker`:

```sh
bun test/opencode-parity/generate.ts
```

Outputs:

- `crates/stark-tools/src/codemode/testdata/opencode_instruction_sequence.json`
- `crates/stark-tools/src/codemode/testdata/opencode_tool_output.json`
- `crates/stark-providers/testdata/opencode_freeform_sequence.json`
