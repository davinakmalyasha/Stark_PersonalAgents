import os
import json
from langchain_core.messages import SystemMessage, HumanMessage, AIMessage, ToolMessage

def get_subagent_history_path(chat_id: str, subagent_name: str) -> str:
    """Returns the VFS scratch path for the subagent's conversation history."""
    from tools import _sanitize_path
    filename = f"subagent_history_{chat_id}_{subagent_name}.json"
    return _sanitize_path(f"/scratch/{filename}")

def load_subagent_history(chat_id: str, subagent_name: str) -> list:
    """Loads and rehydrates the subagent's previous message history."""
    path = get_subagent_history_path(chat_id, subagent_name)
    if os.path.isfile(path):
        try:
            with open(path, "r", encoding="utf-8") as f:
                data = json.load(f)
            messages = []
            for m in data:
                m_type = m.get("type")
                content = m.get("content")
                additional_kwargs = m.get("additional_kwargs", {})
                tool_calls = m.get("tool_calls") or []
                tool_call_id = m.get("tool_call_id") or None
                if m_type == "SystemMessage":
                    messages.append(SystemMessage(content=content, additional_kwargs=additional_kwargs))
                elif m_type == "HumanMessage":
                    messages.append(HumanMessage(content=content, additional_kwargs=additional_kwargs))
                elif m_type == "AIMessage":
                    messages.append(AIMessage(content=content, additional_kwargs=additional_kwargs,
                                              tool_calls=tool_calls or None))
                elif m_type == "ToolMessage":
                    messages.append(ToolMessage(content=content, additional_kwargs=additional_kwargs,
                                                tool_call_id=tool_call_id or ""))
            return messages
        except Exception as e:
            print(f"Error loading subagent history: {e}")
    return []

def save_subagent_history(chat_id: str, subagent_name: str, messages: list) -> None:
    """Persists the subagent's message history to VFS scratch folder."""
    path = get_subagent_history_path(chat_id, subagent_name)
    data = []
    for m in messages:
        data.append({
            "type": type(m).__name__,
            "content": m.content,
            "additional_kwargs": getattr(m, "additional_kwargs", {}),
            "tool_calls": getattr(m, "tool_calls", None),
            "tool_call_id": getattr(m, "tool_call_id", None),
        })
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8") as f:
            json.dump(data, f, indent=2)
    except Exception as e:
        print(f"Error saving subagent history: {e}")

def clear_subagent_history(chat_id: str, subagent_name: str) -> None:
    """Clears history file after successful completion."""
    path = get_subagent_history_path(chat_id, subagent_name)
    if os.path.isfile(path):
        try:
            os.remove(path)
        except Exception:
            pass
