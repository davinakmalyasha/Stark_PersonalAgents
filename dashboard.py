import os, json, threading, gradio as gr
from dotenv import load_dotenv
from sync_manager import shared_state, run_and_sync_graph
from bot import bot
from state_sync import safe_serialize_state, safe_update_state, safe_get_state

load_dotenv(override=True)

def poll_state():
    state = safe_get_state()
    usage = state.get("token_usage", {
        "total_input_tokens": 0,
        "total_output_tokens": 0,
        "total_cost": 0.0,
        "calls": []
    })
    
    # Group by team
    teams = {
        "planning": {"input": 0, "output": 0, "cost": 0.0},
        "engineering": {"input": 0, "output": 0, "cost": 0.0},
        "devops": {"input": 0, "output": 0, "cost": 0.0},
        "management": {"input": 0, "output": 0, "cost": 0.0}
    }
    
    for c in usage.get("calls", []):
        agent = c.get("agent", "").lower()
        inp = c.get("input", 0)
        out = c.get("output", 0)
        cost = c.get("cost", 0.0)
        
        if agent in ["ba", "sa"]:
            teams["planning"]["input"] += inp
            teams["planning"]["output"] += out
            teams["planning"]["cost"] += cost
        elif agent in ["developer", "tester", "coder"]:
            teams["engineering"]["input"] += inp
            teams["engineering"]["output"] += out
            teams["engineering"]["cost"] += cost
        elif agent in ["devops"]:
            teams["devops"]["input"] += inp
            teams["devops"]["output"] += out
            teams["devops"]["cost"] += cost
        else: # supervisor, analytics, router, etc.
            teams["management"]["input"] += inp
            teams["management"]["output"] += out
            teams["management"]["cost"] += cost

    # Build HTML for each team
    def format_team_html(name, data):
        return f"""
        <div style='background:#1e1f20; border:1px solid #2c2d30; padding:8px 12px; border-radius:6px; margin-bottom:8px;'>
            <div style='font-size:10px; color:#a8c7fa; font-weight:bold; margin-bottom:4px; text-transform:uppercase; letter-spacing:0.5px;'>{name}</div>
            <div style='display:flex; justify-content:space-between; font-size:11px; color:#e3e3e3;'>
                <span>In: <strong>{data['input']}</strong> | Out: <strong>{data['output']}</strong></span>
                <span style='color:#34d399; font-weight:bold;'>${data['cost']:.5f}</span>
            </div>
        </div>
        """
        
    planning_html = format_team_html("Product/Planning (BA & SA)", teams["planning"])
    engineering_html = format_team_html("Engineering/QA (Dev & QA)", teams["engineering"])
    devops_html = format_team_html("DevOps (Infrastructure)", teams["devops"])
    management_html = format_team_html("Management (Director/Analytics)", teams["management"])
    
    # Build HTML table for call log
    calls = usage.get("calls", [])
    if not calls:
        table_html = "<div style='color:#777; font-size:11px; font-style:italic; padding:8px; text-align:center;'>No calls made yet</div>"
    else:
        table_html = "<div style='max-height:120px; overflow-y:auto; border:1px solid #2c2d30; border-radius:4px; background:#131314;'>"
        table_html += "<table style='width:100%; border-collapse:collapse; font-size:10px; text-align:left; color:#cbd5e1;'>"
        table_html += "<tr style='border-bottom:1px solid #2c2d30; background:#1e1f20; color:#94a3b8; font-weight:bold;'>"
        table_html += "<th style='padding:4px;'>Agent</th><th style='padding:4px;'>Model</th><th style='padding:4px; text-align:right;'>Tokens</th><th style='padding:4px; text-align:right;'>Cost</th></tr>"
        for c in reversed(calls):
            agent = c.get("agent", "").upper()
            model = c.get("model", "")
            if len(model) > 10:
                model = model[:8] + ".."
            tokens = c.get("input", 0) + c.get("output", 0)
            cost = c.get("cost", 0.0)
            table_html += f"<tr style='border-bottom:1px solid #2d2f31;'><td style='padding:4px; font-weight:600;'>{agent}</td><td style='padding:4px; color:#94a3b8;'>{model}</td><td style='padding:4px; text-align:right;'>{tokens}</td><td style='padding:4px; text-align:right; color:#34d399;'>${cost:.4f}</td></tr>"
        table_html += "</table></div>"
        
    outputs = state.get("outputs", {})
    req_txt = outputs.get("requirements", "")
    gherkin_txt = outputs.get("gherkin", "")
    mermaid_txt = outputs.get("mermaid", "")
    spec_txt = outputs.get("tech_spec", "")
    code_txt = outputs.get("code", "")
    test_txt = outputs.get("test_report", "")
    devops_txt = outputs.get("devops_config", "")
    analytics_txt = outputs.get("analytics_report", "")
    
    return (
        safe_serialize_state(),
        planning_html,
        engineering_html,
        devops_html,
        management_html,
        f"${usage.get('total_cost', 0.0):.6f}",
        table_html,
        req_txt,
        gherkin_txt,
        mermaid_txt,
        spec_txt,
        code_txt,
        test_txt,
        devops_txt,
        analytics_txt
    )

