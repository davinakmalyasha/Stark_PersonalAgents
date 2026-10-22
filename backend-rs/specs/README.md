# Frozen tool contract

`tool-specs.json` is generated from `tools/_schemas.py` by
`scripts/export_tool_specs.py`. Do not edit it by hand.

- 42 registered tools from `TOOL_DEFINITIONS`
- 8 agent scopes from `runtime _SCOPE_MAP`
- 6 tools with `exposed: false`
- 41 compact docs; `update_feature`, `update_project_meta`, and `files_changed`
  currently have no compact doc
- `browser_click` and `browser_fill` have compact docs but no registered tool

`propose_plan` is implemented in `tools/MainTool/proposePlan.py` and allowed by
the plan-mode guard, but it is **not** in `TOOL_DEFINITIONS` or `_SCOPE_MAP`, so
the native model-facing tool schema never exposes it. It is intentionally absent
from this contract until that behavior is changed deliberately.

Regenerate:

```bash
venv/Scripts/python.exe -B scripts/export_tool_specs.py
```

Verify without writing:

```bash
venv/Scripts/python.exe -B scripts/export_tool_specs.py --check
```
