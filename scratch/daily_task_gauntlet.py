"""
Daily Task Gauntlet — Feeds a sequence of realistic software engineering
requests to the orchestrator and captures every decision, tool call, and token.

Each task is independent or builds on prior work in the same sandbox.
"""
import os, sys, json, time, shutil, re
from datetime import datetime

sys.path.insert(0, r'D:\MyProject\LangChain')
os.environ['DEEP_AGENTS_BUDGET_CAP'] = '5.00'

from sync_manager import run_and_sync_graph
from state_sync import safe_get_state, reset_shared_state

SANDBOX = r'D:\MyProject\GauntletSandbox'

def clean_sandbox():
    """Fresh workspace for the gauntlet."""
    os.makedirs(SANDBOX, exist_ok=True)
    for f in os.listdir(SANDBOX):
        path = os.path.join(SANDBOX, f)
        try:
            if os.path.isfile(path):
                os.remove(path)
            elif os.path.isdir(path) and f != '.deep_agents':
                shutil.rmtree(path)
        except:
            pass
    print(f"[SANDBOX] Cleaned: {SANDBOX}")

# ═══════════════════════════════════════════════════════════════════════════════
# DAILY TASK SEQUENCE — Realistic software engineering requests
# ═══════════════════════════════════════════════════════════════════════════════

TASKS = [
    # Task 1: Simple website
    {
        "id": "task-01-landing",
        "name": "Build a landing page",
        "prompt": (
            "Create a simple landing page for a startup called 'CloudBrew' that sells "
            "AI-powered coffee machines. Include: header with logo/name, hero section "
            "with tagline 'Coffee. Reinvented.', a 3-column features section (Smart Brewing, "
            "Voice Control, Self-Cleaning), and a footer with contact email. "
            "Use HTML + CSS in a single index.html file. Make it look professional."
        ),
    },
    # Task 2: Add JavaScript interactivity to the existing page
    {
        "id": "task-02-interactivity",
        "name": "Add interactivity to landing page",
        "prompt": (
            "The CloudBrew landing page (index.html) is too static. Add JavaScript to make it interactive: "
            "1. A dark/light mode toggle button in the top-right corner "
            "2. Smooth scroll when clicking nav links "
            "3. A 'Buy Now' button in the hero that shows a modal/popup with a simple order form "
            "(name, email, quantity dropdown). The form should validate that email contains '@' "
            "and name is not empty before allowing submit. On submit, show a success message "
            "inside the modal instead of actually submitting. "
            "Keep everything in index.html (inline CSS + JS is fine)."
        ),
    },
    # Task 3: Refactor — split monolith into separate files
    {
        "id": "task-03-refactor",
        "name": "Refactor into separate files",
        "prompt": (
            "The index.html file has grown too large with inline CSS and JS. Refactor it: "
            "1. Extract CSS into styles.css "
            "2. Extract JavaScript into script.js "
            "3. Update index.html to link to both external files "
            "4. Make sure everything still works — the dark mode toggle, smooth scroll, "
            "modal with validation, all styling. Verify by reading all three files "
            "and confirming the links are correct."
        ),
    },
    # Task 4: Bug fix — something deliberately ambiguous
    {
        "id": "task-04-bugfix",
        "name": "Fix dark mode persistence bug",
        "prompt": (
            "Users report that the dark mode toggle on CloudBrew's page doesn't remember "
            "their preference — every time they refresh, it resets to light mode. "
            "Fix this so the theme preference is saved and restored on page load. "
            "Also, the modal close button (X) sometimes doesn't work on mobile. Investigate and fix."
        ),
    },
    # Task 5: Add tests
    {
        "id": "task-05-tests",
        "name": "Write tests for the landing page",
        "prompt": (
            "We need automated tests for the CloudBrew landing page. Create a test file "
            "test_cloudbrew.py using pytest that verifies: "
            "1. index.html exists and contains the correct title 'CloudBrew' "
            "2. styles.css exists and is linked from index.html "
            "3. script.js exists and is linked from index.html "
            "4. script.js contains a dark mode toggle function "
            "5. script.js contains form validation logic "
            "Run the tests and fix any failures."
        ),
    },
    # Task 6: Add a backend API endpoint
    {
        "id": "task-06-api",
        "name": "Build a Python API for orders",
        "prompt": (
            "CloudBrew now needs a backend to handle pre-orders from the landing page. "
            "Create a simple Flask API in app.py with these endpoints: "
            "POST /api/orders — accepts JSON {name, email, quantity, model}, validates fields "
            "(name required, email must contain @, quantity 1-10, model must be one of: "
            "BrewMaster, CaffeinePro, LatteBot), stores to a SQLite database orders.db, "
            "returns {success: true, order_id: N}. "
            "GET /api/orders — returns all orders as JSON array. "
            "GET /api/orders/<id> — returns single order. "
            "Include a requirements.txt with flask. "
            "Test it by starting the server briefly and hitting the endpoints."
        ),
    },
    # Task 7: Cross-cutting — connect frontend to backend
    {
        "id": "task-07-integration",
        "name": "Connect frontend form to backend API",
        "prompt": (
            "Now connect the CloudBrew landing page modal form to the Flask API. "
            "Update script.js so the order form POSTs to /api/orders instead of just "
            "showing a success message. The modal should: "
            "1. Send {name, email, quantity, model: 'BrewMaster'} to POST /api/orders "
            "2. Show the returned order_id on success "
            "3. Show error messages from the API if validation fails "
            "4. Add a dropdown to the form for model selection (BrewMaster, CaffeinePro, LatteBot) "
            "Update index.html for the new model dropdown. Make sure the form still "
            "validates client-side before sending."
        ),
    },
]

