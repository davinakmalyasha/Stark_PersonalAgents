from typing import Optional

"""Central model-facing tool schemas and compact tool documentation.

The facade imports these values back so existing callers continue to use
tools.TOOL_DEFINITIONS, tools._TOOL_DOCS, and tools._TOOL_REQUIRED_ARGS.
"""

TOOL_DEFINITIONS = [
    {
        "name": "read_file",
        "description": "Reads a file from the local filesystem. Use this to inspect code, configs, or any project file. Supports multi-file read via file_paths.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read. Alias for single-file reads; use file_paths for multi-file."
                },
                "file_paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional. Read multiple files in one turn — returns per-file sections with headers."
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional. Line number to start reading from (single-file only; ignored for multi-file)."
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional. Maximum number of lines to read (single-file only; ignored for multi-file)."
                }
            },
            "required": []
        }
    },
    {
        "name": "edit_file",
        "description": "Edit an existing file using SEARCH/REPLACE blocks, a unified diff, or a multi-file patch. PREFERRED method for all file edits.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to modify. Ignored for multi-file patches (paths come from the patch headers)."
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact text to replace (single edit mode). Must match the file content exactly."
                },
                "new_string": {
                    "type": "string",
                    "description": "The text to replace it with (single edit mode)."
                },
                "diff": {
                    "type": "string",
                    "description": "Batch edit mode. Accepts (1) SEARCH/REPLACE blocks: <<<<<<< SEARCH\\n[old]\\n=======\\n[new]\\n>>>>>>> REPLACE; (2) a unified diff (git-style @@ hunks, uses file_path); or (3) a MULTI-FILE patch with opencode-style markers ('*** Add File: path', '*** Update File: path', '*** Delete File: path', '*** Move to: path') or '--- a/path' / '+++ b/path' headers. Multi-file patches apply every section transactionally per file and return a per-file [OK]/[ERROR] summary."
                },
                "verify": {
                    "type": "string",
                    "description": "Optional. Opt-in check after a successful write: 'py_compile', 'tsc', or any shell command (e.g. 'pytest -q'). Omit to skip."
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "run_command",
        "description": "Executes a shell command. Can be run in foreground (blocking, streams logs in real-time) or background (non-blocking, returns immediately). Use process_action parameter to list, view logs, or kill background processes. Supports workdir, expect regex, and streaming.",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute. Optional if process_action is 'list', but required as the target process name/key for 'logs' or 'kill'."
                },
                "timeout": {
                    "type": "integer",
                    "description": "Optional timeout in milliseconds for foreground execution. Default 30000."
                },
                "background": {
                    "type": "boolean",
                    "description": "Optional. If true, run command in background and return a key to manage it. Useful for starting servers."
                },
                "process_action": {
                    "type": "string",
                    "description": "Optional action to manage background processes: 'list' (list all), 'logs' (get logs for process named in command), 'kill' (terminate process named in command)."
                },
                "shell": {
                    "type": "string",
                    "description": "Optional shell override: 'bash' (default, POSIX) or 'cmd' for legacy plain cmd (.bat files)."
                },
                "workdir": {
                    "type": "string",
                    "description": "Optional working directory to run the command in. Defaults to project root."
                },
                "expect": {
                    "type": "string",
                    "description": "Optional regex to expect in output — returns typed pass/fail with structured exit handling."
                },
                "stream": {
                    "type": "boolean",
                    "description": "Optional. If true, stream output as it arrives (currently returns full output; future: incremental deltas)."
                }
            },
            "required": []
        }
    },
    {
        "name": "search_code",
        "description": "Searches code. Default 'content' mode regex-matches file contents (ripgrep-style). Use 'files' mode with a glob pattern (e.g. '**/*.py', 'setup*') to find files by name. Supports multi-pattern via patterns=[...] with ranked results.",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Content mode: the regex pattern to search for in file contents. Files mode: the glob pattern (or filename fragment) to match against paths."
                },
                "patterns": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional. Multiple patterns — returns ranked results with why-matched per file."
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory or file to search in. Defaults to the project root."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional glob pattern to filter files (e.g. '*.py', '*.tsx')."
                },
                "mode": {
                    "type": "string",
                    "enum": ["content", "files"],
                    "description": "Optional. 'content' (default) searches file contents with regex. 'files' matches filenames with the glob pattern and returns matching file paths."
                }
            },
            "required": []
        }
    },
    {
        "name": "ask_user",
        "description": "Ask the human a structured question and wait for the answer. Use when the request is ambiguous, a decision is required, or a plan needs approval.",
        "parameters": {
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "One or more questions to ask.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "Stable question id (echoed in the answer)."},
                            "question": {"type": "string", "description": "The question to display."},
                            "detail": {"type": "string", "description": "Optional supporting detail."},
                            "header": {"type": "string", "description": "Optional short heading."},
                            "options": {
                                "type": "array",
                                "description": "Optional selectable answers (label or {label, description}).",
                                "items": {"type": "string"},
                            },
                            "multiSelect": {"type": "boolean", "description": "Allow more than one option (default false)."},
                            "intent": {
                                "type": "object",
                                "description": "Optional presentation intent.",
                                "properties": {
                                    "kind": {"type": "string", "const": "plan-review"},
                                    "approve": {"type": "string", "description": "Label of the option that approves the plan."},
                                },
                            },
                        },
                        "required": ["id", "question"],
                    },
                },
                "timeout": {"type": "integer", "description": "Seconds to wait before aborting (default 600)."},
            },
            "required": ["questions"]
        }
    },
    {
        "name": "browser_act",
        "description": "Browser automation. Actions: navigate, click, fill, type, press, extract, screenshot, close. Selector = CSS or @ref from navigate snapshot. Start server with run_command(background=True) before navigating.",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "fill", "type", "press",
                             "extract", "screenshot", "close"],
                    "description": "The browser action to perform."
                },
                "url": {"type": "string", "description": "URL for 'navigate'."},
                "wait_ms": {"type": "integer", "description": "Optional wait after the action, ms."},
                "selector": {"type": "string", "description": "CSS selector or @ref tag from the navigate snapshot."},
                "text": {"type": "string", "description": "Text for 'fill' / 'type'."},
                "key": {"type": "string", "description": "Key name for 'press'."},
                "what": {"type": "string", "description": "Extraction target for 'extract': text/html/value/title/url."},
                "path": {"type": "string", "description": "Screenshot path for 'screenshot'."}
            },
            "required": ["action"]
        }
    },
    {
        "name": "browser_navigate",
        "description": "Open a URL in a headless Chromium browser and return the page content as an accessibility snapshot with @ref selectors. Handles JavaScript-rendered pages, SPAs, complex web apps. Returns interactive element tree for use with other browser tools. Slower than a plain GET but handles everything.",
        "exposed": False,
        "parameters": {
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The full URL to navigate to."
                },
                "wait_ms": {
                    "type": "integer",
                    "description": "Optional. Milliseconds to wait for page load (default 5000)."
                }
            },
            "required": ["url"]
        }
    },
    {
        "name": "browser_extract",
        "description": "Extract content from the current browser page. Use after browser_navigate. Can extract text, HTML, attribute values, page title, or URL. Uses @ref selectors from the accessibility snapshot.",
        "exposed": False,
        "parameters": {
            "type": "object",
            "properties": {
                "what": {
                    "type": "string",
                    "description": "What to extract: 'text' (default), 'html', 'value', 'title', 'url', 'attr name'."
                },
                "selector": {
                    "type": "string",
                    "description": "Optional CSS selector or @ref from snapshot. Empty = entire page."
                }
            },
            "required": []
        }
    },
    {
        "name": "browser_screenshot",
        "description": "Take a screenshot of the current browser page. Saves to a temp file and returns the path. Use for visual verification of pages.",
        "exposed": False,
        "parameters": {
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Optional file path to save the screenshot. Auto-named if empty."
                }
            },
            "required": []
        }
    },
    {
        "name": "browser_close",
        "description": "Close the current browser session and free all resources (memory, Chrome process). Call when done with web research to avoid resource leaks.",
        "exposed": False,
        "parameters": {
            "type": "object",
            "properties": {},
            "required": []
        }
    },
    {
        "name": "update_feature",
        "description": "Add or update a feature entry in long-term project memory (features.md) after a feature is implemented. file_refs is a list of {\"path\", \"lines\", \"purpose\"} objects.",
        "exposed": False,
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Feature name, e.g. 'User Profile Page'."},
                "description": {"type": "string", "description": "Short description of the feature."},
                "file_refs": {"type": "array", "description": "List of {\"path\", \"lines\", \"purpose\"} file references."}
            },
            "required": ["name"]
        }
    },
    {
        "name": "update_project_meta",
        "description": "Add/update a project metadata section in long-term memory (project.md). Only these keys are allowed: Tech Stack, Frontend, Database, Testing, Architecture, Project Type, Goals.",
        "exposed": False,
        "parameters": {
            "type": "object",
            "properties": {
                "key": {"type": "string", "description": "Metadata key (Tech Stack, Frontend, Database, Testing, Architecture, Project Type, Goals)."},
                "value": {"type": "string", "description": "Metadata value."}
            },
            "required": ["key", "value"]
        }
    },
    # -- GitHub tools (gh CLI bridge) --
    {
        "name": "github_apply_gate",
        "description": "Create a repo and/or install the CI/security gate: seeds security.yml (SAST/SCA/secrets/quality/coverage/semgrep/codeql) + ci.yml, waits for first run, enforces branch protection. NON-DESTRUCTIVE. Stack auto-detected from manifest (auto/python/node/go/php/generic). Confirms by default (gate=ask).",
        "parameters": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "'owner/name' (or a plain name when create=True)."},
                "stack": {"type": "string", "description": "'auto' (default), 'python', 'node', 'go', 'php', or 'generic'."},
                "codeql_on": {"type": "boolean", "description": "Include the CodeQL job (default true; free on public repos)."},
                "create": {"type": "boolean", "description": "True = create the repo first (repo must be a plain name)."},
                "private": {"type": "boolean", "description": "Private repo when create=True."},
                "description": {"type": "string", "description": "Repo description when create=True."},
                "gate": {"type": "string", "description": "'ask' (default — confirm with the user), 'yes' (apply without asking), 'no' (skip the gate)."}
            },
            "required": ["repo"]
        }
    },
    {
        "name": "template_new",
        "description": "Author a new template: write .j2 files into templates/<level>/<name>/files/ with edit_file first, then call this with metadata to write template.json and validate rendering. Levels: stack, feature, component, snippet, project, config, routine, spec.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Lowercase kebab-case template name, e.g. 'feature-billing'."},
                "level": {"type": "string", "description": "Category dir: stack, feature, component, snippet, project, config, routine, spec."},
                "description": {"type": "string", "description": "ONE-line description shown in template_list."},
                "sample_vars": {"type": "object", "description": "Example variables the CI uses to validate rendering, e.g. {'feature_name': 'User'}."},
                "variables": {"type": "array", "description": "Optional variable specs: [{'name': ..., 'type': 'string|int|bool|list', 'required': bool, 'default': ..., 'description': ...}] or plain names ['var_a']."},
                "files": {"type": "array", "description": "Optional file specs [{'src': 'x.py.j2', 'dest': '{{ var }}/x.py'}] — defaults to all *.j2 files in files/ with dest = name minus .j2."},
                "uses": {"type": "array", "description": "Optional composition recipe (template names or {template, variables} dicts)."},
                "notes": {"type": "string", "description": "Optional conventions/adaptation notes shown in the detailed view."}
            },
            "required": ["name", "level", "description", "sample_vars"]
        }
    },
    {
        "name": "create_project",
        "description": "Create a NEW project: creates the folder under D:\\MyProject, registers it as a workspace with its own chat, and assigns a durable Dev agent to it. Use ONLY when the user genuinely asks for a new project. NEVER call this for work on an existing project.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The project/folder name."},
                "concept": {"type": "string", "description": "The user's concept/goal for the project; the Dev agent starts from this."}
            },
            "required": ["name", "concept"]
        }
    },
    {
        "name": "find_project_dev",
        "description": "Resolve an existing project (by name or folder path) and return its durable Dev agent child_id. Prefer resume_project(name, message) for one-call resume; use this when you need the child_id directly.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Project name or folder path."}
            },
            "required": ["name"]
        }
    },
    {
        "name": "update_plan",
        "description": "Update the task plan. Provide an optional explanation and the ENTIRE plan array — this is a full replacement, always send every step. At most one step may be in_progress at a time.",
        "parameters": {
            "type": "object",
            "properties": {
                "explanation": {
                    "type": "string",
                    "description": "Optional explanation for this plan update."
                },
                "plan": {
                    "type": "array",
                    "description": "The complete list of steps.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {"type": "string", "description": "The task step text."},
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Step status: pending, in_progress (at most one), or completed."
                            }
                        },
                        "required": ["step", "status"]
                    }
                }
            },
            "required": ["plan"]
        }
    },
    {
        "name": "task",
        "description": "Delegate a subtask to a specialized subagent (orchestrator only). Spawns a subagent with its own tools and model; returns a compact handoff report.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Subagent type: Dev, FreeAgent, or general-purpose."},
                "task": {"type": "string", "description": "Detailed instruction for the subagent."},
                "reasoning_effort": {"type": "string", "enum": ["max", "high", "medium", "low", "disabled"], "description": "Thinking budget override."}
            },
            "required": ["name", "task"]
        }
    },
    {
        "name": "resume_project",
        "description": "Resume an existing project's durable Dev agent in one call: resolves the project by name/path and delivers the follow-up message. Replaces find_project_dev + send_message + list_agents. If the project is not found, returns [NOT FOUND] with available names.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Project name or folder path (fuzzy match, e.g. 'my-app')."},
                "message": {"type": "string", "description": "Follow-up instruction to deliver to the project's Dev agent."},
                "timeout": {"type": "integer", "description": "Seconds to wait for the reply (default 1800)."}
            },
            "required": ["name", "message"]
        }
    },
    {
        "name": "async_task",
        "description": "Unified async subagent task manager. action=start: fire-and-forget a subagent (like start_async_task) and return task_id. action=status: poll one task, optionally wait (like check_async_task). action=list: list all async tasks and their statuses. Replaces start_async_task / check_async_task / list_async_tasks.",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["start", "status", "list"], "description": "Action to perform: start a new async task, check one task, or list all."},
                "name": {"type": "string", "description": "Subagent type for action=start (e.g. Dev, FreeAgent, general-purpose)."},
                "task": {"type": "string", "description": "Task description for action=start."},
                "task_id": {"type": "string", "description": "Task id for action=status (from start)."},
                "wait_seconds": {"type": "integer", "description": "For action=status: wait up to N seconds (0-300) before returning."}
            },
            "required": ["action"]
        }
    },
    {
        "name": "template",
        "description": "Unified template manager. action=list: catalog; action=search: ranked keyword search; action=show: per-template details; action=apply: render boilerplate into project. Replaces apply_template + template_list.",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "search", "show", "apply"], "description": "Action: list catalog, search by query, show one template, or apply boilerplate."},
                "template": {"type": "string", "description": "Template name for show/apply (e.g. feature-crud)."},
                "category": {"type": "string", "description": "Filter by level for list/search: stack, feature, component, snippet, project, config, routine, spec."},
                "query": {"type": "string", "description": "Keyword search for search (also regex/symbol/semantic)."},
                "target_dir": {"type": "string", "description": "Destination for apply; empty = workspace root."},
                "variables": {"type": "object", "description": "Template variables for apply, e.g. {'feature_name': 'User'}."},
                "overwrite": {"type": "boolean", "description": "For apply: replace existing files (default skip)."},
                "dry_run": {"type": "boolean", "description": "For apply: preview without writing."},
                "quiet": {"type": "boolean", "description": "For apply: compact summary (default true)."}
            },
            "required": ["action"]
        }
    },
]

