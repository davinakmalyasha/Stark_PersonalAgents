import os
import json
from datetime import datetime
from dotenv import load_dotenv

load_dotenv(override=True)

CHAT_MEMORY_PATH = r"d:\MyProject\LangChain\.deep_agents\chat_memory.json"

def load_chat_memory(workspace_path: str = "", chat_id: str = "") -> dict:
    """Loads the chat memory JSON, initializing structure from chat file or global fallback."""
    default_structure = {
        "daily_summaries": [],
        "current_day_messages": [],
        "last_active_date": ""
    }
    if workspace_path and chat_id:
        try:
            from workspace_manager import get_chat
            chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
            if chat:
                return {
                    "daily_summaries": chat.get("daily_summaries", []),
                    "current_day_messages": chat.get("current_day_messages", []),
                    "last_active_date": chat.get("last_active_date", "")
                }
        except Exception as e:
            print(f"Error loading chat memory for {chat_id}: {e}")
            
    if os.path.isfile(CHAT_MEMORY_PATH):
        try:
            with open(CHAT_MEMORY_PATH, "r", encoding="utf-8") as f:
                data = json.load(f)
                # Ensure structure
                for k in default_structure:
                    if k not in data:
                        data[k] = default_structure[k]
                return data
        except Exception:
            pass
    return default_structure

def save_chat_memory(data: dict, workspace_path: str = "", chat_id: str = "") -> None:
    """Saves the chat memory data back to workspace chat file or global fallback."""
    if workspace_path and chat_id:
        try:
            from workspace_manager import get_chat, save_chat
            chat = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
            if chat:
                chat["daily_summaries"] = data.get("daily_summaries", [])
                chat["current_day_messages"] = data.get("current_day_messages", [])
                chat["last_active_date"] = data.get("last_active_date", "")
                save_chat(workspace_path, chat_id, chat)
                return
        except Exception as e:
            print(f"Error saving chat memory to {chat_id}: {e}")

    # Fallback to global path
    try:
        os.makedirs(os.path.dirname(CHAT_MEMORY_PATH), exist_ok=True)
        with open(CHAT_MEMORY_PATH, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=4)
    except Exception as e:
        print(f"Error saving chat memory: {e}")

def add_chat_message(sender: str, text: str, workspace_path: str = "", chat_id: str = "") -> None:
    """Adds a message to the current day's log, performing summarization if the date changes."""
    mem = load_chat_memory(workspace_path, chat_id)
    today_str = datetime.now().strftime("%Y-%m-%d")
    
    if mem["last_active_date"] and mem["last_active_date"] != today_str:
        if mem["current_day_messages"]:
            summarize_previous_day(mem)
            
    mem["last_active_date"] = today_str
    mem["current_day_messages"].append({
        "sender": sender,
        "text": text,
        "timestamp": datetime.now().isoformat()
    })

    # ── Secondary check: Session-based summarization ──
    # If the message count in the current session/day exceeds 35, trigger a sub-summarization
    # of the first 25 messages to keep the session size bounded and prevent bloating.
    if len(mem.get("current_day_messages", [])) > 35:
        try:
            summarize_session_chunk(mem)
        except Exception as e:
            print(f"Error doing session-based summarization chunk: {e}")

    save_chat_memory(mem, workspace_path, chat_id)


def summarize_session_chunk(mem: dict) -> None:
    """Summarizes the oldest 25 messages in the current session and keeps the rest intact."""
    raw_msgs = mem.get("current_day_messages", [])
    to_summarize = raw_msgs[:25]
    remaining = raw_msgs[25:]
    
    filtered_msgs = []
    for m in to_summarize:
        text = m.get("text", "").strip()
        sender = m.get("sender", "").upper()
        if not text or (sender == "USER" and is_continue_command(text)):
            continue
        filtered_msgs.append(f"{sender}: {text}")
        
    if filtered_msgs:
        messages_str = "\n".join(filtered_msgs)
        sys_inst = (
            "You are a Senior Archivist. Summarize the provided chat history segment into a concise paragraph "
            "retaining all crucial details, user requests, files mentioned, and systems discussed."
        )
        prompt = f"Chat log segment:\n{messages_str}\n\nConcisely summarize this segment:"
        try:
            summary = _call_llm(sys_inst, prompt)
        except Exception:
            summary = "Summary unavailable (execution error)."
    else:
        summary = "No development actions or build requests performed."
        
    # Append to daily summaries as a mid-day milestone
    date_str = mem["last_active_date"]
    mem.setdefault("daily_summaries", []).append({
        "date": f"{date_str} (Part {len(mem.get('daily_summaries', [])) + 1})",
        "summary": summary
    })
    
    # Keep the remaining messages
    mem["current_day_messages"] = remaining