def extract_token_usage(state_snapshot):
    """Pull token/cost details from state."""
    tu = state_snapshot.get("token_usage", {})
    calls = tu.get("calls", [])
    # Normalize per-call fields to match what the summary printer expects
    normalized_calls = []
    for c in calls:
        normalized_calls.append({
            "role": c.get("agent", c.get("role", "?")),
            "model": c.get("model", "?"),
            "input_tokens": c.get("input", 0),
            "output_tokens": c.get("output", 0),
            "cache_hit_tokens": c.get("cache_hits", 0),
            "cache_miss_tokens": c.get("cache_misses", 0),
            "cost": c.get("cost", 0),
            "where": c.get("where", ""),
        })
    return {
        "total_cost": tu.get("total_cost", 0),
        "total_input": tu.get("total_input_tokens", 0),
        "total_output": tu.get("total_output_tokens", 0),
        "cache_hit": tu.get("total_cache_hit_tokens", 0),
        "cache_miss": tu.get("total_cache_miss_tokens", 0),
        "num_calls": len(calls),
        "calls": normalized_calls,
    }

def run_single_task(task, chat_id):
    """Run one task through the orchestrator. Returns full trace."""
    print(f"\n{'#'*70}")
    print(f"# {task['id']}: {task['name']}")
    print(f"{'#'*70}")
    print(f"PROMPT: {task['prompt'][:200]}...")
    print()

    trace = {
        "task": task,
        "steps": [],
        "final_state": None,
        "token_usage": None,
        "files_after": [],
        "errors": [],
    }

    start_time = time.time()

    try:
        for snap in run_and_sync_graph(
            task["prompt"],
            workspace_path=SANDBOX,
            chat_id=chat_id
        ):
            step_info = {
                "active_node": snap.get("active_node", "?"),
                "next_agent": snap.get("next_agent", ""),
                "thoughts": dict(snap.get("thoughts", {})),
                "completed_nodes": list(snap.get("completed_nodes", [])),
            }
            # Capture output keys
            outputs = snap.get("outputs", {})
            for k in ["code", "agent_report", "test_report"]:
                val = outputs.get(k, "")
                if val:
                    step_info[k] = str(val)[:300]

            # Capture live terminal log excerpt
            term = snap.get("live_terminal_log", "")
            if term:
                # Get last meaningful line
                lines = [l for l in term.split("\n") if l.strip()]
                step_info["last_log"] = lines[-1][:200] if lines else ""

            trace["steps"].append(step_info)

            # Show progress
            node = step_info["active_node"]
            nxt = step_info["next_agent"]
            thought = step_info["thoughts"].get("agent", step_info["thoughts"].get("developer", ""))[:100]
            last_log = step_info.get("last_log", "")[:120]
            status_line = f"  [{node}] next={nxt}"
            if thought:
                status_line += f" | {thought}"
            if last_log:
                status_line += f" | log: {last_log}"
            print(status_line)

        # Collect final state
        final = safe_get_state()
        trace["final_state"] = {
            "code": final.get("outputs", {}).get("code", "")[:500],
            "agent_report": final.get("outputs", {}).get("agent_report", "")[:500],
            "test_report": final.get("outputs", {}).get("test_report", "")[:300],
        }
        trace["token_usage"] = extract_token_usage(final)

        elapsed = time.time() - start_time
        print(f"\n  DONE in {elapsed:.1f}s")

    except Exception as e:
        trace["errors"].append(str(e))
        print(f"\n  ERROR: {e}")

    # List files after
    try:
        trace["files_after"] = [
            f for f in os.listdir(SANDBOX)
            if os.path.isfile(os.path.join(SANDBOX, f))
        ]
        print(f"  Files in sandbox: {', '.join(trace['files_after'])}")
    except:
        pass

    return trace

