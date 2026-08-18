"""Central model-facing tool schemas and compact tool documentation.

The facade imports these values back so existing callers continue to use
tools.TOOL_DEFINITIONS, tools._TOOL_DOCS, and tools._TOOL_REQUIRED_ARGS.
"""

TOOL_DEFINITIONS = [
    {
        "name": "read_file",
        "description": "Reads a file from the local filesystem. Use this to inspect code, configs, or any project file.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to read."
                },
                "offset": {
                    "type": "integer",
                    "description": "Optional. Line number to start reading from."
                },
                "limit": {
                    "type": "integer",
                    "description": "Optional. Maximum number of lines to read."
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "edit_file",
        "description": "Edit an existing file using SEARCH/REPLACE blocks. Supports single or multiple blocks in one call. PREFERRED method for all file edits.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file to modify."
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
                    "description": "One or more SEARCH/REPLACE blocks (batch edit mode). Format: <<<<<<< SEARCH\\n[old]\\n=======\\n[new]\\n>>>>>>> REPLACE"
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "run_command",
        "description": "Executes a shell command. Can be run in foreground (blocking, streams logs in real-time) or background (non-blocking, returns immediately). Use process_action parameter to list, view logs, or kill background processes.",
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
                }
            },
            "required": []
        }
    },
    {
        "name": "search_code",
        "description": "Searches file contents using Python regex (ripgrep-style patterns). Use this to find functions, classes, imports, or patterns across the codebase.",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for in file contents."
                },
                "path": {
                    "type": "string",
                    "description": "Optional directory or file to search in. Defaults to the project root."
                },
                "glob": {
                    "type": "string",
                    "description": "Optional glob pattern to filter files (e.g. '*.py', '*.tsx')."
                }
            },
            "required": ["pattern"]
        }
    },
    {
        "name": "ask_user",
        "description": "Ask the human a structured question and wait for the answer. Use when the request is ambiguous, a decision is required, or a plan needs approval (plan-review intent).",
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
                "timeout": {"type": "integer", "description": "Optional seconds to wait before aborting (default 600)."},
            },
            "required": ["questions"]
        }
    },
    {
        "name": "send_message",
        "description": "Deliver a follow-up turn to a continuable subagent. Resumes it from its durable session if idle; queues the message if it is running.",
        "parameters": {
            "type": "object",
            "properties": {
                "child_id": {"type": "string", "description": "Child id from list_agents()."},
                "message": {"type": "string", "description": "The follow-up instruction."},
                "timeout": {"type": "integer", "description": "Optional seconds to wait for the reply (default 1800)."},
            },
            "required": ["child_id", "message"]
        }
    },
    {
        "name": "list_agents",
        "description": "List continuable subagents and their statuses (running/idle). Durable enumeration — no agent is resumed.",
        "parameters": {"type": "object", "properties": {}, "required": []}
    },
    {
        "name": "interrupt_agent",
        "description": "Request a running continuable subagent to stop at its next step. Queued messages are kept.",
        "parameters": {
            "type": "object",
            "properties": {
                "child_id": {"type": "string", "description": "Child id from list_agents()."},
            },
            "required": ["child_id"]
        }
    },
    {
        "name": "report",
        "description": "Report progress back to the parent agent (continuable subagents only). Default wakes the parent; quiet=True records without waking.",
        "parameters": {
            "type": "object",
            "properties": {
                "message": {"type": "string", "description": "The report content."},
                "quiet": {"type": "boolean", "description": "Record without waking the parent (default false)."},
            },
            "required": ["message"]
        }
    },
    {
        "name": "browser_act",
        "description": "The ONLY browser tool. Actions: navigate → click → fill/type → press → extract → screenshot → close (selector = CSS or @ref tag from the navigate snapshot). Start your app with run_command background=True BEFORE navigating; assert the page shows what you built; close the session when done.",
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
        "description": "The ONE gate tool: optionally create the repo (create=True, plain name) and install the security/perf gate: seed the profile's workflow files (security.yml with SAST/SCA/secrets/quality/coverage/perf/semgrep/container/codeql/zizmor + ci.yml), wait for the first run, then enforce branch protection (merge blocked until all checks pass). NON-DESTRUCTIVE: existing project files are never overwritten. Confirms with the user by default (gate: 'ask'|'yes'|'no'). stack: 'auto' (detect: package.json→node, composer.json→php, go.mod→go, pyproject/requirements→python, else generic). NO templates are applied — templates are only used when a task needs them.",
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
        "name": "apply_template",
        "description": "Write a template's boilerplate files into the project (Jinja2 render). USE THIS for repetitive file creation (CRUD features, CI configs, project scaffolds, spec docs, test suites) — it replaces writing boilerplate token-by-token with one tool call. Two-phase: everything renders before anything is written (no partial applies). Existing files are SKIPPED by default (safe re-applies); overwrite=true replaces them. Returns created files + TODO markers + conventions (quiet=true default = compact summary).",
        "parameters": {
            "type": "object",
            "properties": {
                "template": {"type": "string", "description": "Template name (see template_list)."},
                "target_dir": {"type": "string", "description": "Destination directory; empty = workspace root."},
                "variables": {"type": "object", "description": "Template variables, e.g. {'feature_name': 'User', 'model_fields': 'name:string,email:string'}. Check template_list(template=...) for the required set."},
                "overwrite": {"type": "boolean", "description": "True = replace files that already exist (default False = skip them)."},
                "dry_run": {"type": "boolean", "description": "True = render and preview without writing anything."},
                "quiet": {"type": "boolean", "description": "True (default) = compact summary (counts + file lists). False = verbose (per-template nesting, TODOs, notes)."}
            },
            "required": ["template"]
        }
    },
    {
        "name": "template_list",
        "description": "List/search templates. No args = full catalog; query=keyword search across name/description/category; category=filter by level; template=name shows variables, files, example call and notes for one template.",
        "parameters": {
            "type": "object",
            "properties": {
                "template": {"type": "string", "description": "Template name for the detailed view (variables, files, notes)."},
                "category": {"type": "string", "description": "Filter by level/category: stack, feature, component, snippet, project, config, routine, spec."},
                "query": {"type": "string", "description": "Keyword search (case-insensitive) across template name, category and description."}
            },
            "required": []
        }
    },
    {
        "name": "template_new",
        "description": "Author a NEW template into templates/<level>/<name>/. Write the .j2 boilerplate files into templates/<level>/<name>/files/ with edit_file FIRST, then call this tool with the metadata — it writes template.json and validates the new template renders with sample_vars. Levels: stack, feature, component, snippet, project, config, routine, spec.",
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
        "description": "Resolve an existing project (by name or folder path) and return its durable Dev agent child_id. Use before send_message so follow-up coding work continues the SAME Dev session instead of spawning a new one.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Project name or folder path."}
            },
            "required": ["name"]
        }
    },
]

