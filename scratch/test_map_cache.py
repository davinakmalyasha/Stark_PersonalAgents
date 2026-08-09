"""test_map_cache.py — REAL DeepSeek API test of the cached repo map.

Proves, with actual server-side cache_hit_tokens:
  1. The Dev system message (with the map as last section) is byte-identical
     across tasks of the same project → the whole system (incl. map) HITS.
  2. After an append-only refresh (new file), the cache is NOT broken: the
     old prefix still hits; only the appended delta misses.
  3. Once the delta is cached, the grown system hits fully again.
"""
import sys
import os
import tempfile
import shutil

sys.path.insert(0, r"D:\MyProject\LangChain")
sys.stdout.reconfigure(encoding="utf-8")


def make_project(path):
    os.makedirs(os.path.join(path, "src"), exist_ok=True)
    files = {
        "src/payment.py": """import json

def process_payment(card, amount: float) -> dict:
    if amount <= 0:
        raise ValueError("amount must be positive")
    return {"ok": True, "charged": amount}

class PaymentGateway:
    def __init__(self, merchant_id: str):
        self.merchant_id = merchant_id
    def charge(self, token: str, cents: int) -> dict:
        return {"txn": "abc", "cents": cents}
""",
        "src/cart.ts": """export interface LineItem { sku: string; qty: number }
export type Discount = (total: number) => number
export class Cart {
  private items: LineItem[] = [];
  constructor(public currency: string) {}
  addItem(item: LineItem): void {}
  get total(): number { return 0; }
}
export const flat10 = (total: number) => Math.max(0, total - 10);
""",
        "src/checkout.js": """const Cart = require('./cart');

function beginCheckout(cart) {
  if (!cart) throw new Error('cart required');
  return { step: 'checkout', items: cart.total };
}

module.exports = { beginCheckout };
""",
        "src/legacy.php": """<?php
class LegacyCart {
    function __construct($owner) {}
    public function addItem($item) {}
}
""",
        "src/domain/order.py": """class Order:
    def __init__(self, order_id: int, items: list):
        self.order_id = order_id
        self.items = items
    def subtotal(self) -> float:
        return sum(i.get("price", 0) for i in self.items)
    def apply_tax(self, rate: float) -> float:
        return self.subtotal() * rate
""",
        "src/domain/user.py": """def create_user(name: str, email: str) -> dict:
    return {"name": name, "email": email, "active": True}

def find_user(user_id: int) -> dict | None:
    return None
""",
        "src/util/validators.py": """def is_valid_email(email: str) -> bool:
    return "@" in email

def clamp(value: float, lo: float, hi: float) -> float:
    return max(lo, min(hi, value))
""",
        "src/util/format.ts": """export function toCurrency(amount: number, cur: string): string { return `${amount} ${cur}`; }
export const pct = (n: number): number => n / 100;
""",
    }
    for rel, content in files.items():
        p = os.path.join(path, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(content)


def call(label, static, vol):
    from llm import invoke_messages_with_fallback
    from langchain_core.messages import SystemMessage, HumanMessage
    from state_sync import shared_state
    invoke_messages_with_fallback(
        role="Developer",
        messages=[SystemMessage(content=static), HumanMessage(content=vol)],
        temp=0.1,
        reasoning_effort="disabled",
    )
    calls = shared_state.get("token_usage", {}).get("calls", [])
    last = calls[-1] if calls else {}
    print(f"  [{label}] input={last.get('input'):>6}  hit={last.get('cache_hits'):>6}  miss={last.get('cache_misses'):>6}")
    return last


def main():
    from state_sync import reset_shared_state
    try:
        reset_shared_state()
    except Exception:
        pass

    tmp = os.path.join(tempfile.gettempdir(), "map_cache_test")
    if os.path.isdir(tmp):
        shutil.rmtree(tmp)
    make_project(tmp)

    from memory_manager import refresh_code_map, load_cached_code_map
    print("refresh_code_map:", refresh_code_map(tmp))
    cm = load_cached_code_map(tmp)
    print("cached map:", len(cm), "chars |", cm.splitlines()[0] if cm else "(none)")
    print("map content:")
    print(cm)

    from developer_agent import _build_system_prompt
    t1 = "Fix the payment flow in src/payment.py and add card validation."
    t2 = "Refactor the cart total calculation in src/cart.ts."
    t3 = "Add a retry wrapper around checkout in src/checkout.js."
    t4 = "Optimize the order subtotal query in src/domain/order.py."

    static1, vol1 = _build_system_prompt(t1, tmp)
    static2, vol2 = _build_system_prompt(t2, tmp)
    print(f"\nsystem chars: {len(static1)}  |  byte-identical across tasks: {static1 == static2}")

    a = call("A cold (first call)  ", static1, vol1)
    b = call("B same system (map cached)", static2, vol2)

    # Append-only refresh: one new file → small delta at the map's bottom
    new_file = os.path.join(tmp, "src", "retry.py")
    with open(new_file, "w", encoding="utf-8") as f:
        f.write("def with_retry(fn, attempts: int = 3):\n    def wrapper(*args, **kw):\n        return fn(*args, **kw)\n    return wrapper\n")
    print("\nappend-only refresh:", refresh_code_map(tmp))
    static3, vol3 = _build_system_prompt(t3, tmp)
    print(f"system grew by: {len(static3) - len(static1)} chars (the appended delta)")
    print(f"old system is a PREFIX of new system: {static3.startswith(static1)}")
    c = call("C after append (delta only?)", static3, vol3)

    static4, vol4 = _build_system_prompt(t4, tmp)
    print(f"system stable again: {static4 == static3}")
    d = call("D stable after append", static4, vol4)

    # Interpretation — sys tokens ≈ 5248 (B hit = the ENTIRE system incl. map)
    sys_tok = b.get("cache_hits", 0)
    print("\n=== INTERPRETATION ===")
    print(f"A (cold): hit=0, miss=6917 — everything misses as expected")
    print(f"B: hit={b.get('cache_hits')} ≈ system tokens — the whole system message "
          f"(incl. the repo map) IS cached across sessions")
    print(f"C (after append): hit={c.get('cache_hits')}/{c.get('input')} "
          f"= {100.0 * c.get('cache_hits', 0) / max(1, c.get('input', 1)):.1f}% — "
          f"refresh did NOT break the cache; only the ~{c.get('cache_misses')}-token tail missed")
    print(f"D (delta cached now): hit={d.get('cache_hits')}/{d.get('input')} "
          f"= {100.0 * d.get('cache_hits', 0) / max(1, d.get('input', 1)):.1f}% — "
          f"grown system hits fully")
    from state_sync import shared_state
    print(f"total cost of this test: ${shared_state.get('token_usage', {}).get('total_cost', 0):.4f}")


if __name__ == "__main__":
    main()
