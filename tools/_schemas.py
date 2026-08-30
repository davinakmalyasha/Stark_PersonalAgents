from typing import Optional

"""Central model-facing tool schemas and compact tool documentation.

The facade imports these values back so existing callers continue to use
tools.TOOL_DEFINITIONS, tools._TOOL_DOCS, and tools._TOOL_REQUIRED_ARGS.
"""

TOOL_DEFINITIONS = [
    {
        "name": "read_file",
        "description": "Read a file, or several via file_paths (per-file sections).",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "file_paths": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "offset": {"type": "integer", "description": "Start line."},
                "limit": {"type": "integer", "description": "Max lines."}
            },
            "required": []
        }
    },
    {
        "name": "edit_file",
        "description": "Edit or create files via exact replacement.",
        "parameters": {
            "type": "object",
            "properties": {
                "file_path": {"type": "string"},
                "old_string": {"type": "string", "description": "Exact existing text."},
                "new_string": {"type": "string", "description": "Replacement; omit old_string to create/overwrite."},
                "diff": {"type": "string", "description": "Batch mode. (1) SEARCH/REPLACE blocks: <<<<<<< SEARCH\\n[old]\\n=======\\n[new]\\n>>>>>>> REPLACE; (2) unified diff (git @@ hunks, uses file_path); (3) multi-file patch: '*** Add File: path', '*** Update File: path', '*** Delete File: path', '*** Move to: path' headers — applied per-file transactionally with [OK]/[ERROR] summary."},
                "verify": {"type": "string", "description": "'py_compile' | 'tsc' | shell command — runs after write."}
            },
            "required": ["file_path"]
        }
    },
    {
        "name": "run_command",
        "description": "Run a shell command. Foreground blocks and streams logs; background returns a key managed via process_action.",
        "parameters": {
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Shell command; also names the process for process_action='logs'|'kill'."},
                "timeout": {"type": "integer", "description": "Foreground timeout ms."},
                "background": {"type": "boolean"},
                "process_action": {"type": "string", "description": "'list' | 'logs' | 'kill'."},
                "shell": {"type": "string", "description": "'bash' only (Git Bash 5.3.15); 'cmd' is removed — use bash."},
                "workdir": {"type": "string", "description": "Working directory (default project root)."},
                "expect": {"type": "string", "description": "Regex expected in output; typed pass/fail."}
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
        "description": "Regex-search file contents ('content', default) or match path globs ('files'). Multi-pattern results ranked.",
        "parameters": {
            "type": "object",
            "properties": {
                "pattern": {"type": "string", "description": "Regex (content mode) or glob/filename fragment (files mode)."},
                "patterns": {"type": "array", "items": {"type": "string"}},
                "path": {"type": "string"},
                "glob": {"type": "string", "description": "File filter, e.g. '*.py'."},
                "mode": {"type": "string", "enum": ["content", "files"]}
            },
            "required": []
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
        "description": "Drive the live job tracker (the dock the boss watches). Call this whenever the set of jobs changes — at spawn, as results land, or when you abort. Each call is a FULL REPLACEMENT: send every step every time, with the current status. At most one step may be in_progress. Use status=completed (not a separate failed value) for any job that has settled — put the reason in the step text when not successful (e.g. 'Build snake game — failed: tests did not pass').",
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
        "description": "Stage 4: submit a plan proposal to the user. Use ONLY in plan mode when you want the user to review and (manually) press /build to execute. Outside plan mode this tool is a no-op alias for update_plan. The agent CANNOT transition out of plan mode on its own — the user must press /build.",
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
                            "task": {"type": "string", "description": "Complete, self-contained instruction for this specialist. The agent sees only this text — include the goal, any constraints, relevant paths, and how to verify success."},
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
        "description": "Spawn a specialist subagent (see Subagents list) and block until its final report.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Subagent type, e.g. UI | Deploy."},
                "task": {"type": "string", "description": "Goal, targets (files/URL/repo), acceptance check."}
            },
            "required": ["name", "task"]
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
    {
        "name": "template",
        "description": "Boilerplate catalog: list, keyword search, show details, apply (renders files; skips existing unless overwrite).",
        "parameters": {
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "search", "show", "apply"]},
                "template": {"type": "string", "description": "Name for show/apply."},
                "category": {"type": "string"},
                "query": {"type": "string", "description": "Keywords for search/list."},
                "target_dir": {"type": "string", "description": "Apply destination (default workspace root)."},
                "variables": {"type": "object", "description": "Template variables."},
                "overwrite": {"type": "boolean"},
                "dry_run": {"type": "boolean"},
                "quiet": {"type": "boolean"}
            },
            "required": ["action"]
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
    "delegate": "delegate(items=[{agent, task, title?, chat_id?}]) — Spawn Dev/General/House agents IN PARALLEL; each gets its own chat, results arrive as [DELEGATION RESULT] events.",
    "delegation_status": "delegation_status() — List this session's delegations with live statuses and chats.",
    "call_subagent": "call_subagent(name, task) — Spawn a specialist subagent (UI, Deploy); blocks and returns its final report.",
    "ask_user": "ask_user(questions, timeout?) — Ask the human a structured question and wait for the answer (use when the request is ambiguous, a decision is needed, or a plan needs approval via plan-review intent).",
    "github_apply_gate": "github_apply_gate(repo, stack?, codeql_on?, create?, private?, description?, gate?) — The ONE gate tool: create the repo (create=True) and/or install the security/perf gate (seed + branch protection). Confirms with the user (gate: ask|yes|no).",
    "template_new": "template_new(name, level, description, sample_vars, variables?, files?, uses?, notes?) — Author a new template (write .j2 files first, then metadata; auto-validates render).",
    "create_project": "create_project(name, concept, wait?, parent_root?) — Create a NEW project: folder (D:\\MyProject, or parent_root for the DFD agent) + workspace + chat + durable Dev. Only for genuinely new projects. wait=false for bursts. DFD agent passes parent_root inside D:\\MyProject\\DFDClientProjects.",
    "resume_project": "resume_project(name, message, timeout?) — One-call resume: find the project's Dev and deliver the message (replaces find_project_dev + send_message).",
    "template": "template(action, template?, category?, query?, target_dir?, variables?, overwrite?, dry_run?, quiet?) — Unified template: list/search/show/apply (replaces apply_template + template_list).",
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

