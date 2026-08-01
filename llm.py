"""
LLM Invocation — DeepSeek-only with model-level fallback.
Uses the OpenAI-compatible DeepSeek API at https://api.deepseek.com.
"""
import json
import os
import time
from typing import Any, Optional, Type, TypeVar

from dotenv import load_dotenv
from pydantic import BaseModel
from langchain_openai import ChatOpenAI
from langchain_core.language_models.chat_models import BaseChatModel

# Monkeypatch langchain_openai to preserve reasoning_content for DeepSeek prompt caching and response parsing
try:
    import langchain_openai.chat_models.base as base_mod

    # 1. Outgoing messages (for prompt caching)
    orig_convert = base_mod._convert_message_to_dict
    def patched_convert(message, *args, **kwargs):
        res_dict = orig_convert(message, *args, **kwargs)
        if hasattr(message, "additional_kwargs") and "reasoning_content" in message.additional_kwargs:
            res_dict["reasoning_content"] = message.additional_kwargs["reasoning_content"]
        return res_dict
    base_mod._convert_message_to_dict = patched_convert

    # 2. Incoming messages (for parsing reasoning_content in non-streaming responses)
    orig_convert_dict = base_mod._convert_dict_to_message
    def patched_convert_dict(_dict, *args, **kwargs):
        msg = orig_convert_dict(_dict, *args, **kwargs)
        if isinstance(_dict, dict) and "reasoning_content" in _dict:
            msg.additional_kwargs["reasoning_content"] = _dict["reasoning_content"]
        return msg
    base_mod._convert_dict_to_message = patched_convert_dict

    # 3. Incoming streaming chunks (for parsing reasoning_content in streaming responses)
    orig_convert_delta = base_mod._convert_delta_to_message_chunk
    def patched_convert_delta(_dict, *args, **kwargs):
        msg_chunk = orig_convert_delta(_dict, *args, **kwargs)
        if isinstance(_dict, dict) and "reasoning_content" in _dict:
            msg_chunk.additional_kwargs["reasoning_content"] = _dict["reasoning_content"]
        return msg_chunk
    base_mod._convert_delta_to_message_chunk = patched_convert_delta
except Exception:
    pass

from llm_config import TASK_CATEGORIES, get_task_category

class StrWithMetadata(str):
    def __new__(cls, value, reasoning=None):
        obj = super().__new__(cls, value)
        obj.reasoning_content = reasoning
        return obj

from llm_stats import TokenUsageTracker, update_token_stats
# rate_limiter removed — no artificial API throttling. The model decides pacing.

load_dotenv(override=True)
T = TypeVar("T", bound=BaseModel)

DEEPSEEK_BASE_URL = "https://api.deepseek.com"


def _is_off_peak() -> bool:
    """Check if the current time is in the DeepSeek off-peak discount window (16:30-00:30 UTC next day)."""
    import datetime
    now_utc = datetime.datetime.now(datetime.timezone.utc).time()
    start = datetime.time(16, 30)
    end = datetime.time(0, 30)
    if start <= now_utc or now_utc <= end:
        return True
    return False


def log_terminal(msg: str) -> None:
    """Log to shared_state live_terminal_log. Console logging is best-effort."""
    try:
        print(msg, flush=True)
    except (UnicodeEncodeError, OSError):
        try:
            safe = msg.encode("ascii", errors="replace").decode()
            print(safe, flush=True)
        except OSError:
            pass  # console is gone — log to shared_state only
    try:
        from state_sync import safe_append_live_log
        safe_append_live_log(msg)
    except Exception:
        pass


def _extract_text(content: Any) -> str:
    """Safely extract text from LLM response content."""
    if isinstance(content, list):
        return "".join(
            p.get("text", str(p)) if isinstance(p, dict) else str(p)
            for p in content
        )
    return str(content)