def new_session():
    empty_outputs = {
        "requirements": "", "tech_spec": "", "code": "",
        "test_report": "", "devops_config": "", "analytics_report": "",
        "gherkin": "", "mermaid": ""
    }
    safe_update_state({
        "active_node": "",
        "next_agent": "",
        "completed_nodes": [],
        "thoughts": {
            "supervisor": "", "ba": "", "sa": "", "developer": "",
            "tester": "", "devops": "", "analytics": ""
        },
        "client_request": "",
        "outputs": empty_outputs,
        "project_path": "",
        "agents_plan": "",
        "deep_agents_log": [],
        "live_terminal_log": "🧹 Session reset. Ready for a new request.\n",
        "token_usage": {
            "total_input_tokens": 0,
            "total_output_tokens": 0,
            "total_cost": 0.0,
            "calls": []
        }
    })
    return (
        safe_serialize_state(),
        "<div style='color:#777; font-size:11px; font-style:italic; padding:8px; text-align:center;'>No calls made yet</div>",
        "<div style='color:#777; font-size:11px; font-style:italic; padding:8px; text-align:center;'>No calls made yet</div>",
        "<div style='color:#777; font-size:11px; font-style:italic; padding:8px; text-align:center;'>No calls made yet</div>",
        "<div style='color:#777; font-size:11px; font-style:italic; padding:8px; text-align:center;'>No calls made yet</div>",
        "$0.000000",
        "<div style='color:#777; font-size:11px; font-style:italic;'>No calls made yet</div>",
        "", "", "", "", "", "", "", ""
    )

def get_recent_prompts():
    from chat_memory_manager import load_chat_memory
    try:
        mem = load_chat_memory()
        messages = mem.get("current_day_messages", [])
        user_prompts = [m["text"] for m in messages if m.get("sender") == "user"]
        seen = set()
        user_prompts = [x for x in user_prompts if not (x in seen or seen.add(x))]
        if not user_prompts:
            return ["No recent prompts"]
        return user_prompts[:10]
    except Exception:
        return ["No recent prompts"]

_CHAT_KEYWORDS = {"hello", "hi", "hey", "thanks", "thank you", "good morning",
                  "good afternoon", "how are you", "what's up", "bye", "goodbye"}

def _is_obvious_chat(text: str) -> bool:
    import re
    text_lower = text.lower().strip()
    words = text_lower.rstrip(".!?").split()
    if len(words) > 6:
        return False
    
    # Greetings & thanks keyword check with word boundaries to avoid substring matches
    greetings = [
        r"\bhello\b", r"\bhi\b", r"\bhey\b", r"\bthanks\b", r"\bthank\s+you\b",
        r"\bgood\s+morning\b", r"\bgood\s+afternoon\b", r"\bhow\s+are\s+you\b",
        r"\bwhat's\s+up\b", r"\bbye\b", r"\bgoodbye\b"
    ]
    has_greeting = any(re.search(pattern, text_lower) for pattern in greetings)
    if not has_greeting:
        return False

    # Ensure it doesn't contain any task action words
    actions = [
        "build", "create", "make", "write", "code", "fix", "implement",
        "add", "change", "modify", "delete", "remove", "refactor", "deploy",
        "run", "test"
    ]
    has_action = any(re.search(rf"\b{act}\b", text_lower) for act in actions)
    return not has_action

