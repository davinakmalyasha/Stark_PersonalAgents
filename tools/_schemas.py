import os

from typing import Optional

"""Central model-facing tool schemas and compact tool documentation.

The facade imports these values back so existing callers continue to use
tools.TOOL_DEFINITIONS, tools._TOOL_DOCS, and tools._TOOL_REQUIRED_ARGS.
"""

TOOL_DEFINITIONS = [
    {
        "name": "read_file",
        "description": "Read one file, or several in ONE call via file_paths (each gets its own [FILE] section; offset/limit apply to every file in the batch) — when 2+ files are needed, batch them in a single call to save turns. A full read returns the first page under a [FILE] header (lines x-y of N); when PARTIAL is shown, page onward with offset/limit. A repeat full read of an unchanged file returns a cached stub — use offset/limit to re-read a specific slice.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Single file path."},
                "file_paths": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Batch of file paths to read in one call."
                },
                "offset": {"type": "integer", "description": "Start line (1-based; 0 = first line)."},
                "limit": {"type": "integer", "description": "Max lines."}
            },
            "required": []
        }
    },
    {
        "name": "outline",
        "description": "Signature outline of ONE file — every function/class/method with its definition line, bodies omitted (~1/10 the tokens of a full read). Use before read_file: pick the symbol here, then read_file(offset=line, limit=N) for just its body.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "File to outline."}
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "edit_file",
        "description": "Edit or create files. Mode A — targeted change: old_string -> new_string (must match exactly and uniquely; identical strings error). Mode B — create or overwrite a WHOLE file: file_path + new_string only. Mode C — batch/multi-file diff: '*** Add File: p' / '*** Update File: p' / '*** Delete File: p' / '*** Move to: p' headers, optionally wrapped in '*** Begin Patch' / '*** End Patch'. Update bodies use @@ hunks: lines prefixed '-' are removed, '+' are added, and ' ' (space) are unchanged context lines that anchor the hunk — line numbers are optional and ignored, so hunks tolerate drift (preferred over SEARCH/REPLACE for multi-hunk and multi-file edits). Applied transactionally with [OK]/[ERROR] summary. verify = 'py_compile' | 'tsc' | shell command, runs after the write.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string", "description": "Exact existing text (Mode A)."},
                "new_string": {"type": "string", "description": "Replacement (Mode A) or full file content (Mode B: omit old_string)."},
                "diff": {"type": "string", "description": "Mode C payload: '*** File:' headers (optional *** Begin Patch / *** End Patch envelope) with @@ hunks — '-' removed, '+' added, ' ' context anchors; line numbers optional and ignored. Legacy SEARCH/REPLACE blocks and numbered unified diffs are also accepted."},
                "verify": {"type": "string", "description": "'py_compile' | 'tsc' | shell command — runs after write."}
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "run_command",
        "description": "Run a shell command (Git Bash, POSIX). Foreground blocks and streams output; server-like commands (npm run dev, vite, uvicorn, python -m http.server...) that exceed the timeout are auto-moved to background. Background runs return a process name managed via process_action. Windows tools work as subprocesses: powershell.exe -NoProfile -Command '...' or cmd.exe /c '...'.",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command; also names the process for process_action='logs'|'kill'|'wait'."},
                "timeout": {"type": "integer", "description": "Wall-clock limit in ms (default 120000, max 600000; values <1000 are seconds). Use >=60000 for installs; background=true for servers."},
                "idle_timeout": {"type": "integer", "description": "Kill after this long with no output and no CPU activity, in ms (default 60000; 0 disables). Server-like/busy commands are backgrounded instead of killed."},
                "background": {"type": "boolean"},
                "process_action": {"type": "string", "enum": ["list", "logs", "kill", "wait", "ready", "send", "kill_all"], "description": "'list' | 'logs' | 'kill' | 'wait' | 'ready' | 'send' | 'kill_all' (command carries the process name)."},
                "workdir": {"type": "string", "description": "Working directory (default project root)."},
                "expect": {"type": "string", "description": "Regex expected in output; typed pass/fail. For background starts it is awaited up to ready_timeout."},
                "tail_lines": {"type": "integer", "description": "Lines returned by process_action='logs' (default 200)."},
                "ready_timeout": {"type": "integer", "description": "Background readiness wait in ms (default 15000; 0 = don't wait). Also used by process_action='ready'."},
                "input": {"type": "string", "description": "process_action='send': text written to the background process stdin."}
            },
            "required": []
        }
    },
    {
        "name": "read_pdf",
        "description": "Read text from a PDF file, optionally limited to specific pages.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Path to the PDF file."},
                "max_pages": {"type": "integer", "description": "Max pages to read (default 20; 0 = all)."},
                "page_numbers": {"type": "string", "description": "Specific pages to read, e.g. '1-3,5,7-9' (overrides max_pages)."}
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "create_docx",
        "description": "Create a Word document (.docx) with an optional title heading and paragraph content (one paragraph per line).",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Output .docx path."},
                "title": {"type": "string", "description": "Optional document title (heading level 1)."},
                "content": {"type": "string", "description": "Document body — one paragraph per line."}
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "create_xlsx",
        "description": "Create an Excel spreadsheet (.xlsx) from headers + rows.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string", "description": "Output .xlsx path."},
                "headers": {"type": "array", "items": {"type": "string"}, "description": "Optional header row."},
                "rows": {
                    "type": "array",
                    "items": {"type": "array", "items": {}},
                    "description": "Data rows (each row is an array of cell values)."
                }
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "search_code",
        "description": "Regex-search file contents ('content', default) or match path globs ('files'). Output styles: lines (default), files (matching paths), count (matches per file). Matching lines are annotated with their enclosing symbol unless annotate=false; snippet='function' returns the enclosing function body instead of single lines. Multi-pattern results ranked via patterns=[...]. Results are workspace-relative; up to max_matches lines shown (default 100; full list saved to /scratch when exceeded). Narrow with path/glob.",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex (content mode) or glob/filename fragment (files mode)."},
                "patterns": {"type": "array", "items": {"type": "string"}, "description": "Multi-pattern ranked search (replaces pattern)."},
                "path": {"type": "string", "description": "Directory or single file to search (default: workspace root)."},
                "glob": {"type": "string", "description": "Comma-separated file filters, '!' excludes; e.g. '*.py,*.ts,!*.test.*'."},
                "mode": {"type": "string", "enum": ["content", "files"], "description": "'content' regex search or 'files' filename search."},
                "output": {"type": "string", "enum": ["lines", "files", "count"], "description": "Content answer style: matching lines, matching paths, or matches per file."},
                "ignore_case": {"type": "boolean", "description": "Case-insensitive matching."},
                "smart_case": {"type": "boolean", "description": "Ignore case when the pattern has no uppercase letters."},
                "word": {"type": "boolean", "description": "Match whole words only."},
                "fixed_strings": {"type": "boolean", "description": "Treat the pattern as a literal string, not a regex."},
                "context": {"type": "integer", "minimum": 0, "maximum": 5, "description": "Lines of context around each match (lines output)."},
                "max_matches": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Matching lines shown inline (default 100)."},
                "annotate": {"type": "boolean", "description": "Prefix each match with its enclosing symbol (default true)."},
                "snippet": {"type": "string", "enum": ["line", "function"], "description": "line (default) or function: show the enclosing function body once per function."}
            },
            "required": []
        }
    },
    {
        "name": "ast_search",
        "description": "Structural search over Python files (stdlib AST, no text matching). Warehouse tool: not assigned to any agent scope yet; enable per specialist. Results are workspace-relative and bounded like search_code.",
        "parameters": {
            "type": "object",
            "properties": {
                "rule": {"type": "string", "enum": ["call", "import", "class_bases", "decorator", "bare_except", "long_function", "many_params", "no_docstring"], "description": "Structural query. call/import/class_bases/decorator require name; long_function/many_params use min_lines/max_params."},
                "name": {"type": "string", "description": "Function/method/module/class/decorator name for name-based rules (dotted names allowed)."},
                "path": {"type": "string", "description": "Directory or single file to search (default: workspace root)."},
                "glob": {"type": "string", "description": "Comma-separated file filters, '!' excludes. Python files only."},
                "min_lines": {"type": "integer", "minimum": 1, "description": "long_function threshold in lines (default 60)."},
                "max_params": {"type": "integer", "minimum": 0, "description": "many_params threshold; functions above it are reported (default 5)."},
                "max_results": {"type": "integer", "minimum": 1, "maximum": 1000, "description": "Matches shown inline (default 100)."}
            },
            "required": ["rule"]
        }
    },
    {
        "name": "ask_user",
        "description": "Ask the caller structured questions and wait for answers. Use on material ambiguity, a decision only the caller can make, or plan approval. Each question needs 2-4 options (use multiSelect=true if more than one may apply). For plan-approval flows, set intent.kind=\"plan-review\" on the question and put the approving option's label in intent.approve.",
        "parameters": {
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "Stable identifier for this question. Used to reference the answer in your next reply."},
                            "question": {"type": "string"},
                            "detail": {"type": "string"},
                            "header": {"type": "string", "description": "Short chip label shown above the question, ≤12 chars. Example: 'Approve plan'."},
                            "options": {"type": "array", "minItems": 2, "items": {"type": "string"}, "description": "2-4 choices shown to the user. The first option is the recommended one."},
                            "multiSelect": {"type": "boolean", "description": "true = the user can pick multiple options. false (default) = pick exactly one."},
                            "intent": {
                                "type": "object",
                                "properties": {
                                    "kind": {"type": "string", "const": "plan-review"},
                                    "approve": {"type": "string", "description": "Option label that approves."},
                                },
                            },
                        },
                        "required": ["id", "question"],
                    },
                },
                "timeout": {"type": "integer", "default": 600, "description": "Seconds to wait before aborting."},
            },
            "required": ["questions"]
        }
    },
    {
        "name": "browser_act",
        "description": "Browser automation: navigate, click, fill, type, press, extract, screenshot, close. Selector = CSS or @ref from the navigate snapshot. Start servers with run_command(background=true) first.",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["navigate", "click", "fill", "type", "press",
                             "extract", "screenshot", "close"]
                },
                "url": {"type": "string"},
                "wait_ms": {"type": "integer", "description": "Wait after action, ms."},
                "selector": {"type": "string", "description": "CSS or @ref from snapshot."},
                "text": {"type": "string"},
                "key": {"type": "string"},
                "what": {"type": "string", "description": "'text'|'html'|'value'|'title'|'url'."},
                "path": {"type": "string", "description": "Screenshot save path."}
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
        "name": "look_at_screen",
        "description": "Capture the user's live screen RIGHT NOW and attach it inline (vision). Use when the answer depends on what the user is currently viewing: a highlighted passage, a chart, a page, an error, or when they say 'this'/'here'/'what I'm looking at'. Turns the overlay's screen sharing on if it is off. The frame reaches you as an image before your next reply.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": []
        }
    },
    {
        "name": "set_screen_share",
        "description": "Change the live overlay's screen-sharing mode: 'on' (preview + one fresh frame attached to each voice turn), 'off' (stop sharing), 'continuous' (also refresh the latest frame every few seconds so look_at_screen is near-real-time). Ask the user before turning 'continuous' on; turn it off when the task is done.",
        "parameters": {
            "type": "object",
            "properties": {
                "mode": {
                    "type": "string",
                    "enum": ["on", "off", "continuous"],
                    "description": "Desired sharing mode."
                }
            },
            "required": ["mode"]
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
        "description": "Create a repo and/or install the tiered CI/security gate: T1 fast-checks / T2 staging / T3 release workflows + .harness-ci.json, dev/staging/prod branch ladder, waits for first run, enforces prod branch protection. Non-destructive otherwise; stack auto-detected. Confirms first (gate=ask).",
        "parameters": {
            "type": "object",
            "properties": {
                "repo": {"type": "string", "description": "'owner/name' (plain name when create=true)."},
                "stack": {"type": "string", "description": "'auto'(default) | python | node | go | php | generic."},
                "create": {"type": "boolean"},
                "private": {"type": "boolean"},
                "description": {"type": "string"},
                "gate": {"type": "string", "description": "'ask'(default) | 'yes' | 'no'."},
                "update_levels": {"type": "boolean", "description": "Re-render level-owned files after a branch rename."}
            },
            "required": ["repo"]
        }
    },
    {
        "name": "create_project",
        "description": "Create a NEW project: creates the folder (under D:\\MyProject, or under parent_root if given), registers it as a workspace with its own chat, and assigns a durable Dev agent to it. Use ONLY when the user genuinely asks for a new project. NEVER call this for work on an existing project. Pass wait=false when creating SEVERAL projects in one burst — their first Dev turns run in parallel in the background. The DFD agent MUST pass parent_root (the client folder under D:\\MyProject\\DFDClientProjects) so client projects always live inside the client root.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Project/folder name. 1-64 chars; filesystem-safe — avoid '/', '\\\\', '..', control characters, and leading dots. Use kebab-case or snake_case."},
                "concept": {"type": "string", "description": "The user's concept/goal for the project; the Dev agent starts from this."},
                "wait": {"type": "boolean", "default": True, "description": "Wait for the Dev's first turn before returning. Use false for multi-project bursts."},
                "parent_root": {"type": "string", "description": "Optional parent folder. DFD agent: the client folder under D:\\MyProject\\DFDClientProjects (enforced to stay inside it). Others: leave empty."}
            },
            "required": ["name", "concept"]
        }
    },
    {
        "name": "update_plan",
        "description": "Maintain the plan dock. Call when jobs change (spawn, progress, abort). FULL REPLACEMENT each call: send every step with its current status; at most one in_progress. Settled jobs use status=completed — put the reason in the step text when not successful.",
        "parameters": {
            "type": "object",
            "properties": {
                "explanation": {"type": "string", "description": "Short note shown above the plan (e.g. 'Spawned 3 jobs, waiting for results'). Keep to one line."},
                "plan": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {"type": "string", "minLength": 1, "description": "Short, human-readable job description. For settled jobs, prefix the reason when not successful (e.g. 'Build snake game — failed: tests did not pass')."},
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": False
                    }
                }
            },
            "required": ["plan"]
        }
    },
    {
        "name": "propose_plan",
        "description": "Present the plan for the CURRENT plan-mode review. Plan mode is user-initiated via the prompt dock ONLY — never call this to start planning on your own, never suggest switching modes. Outside plan mode this is a no-op (use update_plan and keep working). The agent CANNOT change plan/build mode in either direction.",
        "parameters": {
            "type": "object",
            "properties": {
                "explanation": {"type": "string", "description": "Short note shown above the plan (e.g. 'Here is the plan to fix the crash'). Keep to one line."},
                "rationale": {"type": "string", "description": "Why this approach (1-3 sentences). Shown in the plan-review UI."},
                "plan": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": {"type": "string", "minLength": 1, "description": "Short, human-readable step. For settled jobs, prefix the reason when not successful."},
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
                            }
                        },
                        "required": ["step", "status"],
                        "additionalProperties": False
                    }
                }
            },
            "required": ["plan"]
        }
    },
    {
        "name": "delegate",
        "description": "Spawn one or more workspace agents (Dev / General / House) IN PARALLEL. Every item gets its own visible chat; final replies arrive as [DELEGATION RESULT] events while you keep coordinating.\n\nBATCHING — per user turn: put EVERY independent job from the current request into a SINGLE delegate(items=[...]) call. Any agent mix is allowed and runs in parallel — solo scope (just Dev, just House, or just General) or mixed (Dev + House, Dev + General, all three). The sole batching criterion is independence. If job B needs job A's output, do NOT bundle B with A: delegate A first, wait for its [DELEGATION RESULT], then delegate B with A's result in the task. Never split a single request across multiple delegate calls for tidiness or sequencing — one call, all independent items.\n\nCHAT CONTINUATION — chat_id is optional. Omit it for a new job (default). Set it ONLY when this item is a follow-up, correction, or bug on a chat you can identify from a prior [DELEGATION RESULT] in this session. If the chat_id you pass does not exist, the runtime silently starts a new chat — so never guess an id.",
        "parameters": {
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "description": "Delegation items to launch concurrently.",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent": {"type": "string", "enum": ["Dev", "General", "House"], "description": "Target workspace agent. Dev builds/fixes software; General handles research, docs, images, personal notes/admin; House controls smart-home devices. Case-insensitive."},
                            "task": {"type": "string", "description": "Complete, self-contained instruction: goal, relevant paths/targets, constraints, and how to verify success. The agent sees only this text. Context, not method — do not prescribe step-by-step process."},
                            "title": {"type": "string", "description": "Short label for the delegation chat (e.g. 'Space game · Dev')."},
                            "chat_id": {"type": "string", "description": "Omit for a new chat. Set ONLY with a chat_id surfaced by an earlier [DELEGATION RESULT] in this session — never guess. Unknown ids silently start a new chat."},
                            "workspace": {"type": "string", "description": "Workspace id, folder path, or name. Omit to use the default: current project for Dev, General workspace for General, House workspace for House."},
                            "images": {"type": "array", "items": {"type": "string"}, "description": "Absolute file paths of image attachments relevant to THIS item. Missing files are silently dropped, so only attach paths you have verified exist."}
                        },
                        "required": ["agent", "task"],
                        "additionalProperties": False
                    },
                    "examples": [
                        {
                            "items": [
                                {"agent": "Dev", "task": "Build a snake game in Python with tests", "title": "Snake game · Dev"},
                                {"agent": "House", "task": "Turn off the living room lights", "title": "Lights off · House"},
                                {"agent": "General", "task": "Save a journal note: shipped v0.1", "title": "Note · General"}
                            ]
                        },
                        {
                            "items": [
                                {"agent": "Dev", "task": "Fix the login crash (regression from PR #42)", "title": "Login crash fix", "chat_id": "<id from prior [DELEGATION RESULT]>"}
                            ]
                        }
                    ]
                }
            },
            "required": ["items"]
        }
    },
    {
        "name": "delegation_status",
        "description": "List all delegations spawned this session with live statuses (running / done / failed) and their chat ids. Call when you need to know what is still outstanding — typically before a wrap-up summary, after a long wait, or when you are unsure whether everything has settled.",
        "parameters": {
            "type": "object",
            "properties": {},
            "required": []
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
        "name": "call_subagent",
        "description": "Specialist subagent lifecycle (see Subagents list). action=run: spawn and block until the final report (default). action=spawn: start it in the background and keep working on other parts, then collect with status. action=status: status + latest result of a spawned child (wait_seconds≤300). action=message: steer a running child (queued) or send a follow-up. action=stop: interrupt a running child. action=list: this session's subagents. Prefer spawn+status for deep/long verification so you are not blocked.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Subagent type from the Subagents list, e.g. UI | Deploy. Required for run/spawn."},
                "task": {"type": "string", "description": "Goal + targets (files/URL/repo) + acceptance check + constraints. Context, not method — do not prescribe design direction or step-by-step process. Required for run/spawn."},
                "action": {"type": "string", "enum": ["run", "spawn", "status", "message", "stop", "list"], "default": "run", "description": "Lifecycle action (default run)."},
                "child_id": {"type": "string", "description": "Spawned child id (from action=spawn) for status/message/stop."},
                "message": {"type": "string", "description": "Follow-up/steering message for action=message."},
                "wait_seconds": {"type": "integer", "description": "For status: poll up to this many seconds (max 300) before returning.", "default": 0}
            },
            "required": []
        }
    },
    {
        "name": "files_changed",
        "description": "Stage 5 (advisory): list the most recent external file changes in the active workspace. The agent can call this to notice when the user (or another tool outside this run) edits a file. Does NOT invalidate the read cache or interrupt a running tool. Requires the optional `watchfiles` package; if not installed, returns a friendly 'not available' string.",
        "parameters": {
            "type": "object",
            "properties": {
                "limit": {"type": "integer", "description": "Max events to return (1-50, default 5).", "minimum": 1, "maximum": 50, "default": 5}
            }
        }
    },
    # -- House tools (mock smart home; single source of truth = house_manager) --
    {
        "name": "house_get_state",
        "description": "Read the full house state: rooms, every device with its current state, scenes, energy and security summary. Call this first whenever the user asks about the house.",
        "parameters": {"type": "object", "properties": {}, "required": []}
    },
    {
        "name": "house_get_device",
        "description": "Read one house device: current state, writable capabilities, room and online status. Use to check a device before or after actuating it.",
        "parameters": {
            "type": "object",
            "properties": {
                "device_id": {"type": "string", "description": "Device id, e.g. 'light-living-1', 'thermostat-bedroom', 'lock-garage'."}
            },
            "required": ["device_id"]
        }
    },
    {
        "name": "house_set_device",
        "description": "Control a house device. state keys depend on type: light {power, brightness 0-100, colorTemp 2200-6500}; thermostat {targetTemp 10-35, mode off|heat|cool|auto}; lock {locked}; plug {power}; blinds {position 0-100}; camera {recording}. Sensors are read-only.",
        "parameters": {
            "type": "object",
            "properties": {
                "device_id": {"type": "string", "description": "Device id to actuate."},
                "state": {"type": "object", "description": "State patch, e.g. {\"power\": true, \"brightness\": 60} or {\"targetTemp\": 21.5}."}
            },
            "required": ["device_id", "state"]
        }
    },
    {
        "name": "house_run_scene",
        "description": "Activate a house scene (applies all its device actions at once). Available scenes are listed in house_get_state, e.g. scene-morning, scene-night, scene-away, scene-movie, scene-focus.",
        "parameters": {
            "type": "object",
            "properties": {
                "scene_id": {"type": "string", "description": "Scene id, e.g. 'scene-night'."}
            },
            "required": ["scene_id"]
        }
    },
    {
        "name": "house_get_history",
        "description": "Mock energy-usage history for one house device (hourly kWh series, up to 168 hours).",
        "parameters": {
            "type": "object",
            "properties": {
                "device_id": {"type": "string", "description": "Device id."},
                "hours": {"type": "integer", "description": "Hours back (default 24, max 168)."}
            },
            "required": ["device_id"]
        }
    },
    # -- DFD agency tools (bridge + client scaffolding) --
    {
        "name": "dfd_bridge",
        "description": "Call the DFD Agency API through the agent bridge (NON-MONEY operations only): leads, orders, milestones, order notes, activities, client requests, projects, articles, WhatsApp progress messages. Never use for payments/invoices/contracts/pricing.",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "description": "Named endpoint: lead_list|lead_get|lead_update|order_list|order_get|order_milestones|milestone_create|milestone_update|note_create|note_update|activity_create|request_list|request_get|request_update|project_list|project_get|project_update|article_list|article_create|article_update|whatsapp_send."},
                "id": {"type": "string", "description": "Record id for get/update endpoints."},
                "orderId": {"type": "string", "description": "Order id for order-scoped endpoints."},
                "payload": {"type": "object", "description": "JSON body for create/update endpoints (non-money fields only)."},
                "path": {"type": "object", "description": "Optional path-param dict for raw url mode."},
                "method": {"type": "string", "description": "HTTP method for raw url mode (default GET)."},
                "url": {"type": "string", "description": "Relative URL for raw mode, e.g. '/api/agent/orders'."},
                "timeout": {"type": "number", "description": "Request timeout seconds (default 30)."}
            },
            "required": []
        }
    },
    {
        "name": "create_client",
        "description": "Create a client scaffold under D:\\MyProject\\DFDClientProjects\\<slug>\\ (client.json + progress.md + mem/). REQUIRES explicit owner approval first (approved_by must be set). Never call without approval.",
        "parameters": {
            "type": "object",
            "properties": {
                "business_name": {"type": "string", "description": "Client business name (slugified to folder name)."},
                "vertical": {"type": "string", "description": "One of: fnb, salon, laundry, klinik, gym, bimbel, bengkel, beauty, travel, property, katering, photo."},
                "features": {"type": "array", "items": {"type": "string"}, "description": "Selected features, e.g. ['online_order','booking','review_qr']."},
                "whatsapp": {"type": "string", "description": "Client WhatsApp number."},
                "domain": {"type": "string", "description": "Client domain (optional)."},
                "approved_by": {"type": "string", "description": "Who approved this scaffold (must be non-empty; the DFD agent sets it after owner approval)."},
                "order_id": {"type": "string", "description": "DFD order id this client belongs to (optional)."}
            },
            "required": ["business_name", "vertical"]
        }
    },
]

