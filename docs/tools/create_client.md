# create_client

- **Description:** Create a client scaffold under D:\MyProject\DFDClientProjects\<slug>\ (client.json + progress.md + mem/). REQUIRES explicit owner approval first (approved_by must be set). Never call without approval.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** dfd
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** create_client(business_name, vertical, features?, whatsapp?, domain?, approved_by?, order_id?) — Create client scaffold under D:\MyProject\DFDClientProjects\<slug>\ (client.json + progress.md + mem/). REQUIRES explicit owner approval (approved_by).
- **Required args (validated):** business_name, vertical
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `business_name` (string, required): Client business name (slugified to folder name).
  - `vertical` (string, required): One of: fnb, salon, laundry, klinik, gym, bimbel, bengkel, beauty, travel, property, katering, photo.
  - `features` (array, optional): Selected features, e.g. ['online_order','booking','review_qr'].
  - `whatsapp` (string, optional): Client WhatsApp number.
  - `domain` (string, optional): Client domain (optional).
  - `approved_by` (string, optional): Who approved this scaffold (must be non-empty; the DFD agent sets it after owner approval).
  - `order_id` (string, optional): DFD order id this client belongs to (optional).

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
