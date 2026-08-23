"""
Centralized Chat Context Builder.
Provides a single utility to load and format chat history for agent prompts.
"""

import re


def build_chat_context(
    workspace_path: str,
    chat_id: str,
    max_messages: int = 20,
    max_chars_per_msg: int = 2000,
    exclude_traces: bool = True,
) -> str:
    """Build a context string from recent chat history for agent prompts.

    User messages are included with smart truncation. Agent messages are included
    with tool-call and thinking blocks stripped, truncated to max_chars_per_msg,
    and labeled [PREVIOUS AGENT RESPONSE]. Agent trace messages (isTrace: true)
    are always skipped when exclude_traces is set.
    """
    if not workspace_path or not chat_id:
        return ""
    try:
        from workspace_manager import get_chat
        # General mode: resolve empty path to the general workspace
        resolved_path = workspace_path or "_general"
        chat_data = get_chat(resolved_path, chat_id, include_traces=False, include_usage=False)
        if not chat_data:
            return ""

        messages = chat_data.get("messages", [])
        recent = messages[-max_messages:]

        lines = []
        for m in recent:
            role = m.get("role") or m.get("sender") or ""
            text = m.get("content") or m.get("text") or ""

            # Always skip trace-heavy agent messages (subagent tool logs)
            if exclude_traces and role == "agent" and m.get("metadata", {}).get("isTrace"):
                continue
            if not role or not text:
                continue

            if role == "agent":
                # Keep the useful conversational text.  Strip only historical
                # pseudo-tool/thinking blocks so old persisted chats cannot
                # reintroduce a retired protocol into a native-tool turn.
                clean = re.sub(r'```tool\s*\n.*?\n```', '', text, flags=re.DOTALL)
                clean = re.sub(r'<tool>.*?</tool>', '', clean, flags=re.DOTALL)
                clean = re.sub(r'<thinking>.*?</thinking>', '', clean, flags=re.DOTALL)
                clean = clean.strip()
                if not clean:
                    continue
                truncated = clean[:max_chars_per_msg]
                if len(clean) > max_chars_per_msg:
                    truncated = clean[:200] + f"\n...[{len(clean) - 300} chars truncated]...\n" + clean[-100:]
                lines.append(f"[PREVIOUS AGENT RESPONSE]:\n{truncated}")
                continue

            # User messages: keep but truncate if needed
            if len(text) > max_chars_per_msg:
                first_part = text[:200]
                last_part = text[-100:]
                text = f"{first_part}\n...[{len(text) - 300} chars truncated]...\n{last_part}"

            lines.append(f"[{role.upper()}]: {text}")

        return "\n".join(lines) if lines else ""
    except Exception as e:
        print(f"[CHAT CONTEXT] Error building context: {e}")
        return ""