_TOOL_DOCS = {
    "read_file": "read_file(file_path?, file_paths?, offset?, limit?) — Read one file, or batch several via file_paths in ONE call ([FILE] sections; offset 1-based, applies per file in a batch). Full read returns first page + header; PARTIAL -> page with offset/limit; unchanged repeat -> cached stub.",
    "outline": "outline(file_path) — Every signature in one file with definition lines, no bodies (~1/10 the tokens). Then read_file(offset=line, limit=N) for one body.",
    "edit_file": "edit_file(file_path, old_string?, new_string?, diff?, verify?) — A: old_string+new_string = targeted unique replace; B: new_string only = create/overwrite whole file; C: diff = '*** Add/Update/Delete/Move File' headers (optional *** Begin Patch/End Patch) with @@ hunks: '-' removed, '+' added, ' ' context anchors (numbers optional). verify=py_compile|tsc|shell after write.",
    "run_command": "run_command(command, timeout?, idle_timeout?, background?, process_action?, workdir?, expect?, tail_lines?, ready_timeout?, input?) — Git Bash (POSIX): pipes, &&, $( ), jq, gh. timeout ms (default 120000; <1000 = seconds; max 600000); idle_timeout kills commands idle by output AND CPU (default 60000; 0 = off); server-like/busy commands auto-background on timeout. process_action=list|logs|kill|wait|ready|send|kill_all. background+expect waits for readiness and reports the listening URL. workdir sets cwd, expect checks output regex.",
    "search_code": "search_code(pattern?, patterns?, path?, glob?, mode?, output?, ignore_case?, smart_case?, word?, fixed_strings?, context?, max_matches?, annotate?, snippet?) — Regex search (content) or filename search. output=lines|files|count; results annotated with enclosing symbol; snippet=function returns the body; up to 100 lines shown (full list in scratch).",
    "ast_search": "ast_search(rule, name?, path?, glob?, min_lines?, max_params?, max_results?) — Structural search over Python (call/import/class_bases/decorator/bare_except/long_function/many_params/no_docstring).",
    "browser_navigate": "browser_navigate(url, wait_ms?) — Open URL in headless Chromium, returns snapshot with @ref selectors.",
    "browser_act": "browser_act(action, selector?, text?, key?, what?, path?) — click / fill / type / press / extract / screenshot / close.",
    "browser_extract": "browser_extract(what?, selector?) — Extract text/html/value from current browser page.",
    "browser_screenshot": "browser_screenshot(path?) — Screenshot of current browser page.",
    "browser_click": "browser_click(selector) — Click an element using CSS selector or snapshot @ref tag.",
    "browser_fill": "browser_fill(selector, text) — Clear and fill an input element.",
    "browser_close": "browser_close() — Close browser session and free resources.",
    "look_at_screen": "look_at_screen() — Capture the user's live screen now and attach it inline (vision). For 'this'/'here'/highlighted content questions; enables sharing if off.",
    "set_screen_share": "set_screen_share(mode) — Live screen sharing: on / off / continuous (continuous refreshes every few seconds for near-real-time look_at_screen).",
    "create_docx": "create_docx(file_path, title?, content?) — Create a Word document (.docx).",
    "create_xlsx": "create_xlsx(file_path, headers, rows) — Create an Excel spreadsheet (.xlsx).",
    "read_pdf": "read_pdf(file_path, max_pages?, page_numbers?) — Read text from a PDF file.",
    "delegate": "delegate(items=[{agent, task, title?, chat_id?}]) — Spawn Dev/General/House agents IN PARALLEL; each gets its own chat, results arrive as [DELEGATION RESULT] events.",
    "delegation_status": "delegation_status() — List this session's delegations with live statuses and chats.",
    "call_subagent": "call_subagent(name, task, action?, child_id?, message?, wait_seconds?) — Specialist subagent lifecycle: run (block) / spawn (background) / status / message / stop / list (e.g. UI for browser verification).",
    "ask_user": "ask_user(questions, timeout?) — Ask the human a structured question and wait for the answer (use when the request is ambiguous, a decision is needed, or a plan needs approval via plan-review intent).",
    "github_apply_gate": "github_apply_gate(repo, stack?, codeql_on?, create?, private?, description?, gate?) — The ONE gate tool: create the repo (create=True) and/or install the security/perf gate (seed + branch protection). Confirms with the user (gate: ask|yes|no).",
    "create_project": "create_project(name, concept, wait?, parent_root?) — Create a NEW project: folder (D:\\MyProject, or parent_root for the DFD agent) + workspace + chat + durable Dev. Only for genuinely new projects. wait=false for bursts. DFD agent passes parent_root inside D:\\MyProject\\DFDClientProjects.",
    "resume_project": "resume_project(name, message, timeout?) — One-call resume: find the project's Dev and deliver the message (replaces find_project_dev + send_message).",
    "update_plan": "update_plan(explanation?, plan=[{step, status}]) — Maintain the task plan. FULL replacement: always send every step. At most one step in_progress.",
    "house_get_state": "house_get_state() — Read the full house state: rooms, devices, scenes, energy, security. Call first for any house question.",
    "house_get_device": "house_get_device(device_id) — One device's state, capabilities, room, online status.",
    "house_set_device": "house_set_device(device_id, state) — Actuate a device: light {power,brightness,colorTemp}, thermostat {targetTemp,mode}, lock {locked}, plug {power}, blinds {position}, camera {recording}. Sensors read-only.",
    "house_run_scene": "house_run_scene(scene_id) — Activate a scene (applies all actions at once).",
    "house_get_history": "house_get_history(device_id, hours?) — Mock hourly kWh usage series for a device (max 168h).",
    "dfd_bridge": "dfd_bridge(action?, id?, orderId?, payload?, path?, method?, url?, timeout?) — Call the DFD Agency API bridge (NON-MONEY): leads, orders, milestones, notes, activities, client requests, projects, articles, whatsapp. Named actions (lead_list, order_get, milestone_create, note_create, activity_create, request_update, project_update, article_create, whatsapp_send) or raw method+url.",
    "create_client": "create_client(business_name, vertical, features?, whatsapp?, domain?, approved_by?, order_id?) — Create client scaffold under D:\\MyProject\\DFDClientProjects\\<slug>\\ (client.json + progress.md + mem/). REQUIRES explicit owner approval (approved_by).",
}