def print_summary(all_traces):
    """Print final gauntlet summary with token breakdown."""
    print("\n\n")
    print("=" * 80)
    print("GAUNTLET SUMMARY")
    print("=" * 80)

    total_cost = 0
    total_input = 0
    total_output = 0
    total_cache_hit = 0
    total_calls = 0

    for t in all_traces:
        task = t["task"]
        tu = t.get("token_usage") or {}
        cost = tu.get("total_cost", 0)
        inp = tu.get("total_input", 0)
        out = tu.get("total_output", 0)
        cache_hit = tu.get("cache_hit", 0)
        cache_miss = tu.get("cache_miss", 0)
        calls = tu.get("num_calls", 0)

        total_cost += cost
        total_input += inp
        total_output += out
        total_cache_hit += cache_hit
        total_calls += calls

        # Check errors: explicit exceptions OR test_report STATUS: FAIL OR empty final state
        has_exception = bool(t["errors"])
        test_report = (t.get("final_state") or {}).get("test_report", "")
        has_test_failure = "STATUS: FAIL" in str(test_report)
        no_files_and_short = (
            not t.get("files_after") and
            len(t["steps"]) <= 2 and
            t.get("token_usage", {}).get("num_calls", 0) <= 1
        )
        status = "ERROR" if (has_exception or has_test_failure) else "OK"
        if no_files_and_short and not has_exception:
            status = "NOOP"
        n_steps = len(t["steps"])
        files = ", ".join(t.get("files_after", []))
        hit_pct = f"{cache_hit/inp*100:.0f}%" if inp > 0 else "0%"

        print(f"\n  [{status}] {task['id']}: {task['name']}")
        print(f"      Calls: {calls} | Cost: ${cost:.5f} | In: {inp} | Out: {out}")
        print(f"      Cache: hit={cache_hit} miss={cache_miss} ({hit_pct} hit)")
        print(f"      Files: {files or '(none)'}")
        if t["errors"]:
            print(f"      Errors: {'; '.join(t['errors'])}")

    print(f"\n  -------------------------------------------------")
    print(f"  TOTAL: Cost=${total_cost:.6f} | Input={total_input} | Output={total_output} | CacheHit={total_cache_hit} | Calls={total_calls}")
    if total_input > 0:
        print(f"  Cache efficiency: {total_cache_hit}/{total_input} = {total_cache_hit/total_input*100:.1f}%")

    # Detailed token call breakdown
    print(f"\n{'-'*80}")
    print("PER-CALL TOKEN BREAKDOWN")
    print(f"{'-'*80}")
    for t in all_traces:
        task = t["task"]
        tu = t.get("token_usage") or {}
        calls = tu.get("calls", [])
        if calls:
            print(f"\n  {task['id']}:")
            for i, c in enumerate(calls):
                role = c.get("role", "?")
                inp_t = c.get("input_tokens", 0)
                out_t = c.get("output_tokens", 0)
                cache_h = c.get("cache_hit_tokens", 0)
                cache_m = c.get("cache_miss_tokens", 0)
                cost_c = c.get("cost", 0)
                model = c.get("model", "?")

                # Summarize the prompt/content
                content = c.get("content", c.get("prompt", ""))
                content_preview = ""
                if isinstance(content, str):
                    content_preview = content[:80].replace("\n", " ")
                elif isinstance(content, list):
                    # messages format
                    msgs = content
                    if len(msgs) > 1:
                        last = msgs[-1]
                        if isinstance(last, dict):
                            content_preview = str(last.get("content", ""))[:80].replace("\n", " ")
                    elif msgs:
                        content_preview = str(msgs[0])[:80].replace("\n", " ")

                print(f"    #{i}: [{role}] {model} | in={inp_t} out={out_t} cache_hit={cache_h} cache_miss={cache_m} cost=${cost_c:.6f}")
                if content_preview:
                    print(f"         \"{content_preview}...\"")
        else:
            print(f"\n  {task['id']}: (no token calls recorded)")

    # Write full JSON report
    report_path = r"D:\MyProject\LangChain\gauntlet_report.json"
    with open(report_path, "w", encoding="utf-8") as f:
        json.dump(all_traces, f, indent=2, default=str)
    print(f"\n  Full report saved to: {report_path}")


if __name__ == "__main__":
    clean_sandbox()
    all_traces = []

    for i, task in enumerate(TASKS):
        chat_id = f"gauntlet-{task['id']}"
        reset_shared_state()  # Clean slate — no token bleed, no stale logs
        trace = run_single_task(task, chat_id)
        all_traces.append(trace)

    print_summary(all_traces)