_TOOL_DOCS = {
    "read_file": "read_file(file_path?, file_paths?, offset?, limit?) — Read file(s). Single file via file_path; multi-file via file_paths=[paths].",
    "edit_file": "edit_file(file_path, old_string?, new_string?, diff?, verify?) — The ONE file tool: new_string only (no old_string) = create/overwrite; old_string+new_string = targeted replace; diff = batch SEARCH/REPLACE blocks. Opt-in verify=py_compile|tsc|shell runs after write. Boilerplate → template(action=\"apply\") first.",
    "run_command": "run_command(command, timeout?, background?, process_action?, shell?, workdir?, expect?, stream?) — Full POSIX bash (default): pipes, &&, $( ), jq, gh pipelines all work. shell='cmd' for legacy, workdir sets cwd, expect checks output regex, stream for live output.",
    "search_code": "search_code(pattern?, patterns?, path?, glob?) — Regex search across files. Multi-pattern via patterns=[...] ranked.",
    "browser_navigate": "browser_navigate(url, wait_ms?) — Open URL in headless Chromium, returns snapshot with @ref selectors.",
    "browser_act": "browser_act(action, selector?, text?, key?, what?, path?) — click / fill / type / press / extract / screenshot / close.",
    "browser_extract": "browser_extract(what?, selector?) — Extract text/html/value from current browser page.",
    "browser_screenshot": "browser_screenshot(path?) — Screenshot of current browser page.",
    "browser_click": "browser_click(selector) — Click an element using CSS selector or snapshot @ref tag.",
    "browser_fill": "browser_fill(selector, text) — Clear and fill an input element.",
    "browser_close": "browser_close() — Close browser session and free resources.",
    "create_docx": "create_docx(file_path, title?, content?) — Create a Word document (.docx).",
    "create_xlsx": "create_xlsx(file_path, headers, rows) — Create an Excel spreadsheet (.xlsx).",
    "read_pdf": "read_pdf(file_path, max_pages?, page_numbers?) — Read text from a PDF file.",
    "task": "task(name, task, reasoning_effort?) — Delegate a subtask to a subagent (delegators only).",
    "ask_user": "ask_user(questions, timeout?) — Ask the human a structured question and wait for the answer (use when the request is ambiguous, a decision is needed, or a plan needs approval via plan-review intent).",
    "github_apply_gate": "github_apply_gate(repo, stack?, codeql_on?, create?, private?, description?, gate?) — The ONE gate tool: create the repo (create=True) and/or install the security/perf gate (seed + branch protection). Confirms with the user (gate: ask|yes|no).",
    "template_new": "template_new(name, level, description, sample_vars, variables?, files?, uses?, notes?) — Author a new template (write .j2 files first, then metadata; auto-validates render).",
    "create_project": "create_project(name, concept) — Create a NEW project: folder under D:\\MyProject + workspace + chat + a durable Dev agent assigned to it. ONLY for genuinely new projects requested by the user.",
    "find_project_dev": "find_project_dev(name) — Resolve an existing project and return its Dev agent child_id. Prefer resume_project for one-call resume.",
    "resume_project": "resume_project(name, message, timeout?) — One-call resume: find the project's Dev and deliver the message (replaces find_project_dev + send_message).",
    "async_task": "async_task(action, name?, task?, task_id?, wait_seconds?) — Unified async: start/status/list (replaces start_async_task/check_async_task/list_async_tasks).",
    "template": "template(action, template?, category?, query?, target_dir?, variables?, overwrite?, dry_run?, quiet?) — Unified template: list/search/show/apply (replaces apply_template + template_list).",
    "update_plan": "update_plan(explanation?, plan=[{step, status}]) — Maintain the task plan. FULL replacement: always send every step. At most one step in_progress.",
}