_TOOL_REQUIRED_ARGS: dict[str, set[str]] = {
    d["name"]: set((d.get("parameters") or {}).get("required", []))
    for d in TOOL_DEFINITIONS
}


def _scope_allowed(scope: Optional[str]) -> set[str] | None:
    """Scope ACL minus eval-only exclusions.

    Eval/benchmark runs (DEEP_AGENTS_EVAL_RUN=1) exercise the Developer as a
    headless coding agent: no human dock, no mode-switch, no plan review. The
    dock/mode tools are removed from the schema so they neither consume
    context (~1.2k tokens) nor invite ceremony in a single-agent loop.
    Production is untouched (env is only set by the eval runner).
    """
    allowed = None
    if scope and scope.lower() in _SCOPE_MAP:
        allowed = set(_SCOPE_MAP[scope.lower()])
        if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1" and scope.lower() == "developer":
            # Headless eval has no human dock: ask_user/propose_plan are always
            # removed. update_plan is the production planning surface and stays
            # available in long-horizon mode (BSA_FULL_DEV_TOOLS=1) where the
            # completion gate keys off the LLM-maintained plan.
            allowed.difference_update({"ask_user", "propose_plan"})
            if os.environ.get("BSA_FULL_DEV_TOOLS", "0") != "1":
                allowed.difference_update({"update_plan"})
    return allowed