_TOOL_DOCS = {
    "read_file": "read_file(file_path, offset?, limit?) â€” Read file.",
    "edit_file": "edit_file(file_path, old_string?, new_string?, diff?) — The ONE file tool: new_string only (no old_string) = create/overwrite; old_string+new_string = targeted replace; diff = batch SEARCH/REPLACE blocks. Boilerplate → apply_template first.",
    "run_command": "run_command(command, timeout?, background?, process_action?, shell?) — Full POSIX bash (default): pipes, &&, $( ), jq, gh pipelines all work. shell='cmd' for legacy plain cmd. Run tests, git, gh, rg, curl, compilers here.",
    "search_code": "search_code(pattern, path?, glob?) â€” Regex search across files.",
    "browser_navigate": "browser_navigate(url, wait_ms?) â€” Open URL in headless Chromium, returns snapshot with @ref selectors.",
    "browser_act": "browser_act(action, selector?, text?, key?, what?, path?) â€” click / fill / type / press / extract / screenshot / close.",
    "browser_extract": "browser_extract(what?, selector?) â€” Extract text/html/value from current browser page.",
    "browser_screenshot": "browser_screenshot(path?) â€” Screenshot of current browser page.",
    "browser_click": "browser_click(selector) â€” Click an element using CSS selector or snapshot @ref tag.",
    "browser_fill": "browser_fill(selector, text) â€” Clear and fill an input element.",
    "browser_close": "browser_close() â€” Close browser session and free resources.",
    "create_docx": "create_docx(file_path, title?, content?) â€” Create a Word document (.docx).",
    "create_xlsx": "create_xlsx(file_path, headers, rows) â€” Create an Excel spreadsheet (.xlsx).",
    "read_pdf": "read_pdf(file_path, max_pages?, page_numbers?) â€” Read text from a PDF file.",
    "task": "task(name, task, reasoning_effort?) â€” Delegate a subtask to a subagent (delegators only).",
    "start_async_task": "start_async_task(name, task) â€” Fire-and-forget parallel subagent; returns a task_id.",
    "check_async_task": "check_async_task(task_id, wait_seconds?) â€” Poll an async task; optionally wait.",
    "list_async_tasks": "list_async_tasks() â€” List async tasks and statuses.",
    "ask_user": "ask_user(questions, timeout?) â€” Ask the human a structured question and wait for the answer (use when the request is ambiguous, a decision is needed, or a plan needs approval via plan-review intent).",
    "send_message": "send_message(child_id, message) â€” Follow up with a continuable subagent (resumes it from its durable session if idle; queues the message if it is running).",
    "list_agents": "list_agents() â€” List your continuable subagents and their statuses.",
    "interrupt_agent": "interrupt_agent(child_id) â€” Request a running continuable subagent to stop at its next step (queued messages are kept).",
    "report": "report(message, quiet?) â€” Report progress back to the parent agent. Default wakes the parent; quiet=1 only records it.",
    "github_apply_gate": "github_apply_gate(repo, stack?, codeql_on?, create?, private?, description?, gate?) — The ONE gate tool: create the repo (create=True) and/or install the security/perf gate (seed + branch protection). Confirms with the user (gate: ask|yes|no).",
    "apply_template": "apply_template(template, target_dir?, variables?, overwrite?, dry_run?, quiet?) â€” Write a template's boilerplate files (Jinja2 render, sandboxed). Two-phase render-then-write; existing files skipped by default (overwrite=true to replace); dry_run=true previews; quiet=true (default) compact summary. ~30 tokens vs writing boilerplate manually.",
    "template_list": "template_list(template?, category?, query?) â€” No arg = catalog; query= ranked search (regex, words, code symbols, semantic); template name = variables + files + example + notes.",
    "template_new": "template_new(name, level, description, sample_vars, variables?, files?, uses?, notes?) â€” Author a new template (write .j2 files first, then metadata; auto-validates render).",
    "create_project": "create_project(name, concept) â€” Create a NEW project: folder under D:\\MyProject + workspace + chat + a durable Dev agent assigned to it. ONLY for genuinely new projects requested by the user.",
    "find_project_dev": "find_project_dev(name) â€” Resolve an existing project and return its Dev agent child_id (then send_message to it). Never spawn a duplicate Dev for an existing project.",
}

_TOOL_REQUIRED_ARGS: dict[str, set[str]] = {
    d["name"]: set((d.get("parameters") or {}).get("required", []))
    for d in TOOL_DEFINITIONS
}


def get_native_tools() -> list[dict]:
    """Return all exposed tools as DeepSeek/OpenAI-native function definitions,
    derived from TOOL_DEFINITIONS."""
    tools = []
    for d in TOOL_DEFINITIONS:
        if d.get("exposed") is False:
            continue
        tools.append({
            "type": "function",
            "function": {
                "name": d["name"],
                "parameters": d.get("parameters") or {
                    "type": "object",
                    "properties": {},
                    "required": [],
                },
            },
        })
    return sorted(tools, key=lambda t: t["function"]["name"])