def summarize_previous_day(mem: dict) -> None:
    """Summarizes the current day's messages and appends it to daily_summaries, then clears current messages."""
    date = mem["last_active_date"]
    raw_msgs = mem.get("current_day_messages", [])
    
    # Filter out continuation commands, greetings, and empty messages
    filtered_msgs = []
    for m in raw_msgs:
        text = m.get("text", "").strip()
        sender = m.get("sender", "").upper()
        if not text:
            continue
        # Skip standard continue commands
        if sender == "USER" and is_continue_command(text):
            continue
        # Skip simple greetings
        text_lower = text.lower()
        if any(text_lower.startswith(g) for g in ["hello", "hi ", "hey", "what's up"]):
            if len(text_lower.split()) <= 4:
                continue
        filtered_msgs.append(f"{sender}: {text}")
        
    # If no meaningful development messages remain, bypass the LLM entirely
    if not filtered_msgs:
        summary = "No development actions or build requests performed."
    else:
        messages_str = "\n".join(filtered_msgs)
        
        sys_inst = (
            "You are a Senior Archivist. Summarize the provided chat history of one day into a concise paragraph "
            "retaining all crucial details, user requests, files mentioned, and systems discussed."
        )
        prompt = f"Chat log for {date}:\n{messages_str}\n\nConcisely summarize this day's events:"
        
        try:
            summary = _call_llm(sys_inst, prompt)
        except Exception:
            summary = "Summary unavailable (execution error)."

    mem["daily_summaries"].append({
        "date": date,
        "summary": summary
    })
    mem["current_day_messages"] = []

def _call_llm(sys_inst: str, prompt: str) -> str:
    """Calls Gemini or Groq directly to summarize chat history."""
    from llm import invoke_with_fallback
    try:
        return invoke_with_fallback(
            role="ChatSummarizer",
            sys_inst=sys_inst,
            prompt=prompt,
            temp=0.3
        )
    except Exception:
        return "Summary unavailable (no API response)."

def is_continue_command(text: str) -> bool:
    """Checks if a user message is a variant of 'continue', 'resume', 'go on', etc."""
    t = text.lower().strip().rstrip(".!?")
    if not t:
        return False
    # Simple direct matches
    continue_words = {
        "continue", "resume", "proceed", "go on", "go", "yes", "confirm", "approve",
        "do it", "run it", "looks good", "yessss continueee", "go on continue",
        "continue please", "please continue", "yesss continueee", "continueee"
    }
    if t in continue_words:
        return True
    # Word based check: if it contains resume/continue keywords and is short, or starts with them
    words = t.split()
    if len(words) <= 5 and any(w in words for w in ["continue", "resume", "proceed", "go"]):
        return True
    # If it's a long continuation string
    if any(phrase in t for phrase in ["continue from where", "resume from where", "continue where we left", "go on continue", "continue last"]):
        return True
    return False