_EVAL_BLOCKED_TOOLS = {"ask_user", "propose_plan"}


def _eval_blocked(name: str) -> bool:
    """True for tools no agent may be offered in a headless eval run.

    Specialist subagents (UI/Deploy) carry their own tool allow-lists from
    agents/specialists/*.json, which bypass the developer-scope exclusion.
    Without a human connected, an ask_user call would block until its timeout,
    so the tools are removed for every scope while DEEP_AGENTS_EVAL_RUN=1.
    Production is untouched (the env is only set by the eval runner).
    """
    return os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1" and str(name) in _EVAL_BLOCKED_TOOLS


def get_native_tools(scope: Optional[str] = None) -> list[dict]:
    """Return tools as DeepSeek/OpenAI-native function definitions,
    optionally filtered by scope."""
    tools = []
    allowed = _scope_allowed(scope)
    for d in TOOL_DEFINITIONS:
        if d.get("exposed") is False:
            continue
        if _eval_blocked(d["name"]):
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

# Scope ACL — single source of truth is agents/workspace_registry.py.
# B1: the hand-copied literal fallback is removed. The registry module is
# loaded by file path (bypassing agents/__init__ so the tools<->agents cycle
# cannot trigger during bootstrap). A scope-map failure fails loud.
def _load_registry_scope() -> dict[str, list[str]]:
    import importlib.util
    import os
    import sys as _sys

    _here = os.path.dirname(os.path.abspath(__file__))
    _reg_path = os.path.join(_here, "..", "agents", "workspace_registry.py")
    _spec = importlib.util.spec_from_file_location("_workspace_registry", _reg_path)
    _mod = importlib.util.module_from_spec(_spec)
    _sys.modules["_workspace_registry"] = _mod
    _spec.loader.exec_module(_mod)
    return dict(_mod.scope_map())


_SCOPE_MAP: dict[str, list[str]] = _load_registry_scope()


def get_native_tool_names(scope: Optional[str] = None) -> list[str]:
    """Return tool names, optionally filtered by scope."""
    allowed = _scope_allowed(scope)
    if allowed is not None:
        names = [d["name"] for d in TOOL_DEFINITIONS if d["name"] in allowed and d.get("exposed") is not False]
    else:
        names = [d["name"] for d in TOOL_DEFINITIONS if d.get("exposed") is not False]
    names = [n for n in names if not _eval_blocked(n)]
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
