"""cache_diagnose.py — surgical cache hit/miss diagnostic.

Measures EVERY cache-relevant variable in isolation with the FULL 24-tool
native schemas, then re-measures the key scenarios with LEAN schemas
(auto-compressed descriptions) to prove the fix magnitude.

Scenarios:
  W1  session start (fresh project, first call)   -> miss = tools + per-task bytes
  R1  same request repeat (same config)           -> full hit (tools cached)
  H1  config transition (disabled -> high)        -> miss = tools + per-task bytes (transition cost)
  R2  flip back (high -> disabled)                -> hit again
  T1  cross-project (different map/card, same tools/config) -> tools re-miss?
  R3  repeat                                      -> hit
  T2  same project, different task text (H diff)  -> how much does H-variation cost?
  EX  same request, different tools SUBSET        -> tools-variation cost

Report: per-call input/hit/miss + attribution (message tokens via tiktoken,
tools tokens = input - message tokens) + per-component cost table + a
simulated 183-task extrapolation for CURRENT vs LEAN schemas.
"""
import sys
import os
import shutil
import tempfile
import json

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.stdout.reconfigure(encoding="utf-8")

from sync.state_sync import reset_shared_state, safe_update_state
try:
    reset_shared_state()
except Exception:
    pass

from llm import invoke_messages_with_fallback
from langchain_core.messages import SystemMessage, HumanMessage
from tools_pkg import get_native_tools

import tiktoken
_ENC = tiktoken.encoding_for_model("gpt-4o")


def msg_tokens(msgs) -> int:
    total = 0
    for m in msgs:
        total += len(_ENC.encode(str(getattr(m, "content", ""))))
        total += 4  # role overhead
    return total


