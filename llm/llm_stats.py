import time
from typing import Optional

def calculate_cost(model: str, input_tokens: int, output_tokens: int, cache_hit_tokens: int = 0) -> float:
    model_lower = model.lower()
    
    # Defaults (used as fallbacks)
    input_rate = 0.15        # per 1M tokens
    cache_hit_rate = 0.015   # default 1/10th input rate
    output_rate = 0.60       # per 1M tokens
    has_cache_hit_rate = False
    
    if "deepseek-v4-flash" in model_lower or "deepseek-chat" in model_lower or "deepseek-v3" in model_lower or "deepseek-reasoner" in model_lower:
        input_rate = 0.14        # cache miss
        cache_hit_rate = 0.0028  # cache hit ($0.0028 per 1M)
        output_rate = 0.28       # output ($0.28 per 1M)
        has_cache_hit_rate = True
    elif "deepseek-v4-pro" in model_lower:
        input_rate = 0.435       # cache miss ($0.435 per 1M)
        cache_hit_rate = 0.003625 # cache hit ($0.003625 per 1M)
        output_rate = 0.87       # output ($0.87 per 1M)
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
    cost = (cache_miss_tokens / 1_000_000) * input_rate + (cache_hit_tokens / 1_000_000) * cache_hit_rate + (output_tokens / 1_000_000) * output_rate
    return cost

def update_token_stats(role: str, model_used: str, input_tokens: int, output_tokens: int, cache_hit_tokens: int = 0, where: Optional[str] = None, reasoning_chars: Optional[int] = None, reasoning_effort: Optional[str] = None, reasoning_budget: Optional[int] = None) -> None:
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
            
        cost = calculate_cost(model_used, input_tokens, output_tokens, cache_hit_tokens)
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
            # Reasoning (thinking-mode chain-of-thought) is billed as output but
            # discarded by the harness — recording its length lets us measure the
            # reasoning vs content split for cost tuning. Approx tokens = chars / 4.
            "reasoning_chars": reasoning_chars if reasoning_chars is not None else 0,
            "reasoning_effort": reasoning_effort,
            "reasoning_budget": reasoning_budget,
        })
        safe_update_state({"token_usage": usage})
        try:
            import os
            if os.environ.get("DEEP_AGENTS_EVAL_RUN") == "1":
                from eval_logger import save_to_gen5_log
                save_to_gen5_log(role, model_used, input_tokens, output_tokens, cache_hit_tokens, where, reasoning_chars, reasoning_effort, reasoning_budget)
        except Exception as log_err:
            print(f"Error logging to gen5_eval_token_log: {log_err}")
    except Exception as e:
        print(f"Error updating token stats: {e}")