_TOOL_REQUIRED_ARGS: dict[str, set[str]] = {
    d["name"]: set((d.get("parameters") or {}).get("required", []))
    for d in TOOL_DEFINITIONS
}


def get_native_tools(scope: Optional[str] = None) -> list[dict]:
    """Return tools as DeepSeek/OpenAI-native function definitions,
    optionally filtered by scope."""
    tools = []
    allowed = set(_SCOPE_MAP[scope.lower()]) if scope and scope.lower() in _SCOPE_MAP else None
    for d in TOOL_DEFINITIONS:
        if d.get("exposed") is False:
            continue
        if allowed is not None and d["name"] not in allowed:
            continue
        tools.append({
            "type": "function",
            "function": {
                "name": d["name"],
                "description": d.get("description", ""),
                "parameters": d.get("parameters") or {
                    "type": "object",
                    "properties": {},
                    "required": [],
                },
            },
        })
    try:
        from tools.mcp.mcp_bridge import mcp_tool_defs_for_scope
        tools.extend(mcp_tool_defs_for_scope(scope or ""))
    except Exception:
        pass
    return sorted(tools, key=lambda t: t["function"]["name"])

_SCOPE_MAP: dict[str, list[str]] = {
    "developer": [
        "read_file", "edit_file", "run_command", "search_code",
        "task", "browser_act", "template",
        "ask_user", "github_apply_gate", "update_plan",
    ],
    "orchestrator": [
        "run_command",
        "create_project", "resume_project",
        "ask_user",
        "task", "async_task",
        "update_plan",
    ],
    "free_agent": [
        "edit_file", "run_command",
        "read_pdf", "create_docx", "create_xlsx", "ask_user",
    ],
}


