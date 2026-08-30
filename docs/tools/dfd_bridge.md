# dfd_bridge

- **Description:** Call the DFD Agency API through the agent bridge (NON-MONEY operations only): leads, orders, milestones, order notes, activities, client requests, projects, articles, WhatsApp progress messages. Never use for payments/invoices/contracts/pricing.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** dfd
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** dfd_bridge(action?, id?, orderId?, payload?, path?, method?, url?, timeout?) — Call the DFD Agency API bridge (NON-MONEY): leads, orders, milestones, notes, activities, client requests, projects, articles, whatsapp. Named actions (lead_list, order_get, milestone_create, note_create, activity_create, request_update, project_update, article_create, whatsapp_send) or raw method+url.
- **Required args (validated):** (none)
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** standard registry: pre_hooks(cache) -> guards(plan-mode) -> execute -> post_hooks(redact/offload)

## Parameters (JSON Schema)

  - `action` (string, optional): Named endpoint: lead_list|lead_get|lead_update|order_list|order_get|order_milestones|milestone_create|milestone_update|note_create|note_update|activity_create|request_list|request_get|request_update|project_list|project_get|project_update|article_list|article_create|article_update|whatsapp_send.
  - `id` (string, optional): Record id for get/update endpoints.
  - `orderId` (string, optional): Order id for order-scoped endpoints.
  - `payload` (object, optional): JSON body for create/update endpoints (non-money fields only).
  - `path` (object, optional): Optional path-param dict for raw url mode.
  - `method` (string, optional): HTTP method for raw url mode (default GET).
  - `url` (string, optional): Relative URL for raw mode, e.g. '/api/agent/orders'.
  - `timeout` (number, optional): Request timeout seconds (default 30).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. Optional `input_schema` in `tools/_tool_schemas.py` (opt-in Stage1 validation).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Registry auto-derives scope visibility from `_SCOPE_MAP` (runtime `restrict_scope`).
