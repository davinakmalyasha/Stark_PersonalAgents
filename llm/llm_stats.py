import time
from typing import Optional

def calculate_cost(model: str, input_tokens: int, output_tokens: int, cache_hit_tokens: int = 0, reasoning_tokens: int = 0) -> float:
    """USD cost for one or more model requests.

    ``reasoning_tokens`` are billed at the output rate: DeepSeek-provider
    reasoning_content is NOT included in ``completion_tokens`` (observed:
    output=255 while reasoning_chars≈94k), so ignoring them under-counts long
    thinking-heavy runs by orders of magnitude.
    """
    model_lower = model.lower()
    
    # Defaults (used as fallbacks)
    input_rate = 0.15        # per 1M tokens
    cache_hit_rate = 0.015   # default 1/10th input rate
    output_rate = 0.60       # per 1M tokens
    has_cache_hit_rate = False
    
    if "deepseek-v4.1-flash" in model_lower or "deepseek-flash" in model_lower:
        input_rate = 0.30        # cache miss (peak; off-peak $0.15)
        cache_hit_rate = 0.006   # cache hit (peak; off-peak $0.003)
        output_rate = 1.20       # output (peak; off-peak $0.60)
        has_cache_hit_rate = True
    elif "deepseek-r1" in model_lower:
        input_rate = 0.55        # cache miss ($0.55 per 1M)
        cache_hit_rate = 0.14    # cache hit ($0.14 per 1M)
        output_rate = 2.19       # output ($2.19 per 1M)
        has_cache_hit_rate = True
    elif "gemini-1.5-flash" in model_lower:
        input_rate, output_rate = 0.075, 0.30
    elif "gemini-1.5-pro" in model_lower:
        input_rate, output_rate = 1.25, 5.00
    elif "gemini-2.5-flash" in model_lower or "gemini-3.5-flash" in model_lower:
        input_rate, output_rate = 0.075, 0.30
    elif "gemini-3.1-flash-lite" in model_lower or "gemini-2.0-flash-lite" in model_lower:
        input_rate, output_rate = 0.0375, 0.15
    elif "llama-3.1-8b" in model_lower or "llama3-8b" in model_lower:
        input_rate, output_rate = 0.05, 0.08
    elif "llama-3.3-70b" in model_lower or "llama3-70b" in model_lower:
        input_rate, output_rate = 0.59, 0.79
    elif "gpt-4o-mini" in model_lower:
        input_rate, output_rate = 0.150, 0.600
    elif "gpt-4o" in model_lower:
        input_rate, output_rate = 2.50, 10.00
    elif "gpt-3.5" in model_lower:
        input_rate, output_rate = 0.50, 1.50
    else:
        input_rate, output_rate = 0.15, 0.60
        
    if not has_cache_hit_rate:
        cache_hit_rate = input_rate * 0.1  # fallback to 90% discount
        
    cache_miss_tokens = max(0, input_tokens - cache_hit_tokens)
    billed_output = max(0, int(output_tokens or 0)) + max(0, int(reasoning_tokens or 0))
    cost = (cache_miss_tokens / 1_000_000) * input_rate + (cache_hit_tokens / 1_000_000) * cache_hit_rate + (billed_output / 1_000_000) * output_rate
    return cost