def get_native_tool_names(scope: Optional[str] = None) -> list[str]:
    """Return tool names, optionally filtered by scope."""
    if scope and scope.lower() in _SCOPE_MAP:
        allowed = set(_SCOPE_MAP[scope.lower()])
        names = [d["name"] for d in TOOL_DEFINITIONS if d["name"] in allowed and d.get("exposed") is not False]
    else:
        names = [d["name"] for d in TOOL_DEFINITIONS if d.get("exposed") is not False]
    try:
        from tools.mcp.mcp_bridge import mcp_tool_names_for_scope
        names.extend(mcp_tool_names_for_scope(scope or ""))
    except Exception:
        pass
    return names


def get_compact_tool_docs(scope: Optional[str] = None, names: Optional[list[str]] = None) -> dict[str, str]:
    """Return compact tool docstrings dictionary."""
    target_names = set(names) if names else set(get_native_tool_names(scope))
    return {k: v for k, v in _TOOL_DOCS.items() if k in target_names}


def format_compact_tool_help(names: Optional[list[str]] = None, scope: Optional[str] = None) -> str:
    """Format compact tool help lines for prompt injection or diagnostic logging."""
    docs = get_compact_tool_docs(scope=scope, names=names)
    lines = [f"- {v}" for v in docs.values()]
    try:
        from tools.mcp.mcp_bridge import mcp_instructions_for_scope
        for server, instr in mcp_instructions_for_scope(scope or ""):
            lines.append(f"- [{server} MCP] {instr}")
    except Exception:
        pass
    return "\n".join(lines)