def _extract_native_tool_calls(res) -> list[dict]:
    """Extract native function calling tool_calls from an LLM response.

    Returns list of {tool, args} dicts in our canonical format.
    Handles both LangChain's parsed tool_calls and raw additional_kwargs.
    """
    tool_calls = []

    # Path 1: LangChain AIMessage.tool_calls (parsed)
    if hasattr(res, 'tool_calls') and res.tool_calls:
        for tc in res.tool_calls:
            try:
                name = tc.get('name', '') if isinstance(tc, dict) else getattr(tc, 'name', '')
                args = tc.get('args', {}) if isinstance(tc, dict) else getattr(tc, 'args', {})
                if isinstance(args, str):
                    args = json.loads(args)
                if name:
                    tool_calls.append({"tool": name, "args": args})
            except Exception:
                pass

    # Path 2: Raw additional_kwargs from DeepSeek API response
    if not tool_calls:
        raw_tool_calls = res.additional_kwargs.get('tool_calls', []) if hasattr(res, 'additional_kwargs') else []
        for tc in raw_tool_calls:
            try:
                func = tc.get('function', {}) if isinstance(tc, dict) else {}
                name = func.get('name', '')
                args_str = func.get('arguments', '{}')
                args = json.loads(args_str) if isinstance(args_str, str) else args_str
                if name:
                    tool_calls.append({"tool": name, "args": args})
            except Exception:
                pass

    return tool_calls


def _get_deepseek_client(model: str, temp: float, role: Optional[str] = None, response_format: Optional[dict] = None, tools: Optional[list[dict]] = None, reasoning_effort: Optional[str] = None, messages: Optional[list] = None) -> BaseChatModel:
    """Create a DeepSeek ChatOpenAI client with optional native tools."""
    key = os.getenv("DEEPSEEK_API_KEY", "")
    if not key or key.startswith("your_"):
        raise ValueError(
            "DEEPSEEK_API_KEY not set in .env. "
            "Get one at https://platform.deepseek.com/api_keys"
        )

    # No per-call max_tokens — the model decides how much to output.
    # The context compaction system handles the window, the LLM handles pacing.

    kwargs: dict[str, Any] = {
        "model": model,
        "api_key": key,
        "base_url": DEEPSEEK_BASE_URL,
        "timeout": 120.0,
        "max_retries": 1,
        "temperature": temp,
    }
    if response_format is not None:
        kwargs.setdefault("model_kwargs", {})["response_format"] = response_format
    if tools:
        # Pass tools via model_kwargs for DeepSeek native function calling
        kwargs.setdefault("model_kwargs", {})["tools"] = tools

    # Configure native thinking mode dynamically by role/model.
    # Only send thinking parameters if the model supports it or is a fallback model.
    is_reasoning_model = any(x in model.lower() for x in ["r1", "pro", "o1", "o3", "reasoning", "thinking", "fallback", "automatic", "flash"])

    if is_reasoning_model:
        if reasoning_effort is not None:
            if reasoning_effort == "disabled":
                kwargs["extra_body"] = {"thinking": {"type": "disabled"}}
            else:
                kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
                kwargs["reasoning_effort"] = reasoning_effort
        else:
            is_orchestrator = (
                role in ["Developer", "DeveloperFixing"]
                or (role and any(x in role.lower() for x in ["developer", "orchestrator", "designer"]))
            )
            is_heavy = is_orchestrator or (
                role in ["Supervisor", "sa", "Coder"]
                or (role and any(x in role.lower() for x in ["fixing", "coder", "architect", "complex"]))
            )

            if is_orchestrator:
                kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
                kwargs["reasoning_effort"] = "high"
            elif is_heavy:
                kwargs["extra_body"] = {"thinking": {"type": "enabled"}}
                kwargs["reasoning_effort"] = "high"
            else:
                kwargs["extra_body"] = {"thinking": {"type": "disabled"}}

    return ChatOpenAI(**kwargs)



def _clear_failed_models() -> None:
    """Clear the failed models set so each new request starts fresh."""
    try:
        from state_sync import safe_update_state
        safe_update_state({"failed_models": []})
    except Exception:
        pass


def _parse_schema_response(text: str, schema: Type[T]) -> T:
    """Helper to parse a raw JSON string from the LLM into a Pydantic model."""
    import json
    import re

    # Try to find JSON in markdown code block
    json_match = re.search(r"```(?:json)?\s*(\{.*?\})\s*```", text, re.DOTALL)
    if json_match:
        text = json_match.group(1)
    else:
        # Try to find the first '{' and last '}'
        start = text.find("{")
        end = text.rfind("}")
        if start != -1 and end != -1:
            text = text[start : end + 1]

    text = text.strip()
    
    # Escape invalid single backslashes (like PHP namespaces \Sanctum\) to prevent JSON decode errors
    # but avoid corrupting already double-escaped backslashes.
    pattern = re.compile(r'\\\\|\\"|\\/|\\b|\\f|\\n|\\r|\\t|\\u[0-9a-fA-F]{4}|\\')
    def replace_invalid_escape(match):
        val = match.group(0)
        if val == '\\':
            return '\\\\'
        return val
    text = pattern.sub(replace_invalid_escape, text)
    
    # Parse JSON
    try:
        data = json.loads(text)
    except json.JSONDecodeError as e:
        log_terminal(f"JSON parsing failed. Raw response was: {text}\n")
        raise ValueError(f"Failed to parse JSON response: {e}")

    # Validate schema
    if hasattr(schema, "model_validate"):
        return schema.model_validate(data)
    elif hasattr(schema, "parse_obj"):
        return schema.parse_obj(data)
    else:
        return schema(**data)