def update_token_stats(role: str, model_used: str, input_tokens: int, output_tokens: int, cache_hit_tokens: int = 0, where: Optional[str] = None, reasoning_chars: Optional[int] = None, reasoning_effort: Optional[str] = None, reasoning_budget: Optional[int] = None, cost_override: Optional[float] = None, backend: Optional[str] = None, extra: Optional[dict] = None) -> None:
    try:
        from sync.state_sync import safe_update_state, safe_get_state
        state = safe_get_state()
        usage = state.get("token_usage", {
            "total_input_tokens": 0, "total_output_tokens": 0,
            "total_cache_hit_tokens": 0, "total_cache_miss_tokens": 0,
            "total_cost": 0.0, "calls": []
        })
        
        # Backward compatibility check for existing runs/sessions
        if "total_cache_hit_tokens" not in usage:
            usage["total_cache_hit_tokens"] = 0
        if "total_cache_miss_tokens" not in usage:
            usage["total_cache_miss_tokens"] = 0
            
        # cost_override lets callers pin the cost (e.g. free-pool calls record
        # $0 instead of the default per-model rates). None = standard pricing.
        # Reasoning tokens are estimated from reasoning_chars (chars/4) and
        # billed at the output rate — see calculate_cost.
        reasoning_tokens = int((reasoning_chars or 0) // 4)
        cost = cost_override if cost_override is not None else calculate_cost(
            model_used, input_tokens, output_tokens, cache_hit_tokens, reasoning_tokens)
        cache_miss_tokens = max(0, input_tokens - cache_hit_tokens)
        
        usage["total_input_tokens"] += input_tokens
        usage["total_output_tokens"] += output_tokens
        usage["total_cache_hit_tokens"] += cache_hit_tokens
        usage["total_cache_miss_tokens"] += cache_miss_tokens
        usage["total_cost"] += cost

        # Infer where description if not provided
        if not where:
            if role == "Supervisor":
                where = "High-level SDLC planning & routing"
            elif role == "SupervisorSummary":
                where = "Synthesizing final team reports"
            elif role == "MemoryArchivist":
                where = "Archiving task context and episodic memory"
            elif role == "JarvisRouter":
                where = "Routing user request"
            else:
                where = "Executing coding agent loop"
        elif len(where) > 120:
            where = where[:117] + "..."
        
        usage["calls"].append({
            "agent": role, "model": model_used,
            "input": input_tokens, "output": output_tokens,
            "cache_hits": cache_hit_tokens,
            "cache_misses": cache_miss_tokens,
            "cost": round(cost, 6), "timestamp": time.strftime("%H:%M:%S"),
            "where": where,
            "backend": backend,
            # Reasoning (thinking-mode chain-of-thought) is billed as output but
            # discarded by the harness — recording its length lets us measure the
            # reasoning vs content split for cost tuning. Approx tokens = chars / 4.
            "reasoning_chars": reasoning_chars if reasoning_chars is not None else 0,
            "reasoning_effort": reasoning_effort,
            "reasoning_budget": reasoning_budget,
            **(extra or {}),
        })
        safe_update_state({"token_usage": usage})
        try:
            import os
            if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1":
                _append_gen5_call_log(role, model_used, input_tokens, output_tokens,
                                      cache_hit_tokens, where, reasoning_chars,
                                      reasoning_effort, reasoning_budget, cost, backend,
                                      extra=extra)
        except Exception as log_err:
            print(f"Error logging to gen5_eval_token_log: {log_err}")
    except Exception as e:
        print(f"Error updating token stats: {e}")


def _append_gen5_call_log(role: str, model_used: str, input_tokens: int,
                          output_tokens: int, cache_hit_tokens: int,
                          where: Optional[str], reasoning_chars: Optional[int],
                          reasoning_effort: Optional[str],
                          reasoning_budget: Optional[int],
                          cost: float, backend: Optional[str] = None,
                          extra: Optional[dict] = None) -> None:
    """Append one per-call record to .deep_agents/gen5_eval_calls.jsonl.

    Same record shape as the retired eval_logger.save_to_gen5_log (which the
    root cleanup deleted), written append-only: open, one line, close — no
    read-modify-write, safe under parallel eval workers. Only called during
    eval runs (DEEP_AGENTS_EVAL_RUN=1); production behavior unchanged.
    """
    import json as _json
    import os as _os
    import time as _time

    root = _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__)))
    # Per-task log (eval_runner sets BSA_CALLS_LOG) keeps concurrent/sequential
    # runs from mixing call records; the global file remains the fallback.
    calls_path = _os.environ.get("BSA_CALLS_LOG") or _os.path.join(
        root, ".deep_agents", "gen5_eval_calls.jsonl")
    _os.makedirs(_os.path.dirname(calls_path), exist_ok=True)

    w = where or ""
    if len(w) > 120:
        w = w[:117] + "..."
    record = {
        "agent": role,
        "model": model_used,
        "input": input_tokens,
        "output": output_tokens,
        "cache_hits": cache_hit_tokens,
        "cache_misses": max(0, input_tokens - cache_hit_tokens),
        "cost": round(cost, 6),
        "timestamp": _time.strftime("%H:%M:%S"),
        "where": w,
        "reasoning_chars": reasoning_chars if reasoning_chars is not None else 0,
        "reasoning_effort": reasoning_effort,
        "reasoning_budget": reasoning_budget,
        "backend": backend,
        **(extra or {}),
    }
    for _ in range(3):
        try:
            with open(calls_path, "a", encoding="utf-8") as f:
                f.write(_json.dumps(record, ensure_ascii=False) + "\n")
            break
        except IOError:
            _time.sleep(0.05)

