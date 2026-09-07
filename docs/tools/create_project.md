# create_project

- **Description:** Create a NEW project: creates the folder (under D:\MyProject, or under parent_root if given), registers it as a workspace with its own chat, and assigns a durable Dev agent to it. Use ONLY when the user genuinely asks for a new project. NEVER call this for work on an existing project. Pass wait=false when creating SEVERAL projects in one burst — their first Dev turns run in parallel in the background. The DFD agent MUST pass parent_root (the client folder under D:\MyProject\DFDClientProjects) so client projects always live inside the client root.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** orchestrator, dfd
- **Parallel-safe (PARALLEL_SAFE):** False
- **Compact doc:** create_project(name, concept, wait?, parent_root?) — Create a NEW project: folder (D:\MyProject, or parent_root for the DFD agent) + workspace + chat + durable Dev. Only for genuinely new projects. wait=false for bursts. DFD agent passes parent_root inside D:\MyProject\DFDClientProjects.
- **Required args (validated):** concept, name
- **Deps-migrated:** False (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema(TOOL_DEFINITIONS) | Hooks.before/after_tool_execute (cache/guard/redact/offload) | Tool(args_validator, max_retries) | FilteredToolset(allowed_tools)
- **Pipeline:** thin registry shim: direct execute -> ToolExecutionResult

## Parameters (JSON Schema)

  - `name` (string, required): Project/folder name. 1-64 chars; filesystem-safe — avoid '/', '\\', '..', control characters, and leading dots. Use kebab-case or snake_case.
  - `concept` (string, required): The user's concept/goal for the project; the Dev agent starts from this.
  - `wait` (boolean, optional): Wait for the Dev's first turn before returning. Use false for multi-project bursts.
  - `parent_root` (string, optional): Optional parent folder. DFD agent: the client folder under D:\MyProject\DFDClientProjects (enforced to stay inside it). Others: leave empty.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