import threading

class ConcurrencyThrottle:
    def __init__(self):
        self.lock = threading.Lock()
        self.active_requests = 0
        self.max_concurrency = 4
        self.cooldown_until = 0.0
        
    def acquire(self):
        # Spin/wait if active requests exceed limit
        while True:
            with self.lock:
                now = time.time()
                is_economic = os.environ.get("DEEP_AGENTS_LLM_MODE", "").upper() == "ECONOMIC"
                limit = 1 if (is_economic or now < self.cooldown_until) else self.max_concurrency
                if self.active_requests < limit:
                    self.active_requests += 1
                    break
            time.sleep(0.1)
            
    def release(self):
        with self.lock:
            self.active_requests = max(0, self.active_requests - 1)
            
    def trigger_cooldown(self, duration=30):
        with self.lock:
            self.cooldown_until = time.time() + duration

_throttle = ConcurrencyThrottle()


def trim_prompt(text: str) -> str:
    """Pre-processes prompts to strip markdown comments, trailing spaces, and redundant newlines in a single pass."""
    import re
    if not isinstance(text, str) or not text:
        return text
    # 1. Strip markdown HTML comments: <!-- comment -->
    text = re.sub(r"<!--[\s\S]*?-->", "", text)
    # 2. Strip trailing spaces from each line
    lines = [line.rstrip() for line in text.splitlines()]
    # 3. Collapse multiple consecutive empty lines to a single empty line
    collapsed_lines = []
    for line in lines:
        if not line:
            if collapsed_lines and collapsed_lines[-1]:  # only keep one empty line
                collapsed_lines.append("")
        else:
            collapsed_lines.append(line)
    return "\n".join(collapsed_lines).strip()


def _handle_rate_limit(err_str: str, attempt: int) -> int:
    """Calculates backoff wait time, parsing Retry-After headers if available."""
    import re
    retry_after = 0
    # Search for retry-after in headers or text
    match = re.search(r"retry[-_]after[:\s]+(\d+)", err_str, re.IGNORECASE)
    if not match:
        match = re.search(r"after\s+(\d+)\s+seconds", err_str, re.IGNORECASE)
    if match:
        try:
            retry_after = int(match.group(1))
        except Exception:
            pass
    
    # Exponential backoff: 2 * (2 ** attempt)
    exp_backoff = 2 * (2 ** attempt)
    wait = max(retry_after, exp_backoff)
    return min(wait, 30)  # cap at 30s