# Scope ACL — single source of truth is agents/workspace_registry.py.
# The comments below document WHY each agent has its surface; the tool
# lists themselves come from WORKSPACE_REGISTRY so one place changes both.
_SCOPE_MAP: dict[str, list[str]] = {}
try:
    from agents.workspace_registry import scope_map as _ws_scope_map

    _SCOPE_MAP = _ws_scope_map()
except Exception:
    # Import-order guard (agents import tools at module load). The registry
    # module is dependency-light; if it is unavailable, fall back to the
    # preserved literal so tool scoping never silently disappears.
    _SCOPE_MAP = {
        "developer": ["read_file", "edit_file", "run_command", "search_code", "template", "ask_user", "update_plan", "propose_plan", "call_subagent"],
        "ui": ["browser_act", "read_file", "search_code", "run_command"],
        "deploy": ["github_apply_gate", "template", "run_command", "read_file"],
        "orchestrator": ["create_project", "resume_project", "ask_user", "delegate", "delegation_status", "update_plan", "propose_plan", "files_changed"],
        "general": ["run_command", "read_pdf", "create_docx", "create_xlsx", "ask_user", "update_plan", "browser_act", "propose_plan", "files_changed"],
        "house": ["house_get_state", "house_get_device", "house_set_device", "house_run_scene", "house_get_history", "run_command", "browser_act", "ask_user", "update_plan", "propose_plan"],
        "dfd": ["dfd_bridge", "create_client", "create_project", "run_command", "ask_user", "update_plan", "propose_plan", "files_changed"],
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
