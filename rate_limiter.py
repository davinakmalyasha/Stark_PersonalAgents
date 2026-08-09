import functools
import os
import threading
import time
from typing import Any, Callable, TypeVar, cast

F = TypeVar("F", bound=Callable[..., Any])

class TokenBucket:
    """Thread-safe Token Bucket rate limiter."""
    def __init__(self, capacity: float, refill_rate: float) -> None:
        self.capacity: float = capacity
        self.refill_rate: float = refill_rate  # tokens per second
        self.tokens: float = capacity
        self.last_refill: float = time.time()
        self.lock: threading.Lock = threading.Lock()

    def consume(self, amount: float = 1.0) -> float:
        """
        Consume 'amount' tokens.
        If tokens are available, returns 0.0 (no wait required).
        Otherwise, returns the number of seconds to sleep to get enough tokens.
        """
        with self.lock:
            now = time.time()
            elapsed = now - self.last_refill
            self.tokens = min(self.capacity, self.tokens + elapsed * self.refill_rate)
            self.last_refill = now

            if self.tokens >= amount:
                self.tokens -= amount
                return 0.0
            else:
                needed = amount - self.tokens
                wait_time = needed / self.refill_rate
                self.tokens = 0.0
                self.last_refill = now + wait_time
                return wait_time

# Configure and instantiate a global token bucket for all LLM calls
capacity: float = float(os.environ.get("DEEP_AGENTS_RATE_LIMIT_CAPACITY", "5.0"))
refill_rate: float = float(os.environ.get("DEEP_AGENTS_RATE_LIMIT_REFILL_RATE", "1.0"))
global_bucket: TokenBucket = TokenBucket(capacity, refill_rate)

def token_bucket_limit(func: F) -> F:
    """Decorator that wraps LLM calls to rate limit and queue them using a global token bucket."""
    @functools.wraps(func)
    def wrapper(*args: Any, **kwargs: Any) -> Any:
        wait_time = global_bucket.consume(1.0)
        if wait_time > 0:
            from llm import log_terminal
            log_terminal(f"[Token Bucket] Throttling request — waiting {wait_time:.2f}s for rate-limit token...\n")
            time.sleep(wait_time)
        return func(*args, **kwargs)
    return cast(F, wrapper)
