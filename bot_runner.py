import os
import time
import threading
from state_sync import shared_state, safe_get_state
from sync_manager import run_and_sync_graph
from bot_helpers import safe_send_message, safe_edit_message_text

WORKSPACE_DIR = r"D:\MyProject"

def format_telegram_status(s: dict) -> str:
    """Formats shared_state into a simple, clean Telegram status message."""
    active_node = s.get("active_node")
    if active_node and active_node != "agent":
        return f"⏳ Working: {active_node.upper()} Specialist is in progress..."
    return "⏳ Processing your request..."

def send_deliverables(bot, chat_id: int, msg_id: int, req: str) -> None:
    """Sends all generated Word docs and project info via Telegram."""
    import voice_services
    doc_files = [
        "1_Requirements.docx",
        "2_TechnicalSpec.docx",
        "Analytics_Report.docx",
    ]
    for doc_name in doc_files:
        doc_path = os.path.join(WORKSPACE_DIR, doc_name)
        if os.path.exists(doc_path):
            try:
                with open(doc_path, "rb") as f:
                    bot.send_document(chat_id, f)
            except Exception as e:
                print(f"Error sending {doc_name}: {e}")

    project_path = shared_state.get("project_path", "")
    if project_path and os.path.isdir(project_path):
        safe_send_message(
            bot,
            chat_id,
            f"📁 *Project successfully established:*\n`{project_path}`\n\n"
            f"All source code is prepared for your review in the directory above.",
            parse_mode="Markdown",
        )

    summary = f"Workflow successfully executed: {req}."
    tts_path = os.path.join(WORKSPACE_DIR, f"reply_{msg_id}.mp3")
    voice_services.generate_tts_sync(summary, tts_path)
    if os.path.exists(tts_path):
        try:
            with open(tts_path, "rb") as f:
                bot.send_voice(chat_id, f, caption="🎙️ Summary")
        except Exception as e:
            print(f"Error sending TTS voice: {e}")
        try:
            os.remove(tts_path)
        except Exception:
            pass

def handle_build_request(bot, req: str, chat_id: int, status_msg) -> None:
    """Runs the full agentic pipeline and sends live status updates."""
    from memory_manager import is_valid_build_request
    from chat_memory_manager import add_chat_message

    if status_msg is None:
        status_msg = safe_send_message(bot, chat_id, "⏳ Starting the execution pipeline...")

    is_running = True
    def runner() -> None:
        nonlocal is_running
        try:
            for _ in run_and_sync_graph(req, workspace_path=WORKSPACE_DIR, chat_id=str(chat_id)):
                pass
        finally:
            is_running = False

    threading.Thread(target=runner, daemon=True).start()

    last_text = ""
    while is_running:
        txt = format_telegram_status(safe_get_state())
        if txt != last_text:
            safe_edit_message_text(bot, txt, chat_id, status_msg.message_id)
            last_text = txt
        time.sleep(2)

    # Final status update
    txt = format_telegram_status(safe_get_state())
    if txt != last_text:
        safe_edit_message_text(bot, txt, chat_id, status_msg.message_id)

    # Retrieve and send agent_report
    state = safe_get_state()
    agent_report = state.get("outputs", {}).get("agent_report", "")

    # Only send deliverables if it is a valid build request and files were actually updated/created
    was_updated = (
        state.get("code_updated", False) or
        state.get("requirements_updated", False) or
        state.get("tech_spec_updated", False)
    )

    if agent_report:
        # Add the report to chat memory
        add_chat_message("jarvis", agent_report, workspace_path=WORKSPACE_DIR, chat_id=str(chat_id))
        
        if not was_updated:
            import re
            clean_reply = agent_report
            if "## Summary" in agent_report:
                match = re.search(r"## Summary\s*\n(.*?)(?=\n##|$)", agent_report, re.DOTALL)
                if match:
                    clean_reply = match.group(1).strip()
            
            # Replace the "Thinking..." status message with the clean response directly!
            sent = safe_edit_message_text(bot, clean_reply, chat_id, status_msg.message_id, parse_mode="Markdown")
            if not sent or sent is True:
                sent_msg = safe_send_message(bot, chat_id, clean_reply, parse_mode="Markdown")
                if sent_msg is None:
                    safe_send_message(bot, chat_id, clean_reply)
        else:
            # Send full report with Markdown, fallback to plain text if Telegram fails
            sent = safe_send_message(bot, chat_id, agent_report, parse_mode="Markdown")
            if sent is None:
                safe_send_message(bot, chat_id, agent_report)

    if is_valid_build_request(req) and was_updated:
        safe_send_message(bot, chat_id, "🎉 Commands fully executed. Delivering assets now.")
        send_deliverables(bot, chat_id, status_msg.message_id, req)