def resolve_request_text(req: str, workspace_path: str = "", chat_id: str = "") -> str:
    """If the request is a 'resume' or 'continue' command, resolves it to the last active request in history."""
    import re
    text_lower = req.lower().strip().rstrip(".!?")
    
    # If the request is a planning command, do not resolve it to history
    if bool(re.search(r"(?:^|\s)/plan(?:\s|$|\b|,)", text_lower)):
        return req
        
    is_resume = is_continue_command(text_lower)
    
    if is_resume:
        # Check if task.json exists in progress. If it does, we always resolve to executing the plan for this task,
        # since it is the active, incomplete task. This correctly handles mid-task pivots and resumes.
        try:
            from sync_helpers import load_task_tracking
            task_data = load_task_tracking(workspace_path, chat_id)
            if task_data and task_data.get("status") == "in_progress":
                goal = task_data.get("user_request") or ""
                # Strip progress blocks
                goal = goal.split("=== TASK PROGRESS")[0].strip()
                # Strip /plan
                goal = re.sub(r"(?:^|\s)/plan(?:\s|$|\b|,)", " ", goal, flags=re.IGNORECASE).strip()
                goal = re.sub(r"\s*,\s*", " ", goal).strip()
                if goal:
                    return f"Execute the plan from planning.md for: {goal}"
        except Exception as e:
            print(f"Error checking active task.json: {e}")

        # First, try to scan the chat history file directly if we have chat_id, to keep context within THIS session
        if workspace_path and chat_id:
            try:
                from workspace_manager import get_chat
                chat_data = get_chat(workspace_path, chat_id, include_traces=False, include_usage=False)
                if chat_data:
                    # Keywords to skip — meta-questions about the bot, greetings, continue commands
                    meta_patterns = [
                        "couldnt you read", "could you read", "why cant you", "why can't you",
                        "dont you remember", "don't you remember", "do you remember",
                        "why the agent", "why agent", "couldnt you", "could you not",
                        "u sure?", "are you sure",
                    ]
                    for m in reversed(chat_data.get("messages", [])):
                        role = m.get("role") or m.get("sender") or ""
                        content = m.get("content") or m.get("text") or ""
                        if role == "user" and content:
                            content_stripped = content.strip().rstrip(".!?").lower()
                            # Skip continue commands
                            if is_continue_command(content_stripped):
                                continue
                            # Skip meta-questions about the bot
                            if any(pattern in content_stripped for pattern in meta_patterns):
                                continue
                            # Skip if not a valid build request (greetings, guidance, etc.)
                            from memory_manager import is_valid_build_request
                            if not is_valid_build_request(content):
                                continue
                            
                            # Check if the found user message was a /plan command
                            if bool(re.search(r"(?:^|\s)/plan(?:\s|$|\b|,)", content.lower())):
                                goal = re.sub(r"(?:^|\s)/plan(?:\s|$|\b|,)", " ", content, flags=re.IGNORECASE).strip()
                                goal = re.sub(r"\s*,\s*", " ", goal).strip()
                                return f"Execute the plan from planning.md for: {goal}"
                            return content
            except Exception as e:
                print(f"Error extracting last request from chat history: {e}")

        # Fallback to scanning the supervisor memory file if chat history is empty/unavailable
        from memory_io import load_memory
        mem = load_memory(workspace_path)
        past_requests = mem.get("past_requests", [])
        if past_requests:
            last_req = past_requests[-1]
            if bool(re.search(r"(?:^|\s)/plan(?:\s|$|\b|,)", last_req.lower())):
                goal = re.sub(r"(?:^|\s)/plan(?:\s|$|\b|,)", " ", last_req, flags=re.IGNORECASE).strip()
                # Clean up duplicate whitespace or commas
                goal = re.sub(r"\s*,\s*", " ", goal).strip()
                return f"Execute the plan from planning.md for: {goal}"
            return last_req
            
    # 2. Check if the text matches plan answers, option selection, or contains a go on/proceed indicator (multi-turn guidance)
    has_numbering = bool(re.search(r"^\s*\d+[\.\)]", text_lower))
    is_option_select = text_lower in ("a", "b", "c", "d") or any(text_lower == f"option {x}" for x in ("a", "b", "c", "d"))
    ends_with_go = any(text_lower.endswith(x) for x in ["go on", "proceed", "go"])
    contains_guidance_indicators = any(x in text_lower for x in ["sounds good", "yes", "no", "separate", "basic", "tailwind", "confirm", "proceed"])
    
    if has_numbering or is_option_select or ends_with_go or contains_guidance_indicators:
        from memory_io import load_memory
        mem = load_memory(workspace_path)
        past_requests = mem.get("past_requests", [])
        if past_requests:
            last_request = past_requests[-1]
            # Avoid appending guidance repeatedly if they match
            if "(User Guidance:" not in last_request:
                return f"{last_request} (User Guidance: {req})"
                
    return req