def run_from_web(req, model, temp):
    from chat_memory_manager import add_chat_message, resolve_request_text

    safe_update_state({
        "selected_model": model,
        "selected_temp": float(temp)
    })

    add_chat_message("user", req)
    resolved_req = resolve_request_text(req)

    if _is_obvious_chat(req):
        add_chat_message("jarvis", "Hi! Ready to help.")
        safe_update_state({
            "active_node": "agent", "next_agent": "", "completed_nodes": [],
            "thoughts": {"agent": "Ready and waiting."},
            "client_request": resolved_req,
            "outputs": {
                "requirements": "", "tech_spec": "", "code": "",
                "test_report": "", "devops_config": "", "analytics_report": "",
                "gherkin": "", "mermaid": ""
            },
            "deep_agents_log": [],
            "live_terminal_log": f"👤 User: {req}\n\n🤖 Assistant: Hi! Ready to help.\n"
        })
        yield safe_serialize_state()
        return

    add_chat_message("jarvis", f"Initiating work: {resolved_req}")
    for s in run_and_sync_graph(resolved_req):
        yield safe_serialize_state()

# Start Telegram bot in background thread
threading.Thread(target=bot.infinity_polling, daemon=True).start()

# CSS injection for strict Google AI Studio Charcoal styling
css = """
#state_box { display: none !important; }
.gradio-container {
    background-color: #131314 !important;
    color: #e3e3e3 !important;
    border: none !important;
    font-family: 'Inter', -apple-system, sans-serif !important;
}
.block {
    background-color: #1e1f20 !important;
    border: 1px solid #2c2d30 !important;
    border-radius: 8px !important;
    margin-bottom: 10px !important;
}
.gr-form {
    border: none !important;
    background: transparent !important;
}
/* Title & Heading Overrides */
h1, h2, h3, h4, h5, p {
    color: #ffffff !important;
    font-family: 'Inter', sans-serif !important;
}
/* Style text inputs & dropdowns */
input, select, textarea {
    background-color: #131314 !important;
    border: 1px solid #2c2d30 !important;
    color: #ffffff !important;
    border-radius: 6px !important;
}
input:focus, select:focus, textarea:focus {
    border-color: #8ab4f8 !important;
    outline: none !important;
}
/* Label overrides to remove ugly blue background badges */
.block-title, .block span {
    background-color: transparent !important;
    color: #94a3b8 !important;
    font-size: 10px !important;
    text-transform: uppercase !important;
    letter-spacing: 0.5px !important;
    border: none !important;
    font-weight: 600 !important;
    padding: 0 !important;
}
/* Tab container */
.tabs {
    border-bottom: 1px solid #2c2d30 !important;
}
.tabitem {
    background-color: #131314 !important;
    border: 1px solid #2c2d30 !important;
    border-top: none !important;
}
/* Prompt bar grouping styling */
.prompt-group {
    background-color: #1e1f20 !important;
    border: 1px solid #2c2d30 !important;
    border-radius: 12px !important;
    padding: 12px !important;
}
"""

theme = gr.themes.Soft(
    primary_hue="blue",
    neutral_hue="slate",
    font=[gr.themes.GoogleFont("Inter"), "ui-sans-serif", "system-ui"]
).set(
    body_background_fill="#131314",
    block_background_fill="#1e1f20",
    block_border_color="#2c2d30",
    button_primary_background_fill="linear-gradient(90deg, #60a5fa 0%, #a78bfa 100%)",
    button_primary_text_color="#ffffff",
    button_secondary_background_fill="#2c2d30",
    button_secondary_text_color="#cbd5e1"
)

