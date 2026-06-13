"""
Native Function Calling Tool Schemas — DeepSeek API compatible.

Each tool is a JSON Schema function definition passed via the `tools` API
parameter. The model returns structured tool_calls — no text parsing needed.

CACHE-DRIVEN POLICY — descriptions live in the SYSTEM PROMPTS, not here:
the `tools` parameter is serialized at the request TAIL, so every byte of it
is re-paid on every cache break (session start / config transition / task
change). Tool descriptions are therefore OMITTED entirely — the model gets
names, parameters, types, and required fields here, and the full helpful
descriptions live in the global (always-cached) system prompt text
("## Tool Guidance" in developer_prompt / sdet_prompt / devops_prompt).

TOOL SURFACE: 14 tools. GitHub operations were removed (agent surface);
the 8 browser primitives were unified into browser_navigate + browser_act
(with an action enum) to keep the tail minimal.
"""

def get_native_tools() -> list[dict]:
    """Return all tools as DeepSeek-native function definitions (structural
    only — see module docstring for why descriptions are omitted)."""
    tools = [
        # ── File Reading ──
        {
            "type": "function",
            "function": {
                "name": "read_file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "offset": {"type": "integer"},
                        "limit": {"type": "integer"},
                    },
                    "required": ["file_path"],
                },
            },
        },
        # ── File Writing ──
        {
            "type": "function",
            "function": {
                "name": "write_file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "content": {"type": "string"},
                    },
                    "required": ["file_path", "content"],
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "edit_file",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                        "old_string": {"type": "string"},
                        "new_string": {"type": "string"},
                    },
                    "required": ["file_path", "old_string", "new_string"],
                },
            },
        },
        # ── Execution ──
        {
            "type": "function",
            "function": {
                "name": "run_command",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "timeout": {"type": "integer"},
                        "background": {"type": "boolean"},
                        "process_action": {"type": "string"},
                    },
                    "required": ["command"],
                },
            },
        },
        # ── Code Search ──
        {
            "type": "function",
            "function": {
                "name": "search_code",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "pattern": {"type": "string"},
                        "path": {"type": "string"},
                        "glob": {"type": "string"},
                    },
                    "required": ["pattern"],
                },
            },
        },
        # ── Directory Listing ──
        {
            "type": "function",
            "function": {
                "name": "list_files",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "path": {"type": "string"},
                        "pattern": {"type": "string"},
                        "recursive": {"type": "boolean"},
                    },
                },
            },
        },
        # ── Semantic Search ──
        {
            "type": "function",
            "function": {
                "name": "search_codebase",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "top_k": {"type": "integer"},
                        "language": {"type": "string"},
                        "reindex": {"type": "boolean"},
                    },
                    "required": ["query"],
                },
            },
        },
        # ── Signature Extraction ──
        {
            "type": "function",
            "function": {
                "name": "view_signatures",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                    },
                    "required": ["file_path"],
                },
            },
        },
        # ── History ──
        {
            "type": "function",
            "function": {
                "name": "query_history",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "checkpoint_id": {"type": "string"},
                    },
                },
            },
        },
        {
            "type": "function",
            "function": {
                "name": "compact_conversation",
                "parameters": {"type": "object", "properties": {}},
            },
        },
        # ── Undo ──
        {
            "type": "function",
            "function": {
                "name": "revert_last_change",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "file_path": {"type": "string"},
                    },
                    "required": ["file_path"],
                },
            },
        },
        # ── Web Research ──
        {
            "type": "function",
            "function": {
                "name": "web_fetch",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "url": {"type": "string"},
                        "max_chars": {"type": "integer"},
                    },
                    "required": ["url"],
                },
            },
        },
        # ── Browser (UI verification) ──
        {
            "type": "function",
            "function": {
                "name": "browser_act",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "action": {
                            "type": "string",
                            "enum": ["navigate", "click", "fill", "type", "press",
                                     "extract", "screenshot", "close"],
                        },
                        "url": {"type": "string"},
                        "wait_ms": {"type": "integer"},
                        "selector": {"type": "string"},
                        "text": {"type": "string"},
                        "key": {"type": "string"},
                        "what": {"type": "string"},
                        "path": {"type": "string"},
                    },
                    "required": ["action"],
                },
            },
        },
    ]
    return sorted(tools, key=lambda t: t["function"]["name"])