def make_project(tag: str, files: dict):
    tmp = os.path.join(tempfile.gettempdir(), f"cache_diag_{tag}")
    if os.path.isdir(tmp):
        shutil.rmtree(tmp)
    os.makedirs(tmp)
    for rel, content in files.items():
        p = os.path.join(tmp, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w", encoding="utf-8") as f:
            f.write(content)
    from memory.memory_manager import refresh_code_map, ensure_project_identity
    refresh_code_map(tmp)
    ensure_project_identity(tmp, name=f"diag_{tag}", description="diagnostic fixture",
                            purpose="measure cache behavior", status="stable")
    safe_update_state({"project_path": tmp})
    return tmp


def build(tag, task_text, tools):
    from agents.developer_agent import _build_system_prompt
    S, V = _build_system_prompt(task_text, tag)
    H = f"{task_text}\n\n{V}"
    return [SystemMessage(content=S), HumanMessage(content=H)], S, H


class Recorder:
    def __init__(self):
        self.rows = []

    def call(self, label, msgs, effort, tools):
        invoke_messages_with_fallback(role="Developer", messages=list(msgs), temp=0.2,
                                      reasoning_effort=effort, tools=tools, where=label)
        from sync.state_sync import shared_state
        c = shared_state["token_usage"]["calls"][-1]
        m_tok = msg_tokens(msgs)
        t_tok = max(0, c["input"] - m_tok)
        self.rows.append({
            "label": label, "effort": effort, "input": c["input"],
            "hit": c["cache_hits"], "miss": c["cache_misses"],
            "msg_tokens": m_tok, "tools_tokens": t_tok,
        })
        print(f"  [{label:6s}] {effort:8s} in={c['input']/1000:6.1f}k hit={c['cache_hits']/1000:6.1f}k "
              f"miss={c['cache_misses']/1000:6.1f}k  (msg≈{m_tok/1000:.1f}k tools≈{t_tok/1000:.1f}k)")
        return c


def lean_tools(tools):
    """Programmatic compression of every schema description to one crisp line —
    mirrors the planned real fix, without editing tool_schemas.py yet."""
    out = []
    for t in tools:
        fn = dict(t["function"])
        desc = fn.get("description", "")
        first = desc.split(".")[0].strip()[:120] if desc else ""
        fn["description"] = first
        params = fn.get("parameters", {}).get("properties", {})
        new_params = {}
        for pname, pinfo in params.items():
            np = {k: v for k, v in pinfo.items() if k != "description"}
            out_desc = str(pinfo.get("description", ""))[:60]
            np["description"] = out_desc
            new_params[pname] = np
        fn["parameters"] = {"type": "object", "properties": new_params,
                            "required": fn.get("parameters", {}).get("required", [])}
        out.append({"type": "function", "function": fn})
    return out


FILES_A = {
    "src/cart.js": "const { store } = require('./store');\nfunction addItem(item) { store.push(item); }\nfunction total() { return store.reduce((a, b) => a + b.price, 0); }\nmodule.exports = { addItem, total };\n",
    "src/store.js": "const store = [];\nmodule.exports = { store };\n",
    "src/checkout.js": "const { total } = require('./cart');\nfunction checkout() { return { amount: total() }; }\nmodule.exports = { checkout };\n",
    "src/discounts.py": "def apply(code, amount):\n    codes = {'SAVE10': 0.1}\n    return amount * (1 - codes.get(code, 0))\n",
    "src/taxes.py": "def with_tax(amount, rate=0.11):\n    return round(amount * (1 + rate), 2)\n",
    "src/order.py": "class Order:\n    def __init__(self, items):\n        self.items = items\n    def subtotal(self):\n        return sum(i['price'] for i in self.items)\n",
    "package.json": '{"name": "cart-app", "dependencies": {"express": "^4.18"}}\n',
}
FILES_B = {
    "src/auth.py": "def login(user, pw):\n    return user == 'admin' and pw == 'secret'\n",
    "src/session.py": "class Session:\n    def __init__(self):\n        self.user = None\n    def set(self, u):\n        self.user = u\n",
    "src/tokens.ts": "export function sign(payload: object): string { return 'tok.' + JSON.stringify(payload); }\nexport function verify(tok: string): boolean { return tok.startsWith('tok.'); }\n",
    "package.json": '{"name": "auth-app", "dependencies": {"jsonwebtoken": "^9"}}\n',
}
FILES_C = {
    "src/api.py": "def handle(req):\n    return {'status': 200, 'body': req}\n",
    "src/db.py": "def connect():\n    return {'ok': True}\n",
    "src/logger.js": "function log(msg) { console.log('[api]', msg); }\nmodule.exports = { log };\n",
    "package.json": '{"name": "api-app"}\n',
}


def run_sequence(rec, tools, tag_suffix):
    full_tools = tools
    t_tok_full = len(json.dumps(full_tools)) // 4
    projA = make_project("A" + tag_suffix, FILES_A)
    projB = make_project("B" + tag_suffix, FILES_B)

    msgsA, SA, HA = build(projA, "The cart total is wrong when discounts apply.", full_tools)
    print(f"\n--- tools payload ≈ {t_tok_full} tokens ---")
    print("== FULL-tools sequence ==")
    rec.call("W1", msgsA, "disabled", full_tools)
    rec.call("R1", msgsA, "disabled", full_tools)
    rec.call("H1", msgsA, "high", full_tools)
    rec.call("R2", msgsA, "disabled", full_tools)

    msgsB, SB, HB = build(projB, "The login endpoint crashes on bad passwords.", full_tools)
    rec.call("T1", msgsB, "disabled", full_tools)
    rec.call("R3", msgsB, "disabled", full_tools)

    msgsA2, _, _ = build(projA, "The invoice page shows floating point noise.", full_tools)
    rec.call("T2", msgsA2, "disabled", full_tools)

    subset = [t for t in full_tools if t["function"]["name"] in
              ("read_file", "list_files", "search_code")]
    rec.call("EX", msgsA, "disabled", subset)


def report(rec, tools_tokens_full, tools_tokens_lean):
    print("\n" + "=" * 78)
    print("CACHE DIAGNOSTIC REPORT")
    print("=" * 78)
    rows = rec.rows
    by = {}
    for r in rows:
        by[r["label"]] = r

    def miss(label):
        return by[label]["miss"]

    print(f"message+tools attribution (avg over calls): "
          f"msg≈{sum(r['msg_tokens'] for r in rows)//len(rows)/1000:.1f}k  "
          f"tools≈{sum(r['tools_tokens'] for r in rows)//len(rows)/1000:.1f}k")
    print(f"\ntools block token cost: FULL={tools_tokens_full}  LEAN={tools_tokens_lean}  "
          f"(saving {tools_tokens_full - tools_tokens_lean})")

    print("\ncomponent costs (FULL schemas):")
    print(f"  session-start miss (W1)          = {miss('W1')/1000:.2f}k  (tools {tools_tokens_full/1000:.2f}k + bytes {max(0,miss('W1')-tools_tokens_full)/1000:.2f}k)")
    print(f"  in-session repeat (R1)           = {miss('R1')/1000:.2f}k  (fully cached)")
    print(f"  config transition (H1)           = {miss('H1')/1000:.2f}k  (tools re-missed)")
    print(f"  flip back (R2)                   = {miss('R2')/1000:.2f}k")
    print(f"  cross-project session (T1)       = {miss('T1')/1000:.2f}k  (tools re-missed across tasks)")
    print(f"  cross-project repeat (R3)        = {miss('R3')/1000:.2f}k")
    print(f"  same project, new task (T2)      = {miss('T2')/1000:.2f}k  (H-variation cost)")
    print(f"  tools-subset change (EX)         = {miss('EX')/1000:.2f}k  (tools-variation cost)")

    # Extrapolate a 183-task run: tasks have ~2 config sessions (iter-1 + iter-2/transition) + flips
    per_task = miss('W1') + miss('H1')  # session start + first transition
    flips = 1.2 * miss('H1')  # average ~1.2 flips/task (v18 flip frequency)
    per_task_full = per_task + flips
    per_task_lean = 2 * (tools_tokens_lean + max(0, miss('W1') - tools_tokens_full)) + 1.2 * (tools_tokens_lean + max(0, miss('W1') - tools_tokens_full))
    print("\nextrapolation for a 183-task run (v18-style flip frequency):")
    print(f"  FULL schemas: ~{per_task_full/1000:.1f}k/task -> {per_task_full*183/1e6:.2f}M miss")
    print(f"  LEAN schemas: ~{per_task_lean/1000:.1f}k/task -> {per_task_lean*183/1e6:.2f}M miss  (saving {(per_task_full-per_task_lean)*183/1e6:.2f}M)")


def main():
    rec = Recorder()
    full = get_native_tools()
    lean = lean_tools(full)
    t_full = len(json.dumps(full)) // 4
    t_lean = len(json.dumps(lean)) // 4

    run_sequence(rec, full, "f")
    # lean sequence: fresh projects so bytes don't collide with the full run
    rec2 = Recorder()
    run_sequence(rec2, lean, "l")
    print("\n--- LEAN-schemas sequence (fresh fixtures) ---")
    for r in rec2.rows:
        print(f"  [{r['label']:6s}] {r['effort']:8s} miss={r['miss']/1000:6.1f}k")

    report(rec, t_full, t_lean)
    print("\nLEAN key misses: W1=%.1fk H1=%.1fk T1=%.1fk (vs FULL %.1fk/%.1fk/%.1fk)" % (
        rec2.rows[0]["miss"]/1000, rec2.rows[2]["miss"]/1000, rec2.rows[4]["miss"]/1000,
        rec.rows[0]["miss"]/1000, rec.rows[2]["miss"]/1000, rec.rows[4]["miss"]/1000))


if __name__ == "__main__":
    main()
