# search_code

- **Description:** Regex-search file contents ('content', default) or match path globs ('files'). Output styles: lines (default), files (matching paths), count (matches per file). Matching lines are annotated with their enclosing symbol unless annotate=false; snippet='function' returns the enclosing function body instead of single lines. Multi-pattern results ranked via patterns=[...]. Results are workspace-relative; up to max_matches lines shown (default 100; full list saved to /scratch when exceeded). Narrow with path/glob.
- **Exposed to model:** True
- **Scopes (from _SCOPE_MAP):** developer, ui, deploy
- **Parallel-safe (PARALLEL_SAFE):** True
- **Compact doc:** search_code(pattern?, patterns?, path?, glob?, mode?, output?, ignore_case?, smart_case?, word?, fixed_strings?, context?, max_matches?, annotate?, snippet?) — Regex search (content) or filename search. output=lines|files|count; results annotated with enclosing symbol; snippet=function returns the body; up to 100 lines shown (full list in scratch).
- **Required args (validated):** (none)
- **Deps-migrated:** True (accepts ctx=WorkspaceDeps via registry shim)
- **Native equivalent (pydantic-ai 2.36):** Tool.from_schema | cache via Hooks | FilteredToolset
- **Pipeline:** direct shim call (no hooks)

## Parameters (JSON Schema)

  - `pattern` (string, optional): Regex (content mode) or glob/filename fragment (files mode).
  - `patterns` (array, optional): Multi-pattern ranked search (replaces pattern).
  - `path` (string, optional): Directory or single file to search (default: workspace root).
  - `glob` (string, optional): Comma-separated file filters, '!' excludes; e.g. '*.py,*.ts,!*.test.*'.
  - `mode` (string, optional): 'content' regex search or 'files' filename search.
  - `output` (string, optional): Content answer style: matching lines, matching paths, or matches per file.
  - `ignore_case` (boolean, optional): Case-insensitive matching.
  - `smart_case` (boolean, optional): Ignore case when the pattern has no uppercase letters.
  - `word` (boolean, optional): Match whole words only.
  - `fixed_strings` (boolean, optional): Treat the pattern as a literal string, not a regex.
  - `context` (integer, optional): Lines of context around each match (lines output).
  - `max_matches` (integer, optional): Matching lines shown inline (default 100).
  - `annotate` (boolean, optional): Prefix each match with its enclosing symbol (default true).
  - `snippet` (string, optional): line (default) or function: show the enclosing function body once per function.

## How to recreate

1. `tools/_schemas.py` `TOOL_DEFINITIONS` entry (above schema).
2. `_TOOL_DOCS` one-liner.
3. Implementation in `tools/MainTool/` or `tools/AddOnTool/`, exported in `tools/__init__.py`.
4. `agents/workspace_registry.py` — agent scope (single source for `_SCOPE_MAP`).
5. `PARALLEL_SAFE` in `tools/_parallel.py` if read-only concurrency-safe.
6. Native path scopes via `FilteredToolset` (pydantic-ai); legacy shim has no runtime scope filter.