with gr.Blocks(title="Google AI Studio", theme=theme, css=css) as demo:
    # Header bar
    with gr.Row():
        gr.HTML("<div style='display:flex; align-items:center; padding:12px 20px; border-bottom:1px solid #2c2d30; width:100%; background:#1e1f20; justify-content:space-between;'><div style='display:flex; align-items:center; gap:8px;'><span style='font-size:18px; font-weight:700; color:#ffffff;'>Google AI Studio</span><span style='background:#2c2d30; color:#8ab4f8; font-size:10px; font-weight:600; padding:2px 6px; border-radius:10px;'>Playground</span></div><div style='color:#94a3b8; font-size:12px; font-weight:500;'>Deep Agents Agent Workspace</div></div>")
        
    with gr.Row():
        # COLUMN 1: LEFT SIDEBAR (Navigation & History)
        with gr.Column(scale=1, min_width=200):
            gr.Markdown("### Workspace")
            new_session_btn = gr.Button("🧹 New Session", variant="secondary")
            
            gr.Markdown("### Prompt Templates")
            tpl_calc = gr.Button("📊 SQLite Calculator", variant="secondary")
            tpl_pass = gr.Button("🔑 Password DB", variant="secondary")
            tpl_weather = gr.Button("🌦️ Weather Fetcher", variant="secondary")
            
            gr.Markdown("### Recent Prompts")
            recent_dropdown = gr.Dropdown(
                label="Select History", 
                choices=get_recent_prompts(),
                value=get_recent_prompts()[0] if get_recent_prompts() else None,
                interactive=True
            )
            refresh_history_btn = gr.Button("🔄 Refresh History", variant="secondary")

        # COLUMN 2: MIDDLE PANEL (Code Editor Workspace & Prompt Input)
        with gr.Column(scale=3):
            # Code Editor Tab Layout representing Workspace Output Files
            with gr.Tabs():
                with gr.Tab("📋 Requirements.txt"):
                    req_display = gr.Code(label="Requirements", language="markdown", interactive=False, lines=22)
                with gr.Tab("⚙️ Acceptance Criteria"):
                    gherkin_display = gr.Code(label="Acceptance Criteria (Gherkin)", language="markdown", interactive=False, lines=22)
                with gr.Tab("🗺️ User Flow"):
                    mermaid_display = gr.Code(label="User Flow (Mermaid)", language="markdown", interactive=False, lines=22)
                with gr.Tab("📄 Technical Spec"):
                    spec_display = gr.Code(label="Technical Specification", language="markdown", interactive=False, lines=22)
                with gr.Tab("🐍 generated_app.py"):
                    code_display = gr.Code(label="Python Code", language="python", interactive=False, lines=22)
                with gr.Tab("🧪 QA Feedback"):
                    test_display = gr.Code(label="QA Stack Trace", language="markdown", interactive=False, lines=22)
                with gr.Tab("🐳 Dockerfile"):
                    devops_display = gr.Code(label="Dockerfile", language="dockerfile", interactive=False, lines=22)
                with gr.Tab("📊 Analytics Report"):
                    analytics_display = gr.Code(label="Analytics Report", language="markdown", interactive=False, lines=22)
            
            # Google AI Studio Styled Prompt Bar Panel at the bottom
            with gr.Group(elem_classes=["prompt-group"]):
                req_input = gr.Textbox(
                    placeholder="Start typing a prompt to see what our agents can do...", 
                    show_label=False,
                    lines=3
                )
                with gr.Row():
                    gr.Button("🛠️ Tools", variant="secondary", size="sm")
                    gr.Button("💻 Code execution", variant="secondary", size="sm")
                    gr.Button("🌐 Grounding with Google Search", variant="secondary", size="sm")
                    run_btn = gr.Button("Run ⚡", variant="primary", size="sm")
            
            state_box = gr.Textbox(visible=True, elem_id="state_box")

        # COLUMN 3: RIGHT SIDEBAR (Settings & Team Token Stats)
        with gr.Column(scale=1, min_width=220):
            gr.Markdown("### Run Settings")
            model_dropdown = gr.Dropdown(
                choices=[
                    "Automatic Fallback",
                    "Gemini 1.5 Flash",
                    "Gemini 2.5 Flash",
                    "Gemini 1.5 Pro",
                    "GPT-4o Mini",
                    "GPT-4o",
                    "Llama 3.3 70B (Groq)",
                    "Llama 3.1 8B (Groq)"
                ],
                value="Automatic Fallback",
                label="Model Override",
                interactive=True
            )
            
            temp_slider = gr.Slider(
                minimum=0.0,
                maximum=2.0,
                value=0.7,
                step=0.1,
                label="Temperature",
                interactive=True
            )
            
            gr.Markdown("### Token Stats by Team")
            planning_stats = gr.HTML(
                value="<div style='background:#1e1f20; border:1px solid #2c2d30; padding:8px 12px; border-radius:6px; margin-bottom:8px;'><div style='font-size:10px; color:#a8c7fa; font-weight:bold; margin-bottom:4px; text-transform:uppercase;'>Product/Planning (BA & SA)</div><div style='font-size:11px; color:#777; font-style:italic;'>No tokens yet</div></div>"
            )
            engineering_stats = gr.HTML(
                value="<div style='background:#1e1f20; border:1px solid #2c2d30; padding:8px 12px; border-radius:6px; margin-bottom:8px;'><div style='font-size:10px; color:#a8c7fa; font-weight:bold; margin-bottom:4px; text-transform:uppercase;'>Engineering/QA (Dev & QA)</div><div style='font-size:11px; color:#777; font-style:italic;'>No tokens yet</div></div>"
            )
            devops_stats = gr.HTML(
                value="<div style='background:#1e1f20; border:1px solid #2c2d30; padding:8px 12px; border-radius:6px; margin-bottom:8px;'><div style='font-size:10px; color:#a8c7fa; font-weight:bold; margin-bottom:4px; text-transform:uppercase;'>DevOps (Infrastructure)</div><div style='font-size:11px; color:#777; font-style:italic;'>No tokens yet</div></div>"
            )
            management_stats = gr.HTML(
                value="<div style='background:#1e1f20; border:1px solid #2c2d30; padding:8px 12px; border-radius:6px; margin-bottom:8px;'><div style='font-size:10px; color:#a8c7fa; font-weight:bold; margin-bottom:4px; text-transform:uppercase;'>Management (Director/Analytics)</div><div style='font-size:11px; color:#777; font-style:italic;'>No tokens yet</div></div>"
            )
            
            with gr.Group():
                gr.Markdown("### Total Combined Cost")
                total_cost_display = gr.Markdown("<span style='font-size:18px; font-weight:bold; color:#34d399;'>$0.000000</span>")
            
            gr.Markdown("### Call Log History")
            call_log_table = gr.HTML(
                value="<div style='color:#777; font-size:11px; font-style:italic;'>No calls made yet</div>"
            )

    # Poll state every second using gr.Timer to update stats and editor tabs in real-time
    timer = gr.Timer(1.0)
    timer.tick(
        fn=poll_state, 
        outputs=[
            state_box, 
            planning_stats, 
            engineering_stats, 
            devops_stats, 
            management_stats, 
            total_cost_display, 
            call_log_table,
            req_display,
            gherkin_display,
            mermaid_display,
            spec_display,
            code_display,
            test_display,
            devops_display,
            analytics_display
        ]
    )
    
    # Broadcast state changes
    state_box.change(
        fn=None,
        inputs=state_box,
        js="""
        (state) => {
            const iframes = document.querySelectorAll('iframe');
            for (const iframe of iframes) {
                try {
                    iframe.contentWindow.postMessage({ type: 'state_update', state: state }, '*');
                } catch(e) {}
            }
        }
        """
    )
    
    # Prefill template buttons
    def load_tpl_calc():
        return "Build a terminal-based calculator with history tracking stored in an SQLite database."
    def load_tpl_pass():
        return "Create a Secure Password Manager command-line app with SQLite to store credentials."
    def load_tpl_weather():
        return "Generate a Python script that fetches current weather for a city and formats the output."
        
    tpl_calc.click(fn=load_tpl_calc, outputs=req_input)
    tpl_pass.click(fn=load_tpl_pass, outputs=req_input)
    tpl_weather.click(fn=load_tpl_weather, outputs=req_input)
    
    # Dropdown select histories
    def on_recent_change(selected):
        if selected == "No recent prompts":
            return ""
        return selected
    recent_dropdown.change(fn=on_recent_change, inputs=recent_dropdown, outputs=req_input)
    
    # Refresh history dropdown
    def refresh_recent():
        prompts = get_recent_prompts()
        return gr.Dropdown(choices=prompts, value=prompts[0] if prompts else None)
    refresh_history_btn.click(fn=refresh_recent, outputs=recent_dropdown)
    
    # New Session reset hook
    new_session_btn.click(
        fn=new_session,
        outputs=[
            state_box, 
            planning_stats, 
            engineering_stats, 
            devops_stats, 
            management_stats, 
            total_cost_display, 
            call_log_table,
            req_display,
            gherkin_display,
            mermaid_display,
            spec_display,
            code_display,
            test_display,
            devops_display,
            analytics_display
        ]
    )
    
    # Run from web click
    run_btn.click(
        fn=run_from_web, 
        inputs=[req_input, model_dropdown, temp_slider], 
        outputs=[state_box]
    )

if __name__ == "__main__":
    demo.launch(
        server_name="127.0.0.1", 
        server_port=7860
    )