def invoke_with_fallback(
    role: str,
    sys_inst: str,
    prompt: str,
    schema: Optional[Type[T]] = None,
    temp: float = 0.7,
    where: Optional[str] = None,
) -> Any:
    """Invoke LLM with DeepSeek fallback chain. Supports structured output."""
    

    category = get_task_category(role)
    queue = TASK_CATEGORIES[category].copy()
    
    # Under ECONOMIC mode, strip out any 'pro' models from the queue
    if os.environ.get("DEEP_AGENTS_LLM_MODE", "").upper() == "ECONOMIC":
        queue = [item for item in queue if "pro" not in item["model"].lower()]
        
    selected_temp = temp

    try:
        from state_sync import safe_get_state
        state = safe_get_state()
        selected_temp = state.get("selected_temp", temp)
    except Exception:
        pass

    # Use direct messages instead of ChatPromptTemplate to avoid
    # curly braces in JSON content being parsed as template variables.
    from langchain_core.messages import SystemMessage, HumanMessage

    errors_by_model: dict[str, str] = {}

    for item in queue:
        model = item["model"]
        try:
            off_peak = " [Off-Peak Discount Active]" if _is_off_peak() else ""
            if role != "MemoryArchivist":
                # Show what the agent is working on, not just the model name
                action = where[:120].replace('\n', ' ').strip() if where else "processing..."
                log_terminal(f"[{role}] {action}")
            response_format = {"type": "json_object"} if schema else None

            # If schema requested, add JSON guidelines to prompt and enable native response_format
            effective_sys = trim_prompt(sys_inst)
            effective_prompt = trim_prompt(prompt)
            if schema:
                fields = schema.model_fields
                field_desc = ", ".join(
                    f'"{k}": <{f.annotation.__name__ if f.annotation else "str"}>'
                    for k, f in fields.items()
                )
                effective_sys += (
                    f"\n\nYou MUST respond with ONLY a valid JSON object "
                    f"matching this schema: {{{field_desc}}}. "
                    f"No markdown, no explanation, just the raw JSON."
                )

            messages = [
                SystemMessage(content=effective_sys),
                HumanMessage(content=effective_prompt),
            ]

            llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, messages=messages)
            tracker = TokenUsageTracker()

            _throttle.acquire()
            try:
                res = llm.invoke(
                    messages,
                    config={"callbacks": [tracker]},
                )
            finally:
                _throttle.release()
            reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
            if reasoning:
                log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
            raw_text = _extract_text(getattr(res, "content", None))
            if not raw_text and reasoning:
                raw_text = reasoning

            if schema:
                val = _parse_schema_response(raw_text, schema)
            else:
                val = raw_text

            update_token_stats(
                role, model, tracker.input_tokens, tracker.output_tokens, tracker.cache_hit_tokens, where
            )

            # Log a smart summary instead of dumping the entire response
            try:
                from state_sync import safe_append_live_log
                if not schema:
                    val_str = str(val)
                    # Extract tool calls for a concise summary
                    import re as _re
                    tools_called = _re.findall(r'"tool":\s*"(\w+)"', val_str)
                    if tools_called:
                        safe_append_live_log(f"  → Tools: {', '.join(tools_called)}")
                    elif len(val_str) < 200:
                        safe_append_live_log(val_str)
                    else:
                        safe_append_live_log(f"  → Response: {len(val_str)} chars")
            except Exception:
                pass

            return val

        except Exception as e:
            err_str = str(e)
            errors_by_model[model] = err_str[:300]
            log_terminal(f"[{model}] Failed: {err_str}\n")

            # If rate-limited, wait and retry
            if "429" in err_str or "rate" in err_str.lower():
                _throttle.trigger_cooldown(30)
                for attempt in range(3):
                    wait = _handle_rate_limit(err_str, attempt)
                    log_terminal(
                        f"[{model}] Rate limited (attempt {attempt+1}/3) — waiting {wait}s...\n"
                    )
                    time.sleep(wait)
                    try:
                        effective_sys = trim_prompt(sys_inst)
                        if schema:
                            fields = schema.model_fields
                            field_desc = ", ".join(
                                f'"{k}": <{f.annotation.__name__ if f.annotation else "str"}>'
                                for k, f in fields.items()
                            )
                            effective_sys += (
                                f"\n\nYou MUST respond with ONLY a valid JSON object "
                                f"matching this schema: {{{field_desc}}}. "
                                f"No markdown, no explanation, just the raw JSON."
                            )
                        retry_msgs = [
                            SystemMessage(content=effective_sys),
                            HumanMessage(content=trim_prompt(prompt)),
                        ]
                        llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, messages=retry_msgs)
                        tracker = TokenUsageTracker()
                        _throttle.acquire()
                        try:
                            res = llm.invoke(
                                retry_msgs,
                                config={"callbacks": [tracker]},
                            )
                        finally:
                            _throttle.release()
                        reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
                        if reasoning:
                            log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
                        raw_text = _extract_text(getattr(res, "content", res))
                        if schema:
                            val = _parse_schema_response(raw_text, schema)
                        else:
                            val = raw_text
                        update_token_stats(
                            role, model,
                            tracker.input_tokens, tracker.output_tokens,
                            tracker.cache_hit_tokens,
                            where,
                        )
                        return val
                    except Exception as retry_e:
                        err_str = str(retry_e)
                        errors_by_model[model] = f"Rate-limit retry {attempt+1} failed: {err_str[:200]}"
                        log_terminal(f"[{model}] Retry {attempt+1} failed: {retry_e}\n")
                        if not ("429" in err_str or "rate" in err_str.lower()):
                            break

    # Build detailed error
    detail = "; ".join(f"{m}: {e}" for m, e in errors_by_model.items())
    raise RuntimeError(
        f"All DeepSeek models exhausted for role '{role}'. "
        f"Errors: {detail}. "
        "Check your DEEPSEEK_API_KEY and account balance at https://platform.deepseek.com."
    )





def _compact_prompt(text: str) -> str:
    if not text:
        return text
    import re
    lines = text.splitlines()
    compacted_lines = []
    in_code_block = False
    for line in lines:
        trimmed = line.strip()
        if trimmed.startswith("```"):
            in_code_block = not in_code_block
            compacted_lines.append(line)
            continue
        
        if in_code_block:
            compacted_lines.append(line)
        else:
            collapsed = re.sub(r'[ \t]+', ' ', line).strip()
            if collapsed:
                compacted_lines.append(collapsed)
            else:
                if compacted_lines and compacted_lines[-1] != "":
                    compacted_lines.append("")
                    
    while compacted_lines and compacted_lines[-1] == "":
        compacted_lines.pop()
        
    return "\n".join(compacted_lines)


def invoke_messages_with_fallback(
    role: str,
    messages: list,
    temp: float = 0.7,
    where: Optional[str] = None,
    schema: Optional[Any] = None,
    tools: Optional[list[dict]] = None,
    reasoning_effort: Optional[str] = None,
    model: Optional[str] = None,
) -> Any:
    """
    Invoke LLM with multi-turn messages and optional native function calling.
    Returns (content_text, tool_calls) tuple when tools are provided,
    or just content_text string when tools=None (legacy mode).
    """
    

    category = get_task_category(role)
    queue = TASK_CATEGORIES[category].copy()
    if model:
        normalized_model = model.replace(":", "-")
        queue = [{"provider": "deepseek", "model": normalized_model}] + [x for x in queue if x["model"] != normalized_model]
    selected_temp = temp

    try:
        from state_sync import safe_get_state
        state = safe_get_state()
        selected_temp = state.get("selected_temp", temp)
    except Exception:
        pass

    errors_by_model: dict[str, str] = {}

    trimmed_messages = []
    for msg in messages:
        if hasattr(msg, "content") and isinstance(msg.content, str):
            content = trim_prompt(msg.content)
            if type(msg).__name__ == "SystemMessage":
                content = _compact_prompt(content)
            
            # Recreate preserving all fields like id, name, tool_calls, additional_kwargs
            kwargs = {}
            for field in ["id", "name", "tool_calls", "additional_kwargs", "tool_call_id"]:
                if hasattr(msg, field):
                    kwargs[field] = getattr(msg, field)
            trimmed_messages.append(type(msg)(content=content, **kwargs))
        else:
            trimmed_messages.append(msg)

    if schema is not None:
        import json
        schema_json = ""
        if hasattr(schema, "schema_json"):
            schema_json = schema.schema_json()
        elif hasattr(schema, "model_json_schema"):
            schema_json = json.dumps(schema.model_json_schema())
        else:
            schema_json = str(schema)
        
        guideline = f"\n\nCRITICAL: You MUST respond with a JSON object matching this schema:\n{schema_json}"
        if trimmed_messages:
            last_msg = trimmed_messages[-1]
            if hasattr(last_msg, "content"):
                kwargs = {}
                for field in ["id", "name", "tool_calls", "additional_kwargs", "tool_call_id"]:
                    if hasattr(last_msg, field):
                        kwargs[field] = getattr(last_msg, field)
                trimmed_messages[-1] = type(last_msg)(content=str(last_msg.content) + guideline, **kwargs)

    for item in queue:
        model = item["model"]
        try:
            off_peak = " [Off-Peak Discount Active]" if _is_off_peak() else ""
            if role != "MemoryArchivist":
                # Show what the agent is working on, not just the model name
                action = where[:120].replace('\n', ' ').strip() if where else "processing..."
                log_terminal(f"[{role}] {action}")
            response_format = {"type": "json_object"} if schema is not None else None
            llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, tools=tools, reasoning_effort=reasoning_effort, messages=trimmed_messages)
            tracker = TokenUsageTracker()
            _throttle.acquire()
            try:
                res = llm.invoke(trimmed_messages, config={"callbacks": [tracker]})
            finally:
                _throttle.release()
            reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
            if reasoning:
                log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
            # Extract native tool_calls if present (structured function calling)
            native_tool_calls = _extract_native_tool_calls(res)
            # DeepSeek with thinking enabled may return reasoning_content but empty content.
            # Fall back to reasoning_content if content is empty.
            val = _extract_text(getattr(res, "content", None))
            if not val and reasoning:
                val = reasoning
            update_token_stats(
                role, model, tracker.input_tokens, tracker.output_tokens, tracker.cache_hit_tokens, where
            )
            # Return structured result: tuple with (text, tool_calls, reasoning) if tools active
            if tools is not None:
                if native_tool_calls:
                    return (val or "", native_tool_calls, reasoning)
                elif val:
                    return (val, [], reasoning)  # Text response, no tool calls — agent is done
                else:
                    log_terminal(f"[{model}] Empty response, trying next...\n")
            else:
                if val:
                    if schema is not None:
                        val = _parse_schema_response(val, schema)
                    else:
                        val = StrWithMetadata(val, reasoning=reasoning)
                    return val
                log_terminal(f"[{model}] Empty response, trying next...\n")
        except Exception as e:
            import traceback
            tb = traceback.format_exc()
            err_str = str(e)
            errors_by_model[model] = err_str[:300]
            log_terminal(f"[{model}] Failed: {err_str}\n{tb}\n")

            # Rate-limit retry
            if "429" in err_str or "rate" in err_str.lower():
                _throttle.trigger_cooldown(30)
                for attempt in range(3):
                    wait = _handle_rate_limit(err_str, attempt)
                    log_terminal(
                        f"[{model}] Rate limited (attempt {attempt+1}/3) — waiting {wait}s...\n"
                    )
                    time.sleep(wait)
                    try:
                        response_format = {"type": "json_object"} if schema is not None else None
                        llm = _get_deepseek_client(model, selected_temp, role, response_format=response_format, tools=tools, reasoning_effort=reasoning_effort, messages=trimmed_messages)
                        tracker = TokenUsageTracker()
                        _throttle.acquire()
                        try:
                            res = llm.invoke(
                                trimmed_messages, config={"callbacks": [tracker]}
                            )
                        finally:
                            _throttle.release()
                        reasoning = res.additional_kwargs.get("reasoning_content") if hasattr(res, "additional_kwargs") else None
                        if reasoning:
                            log_terminal(f"\n[{role} THOUGHTS] >> {reasoning}\n")
                        native_tool_calls = _extract_native_tool_calls(res)
                        val = _extract_text(getattr(res, "content", None))
                        if not val and reasoning:
                            val = reasoning
                        update_token_stats(
                            role, model,
                            tracker.input_tokens, tracker.output_tokens,
                            tracker.cache_hit_tokens,
                            where,
                        )
                        if tools is not None:
                            if native_tool_calls:
                                return (val or "", native_tool_calls, reasoning)
                            elif val:
                                return (val, [], reasoning)
                        elif val:
                            if schema is not None:
                                val = _parse_schema_response(val, schema)
                            else:
                                val = StrWithMetadata(val, reasoning=reasoning)
                            return val
                    except Exception as retry_e:
                        err_str = str(retry_e)
                        errors_by_model[model] = f"Rate-limit retry {attempt+1} failed: {err_str[:200]}"
                        log_terminal(f"[{model}] Retry {attempt+1} failed: {retry_e}\n")
                        if not ("429" in err_str or "rate" in err_str.lower()):
                            break

    detail = "; ".join(f"{m}: {e}" for m, e in errors_by_model.items())
    raise RuntimeError(
        f"All DeepSeek models exhausted for role '{role}'. "
        f"Errors: {detail}. "
        "Check your DEEPSEEK_API_KEY and account balance at https://platform.deepseek.com."
    )


def get_llm() -> BaseChatModel:
    """Get a default LLM client (for legacy callers)."""
    key = os.getenv("DEEPSEEK_API_KEY", "")
    if key and not key.startswith("your_"):
        return ChatOpenAI(
            model="deepseek-v4-flash",
            api_key=key,
            base_url=DEEPSEEK_BASE_URL,
            temperature=0.1,
        )

    class MockChatModel(BaseChatModel):
        def _generate(self, messages, stop=None, run_manager=None, **kwargs):
            from langchain_core.outputs import ChatResult, ChatGeneration
            from langchain_core.messages import AIMessage
            return ChatResult(
                generations=[
                    ChatGeneration(message=AIMessage(content="Mock response"))
                ]
            )

        @property
        def _llm_type(self) -> str:
            return "mock"

    return MockChatModel()
