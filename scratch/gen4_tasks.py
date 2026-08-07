"""gen4_tasks.py — 60-task "gen4" suite, harder in every axis.

  gen4-trap   (15) : nastier traps — the fix for bug A CREATES bug B, decoy "already fixed"
                     comments, misleading names, hidden state, timezones, prototype chain...
  gen4-chain  (15) : 5 mini-repos x 3 sequential tasks with harder cumulative invariants
                     (limits, denormalized state, auth, caching, TTL).
  gen4-log    (15) : deeper debug-from-log — multi-line tracebacks, misleading messages,
                     errors pointing at the wrong frame, stateful crashes.
  gen4-long   (15) : GENERATED long-horizon projects — 25-45 files with filler code so the
                     context genuinely grows; bug buried in one module. Stresses compaction,
                     working memory, and exploration efficiency.

`build_tasks(H)` receives the helper dict from eval_runner (avoids circular import).
Every task carries `_fixed` (hand-written correct version) used ONLY by the pre-flight
verifier; the agent never sees it.
"""
import os


def _pkg():
    return '{"name": "eval-app", "main": "server.js", "dependencies": { "express": "^4.19.0" } }\n'


def _expr(extra, deps=""):
    return (
        "const express = require('express');\n"
        "const app = express();\n"
        "app.use(express.json());\n"
        + extra +
        deps +
        "\nmodule.exports = app;\n"
    )


def build_tasks(H):
    write = H["write"]
    ensure_deps = H["ensure_deps"]
    rt = H["require_test"]
    rat = H["require_async_test"]
    api = H["api_test"]
    ref = H["refactor_test"]

    def express_setup(files):
        return lambda d: (write(d, files), ensure_deps(d))

    tasks = []

    # ══════════════════════════════════════════════════════════════════════
    # gen4-trap — the fix creates a NEW bug
    # ══════════════════════════════════════════════════════════════════════
    tasks.append({
        "id": "g4t-fix-creates-bug",
        "category": "gen4-trap",
        "prompt": "utils.js exports getUser(id, cache) — it returns a cached user when present, "
                  "else fetches. BUG A: when the cache has NO entry, it returns undefined instead "
                  "of fetching. If you fix it by ALWAYS returning the fetched user you create BUG B: "
                  "the cache is never used. Fix getUser so it fetches on a miss AND uses the cache "
                  "on a hit.",
        "setup": lambda d: write(d, {"utils.js":
            "async function fetchUser(id) { return { id, name: 'user' + id }; }\n"
            "async function getUser(id, cache) {\n"
            "  const hit = await cache.get(id);\n"
            "  if (!hit) return undefined;  // BUG A: never fetches on miss\n"
            "  return hit;\n"
            "}\n"
            "module.exports = { getUser };\n",
            }),
        "test": lambda d: rat(d, "utils",
            "    const cache = { store: {}, get: async (k) => cache.store[k] || null, set: async (k, v) => { cache.store[k] = v; } };\n"
            "    const u = await m.getUser(1, cache);\n"
            "    if (!u || u.name !== 'user1') throw new Error('miss fetch got ' + JSON.stringify(u));\n"
            "    if (!cache.store[1]) throw new Error('miss must be cached');\n"
            "    const u2 = await m.getUser(1, cache);\n"
            "    if (u2 !== u) throw new Error('hit must use cache');\n"
            "    const u3 = await m.getUser(2, cache);\n"
            "    if (!u3 || u3.name !== 'user2' || !cache.store[2]) throw new Error('second miss got ' + JSON.stringify(u3));\n"),
        "_fixed": {"utils.js":
            "async function fetchUser(id) { return { id, name: 'user' + id }; }\n"
            "async function getUser(id, cache) {\n"
            "  const hit = await cache.get(id);\n"
            "  if (hit) return hit;\n"
            "  const u = await fetchUser(id);\n"
            "  await cache.set(id, u);\n"
            "  return u;\n"
            "}\n"
            "module.exports = { getUser };\n"},
    })
    tasks.append({
        "id": "g4t-decoy-fixed",
        "category": "gen4-trap",
        "prompt": "utils.js exports formatPhone(p) which must return digits only (formatted). "
                  "There is a DECOY: a comment claims the bug was already fixed, but the code only "
                  "strips spaces/parens/dashes — it keeps the '+' prefix "
                  "(formatPhone('+1-555-0100') === '15550100'). Trust the REQUIREMENT, not the "
                  "comment. Fix it.",
        "setup": lambda d: write(d, {"utils.js":
            "function formatPhone(p) {\n"
            "  // FIXED: no more stripping issues — verified working, do not change\n"
            "  return (p || '').replace(/[() -]/g, '');  // BUG: keeps '+'\n"
            "}\n"
            "module.exports = { formatPhone };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.formatPhone('+1-555-0100') !== '15550100') throw new Error('got ' + m.formatPhone('+1-555-0100'));\n"
            "if (m.formatPhone('(555) 123-4567') !== '5551234567') throw new Error('parens got ' + m.formatPhone('(555) 123-4567'));\n"
            "if (m.formatPhone('+1 (800) 555-1234') !== '18005551234') throw new Error('spaced got ' + m.formatPhone('+1 (800) 555-1234'));\n"
            "if (m.formatPhone('++5') !== '5') throw new Error('double-plus got ' + m.formatPhone('++5'));\n"
            "if (m.formatPhone('123') !== '123') throw new Error('plain got ' + m.formatPhone('123'));\n"
            "if (m.formatPhone('') !== '') throw new Error('empty got ' + m.formatPhone(''));\n"),
        "_fixed": {"utils.js":
            "function formatPhone(p) { return (p || '').replace(/\\D/g, ''); }\n"
            "module.exports = { formatPhone };\n"},
    })
    tasks.append({
        "id": "g4t-shadowing-state",
        "category": "gen4-trap",
        "prompt": "utils.js exports register(name) which appends to a shared registry and returns "
                  "the count. A function-scoped `registry` shadows the module-level one, so every "
                  "call starts fresh (count is always 1). Remove the shadow so the module-level "
                  "registry accumulates.",
        "setup": lambda d: write(d, {"utils.js":
            "const registry = [];\n"
            "function register(name) { const registry = registry || []; registry.push(name); return registry.length; }\n"
            "module.exports = { register };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.register('a') !== 1) throw new Error('1st got ' + m.register('a'));\n"
            "if (m.register('b') !== 2) throw new Error('2nd got ' + m.register('b'));\n"
            "if (m.register('c') !== 3) throw new Error('3rd got ' + m.register('c'));\n"
            "if (m.register('d') !== 4) throw new Error('4th got ' + m.register('d'));\n"
            "if (m.register('e') !== 5) throw new Error('5th got ' + m.register('e'));\n"),
        "_fixed": {"utils.js":
            "const registry = [];\n"
            "function register(name) { registry.push(name); return registry.length; }\n"
            "module.exports = { register };\n"},
    })
    tasks.append({
        "id": "g4t-tz-trap",
        "category": "gen4-trap",
        "prompt": "utils.js exports dayOfWeek(ms) which must return the UTC day name (0=Sunday) "
                  "for an epoch timestamp. It currently uses LOCAL getters, so the day flips "
                  "around midnight local time. Fix it to use UTC getters.",
        "setup": lambda d: write(d, {"utils.js":
            "function dayOfWeek(ms) { return new Date(ms).getDay(); }  // BUG: local tz\n"
            "module.exports = { dayOfWeek };\n"}),
        "test": lambda d: rt(d, "utils",
            "// 2026-01-05 00:30 UTC = Monday(1). Local (e.g. +8) is 08:30 same day — a UTC fix returns 1.\n"
            "if (m.dayOfWeek(Date.UTC(2026, 0, 5, 0, 30)) !== 1) throw new Error('got ' + m.dayOfWeek(Date.UTC(2026, 0, 5, 0, 30)));\n"
            "// 2026-01-05 23:30 UTC = Monday(1); in +8 local it is already Tuesday(2)\n"
            "if (m.dayOfWeek(Date.UTC(2026, 0, 5, 23, 30)) !== 1) throw new Error('late got ' + m.dayOfWeek(Date.UTC(2026, 0, 5, 23, 30)));\n"
            "if (m.dayOfWeek(Date.UTC(2026, 0, 5, 12, 0)) !== 1) throw new Error('noon got ' + m.dayOfWeek(Date.UTC(2026, 0, 5, 12, 0)));\n"
            "if (m.dayOfWeek(Date.UTC(2026, 0, 6, 0, 0)) !== 2) throw new Error('tuesday got ' + m.dayOfWeek(Date.UTC(2026, 0, 6, 0, 0)));\n"),
        "_fixed": {"utils.js":
            "function dayOfWeek(ms) { return new Date(ms).getUTCDay(); }\n"
            "module.exports = { dayOfWeek };\n"},
    })
    tasks.append({
        "id": "g4t-proto-chain",
        "category": "gen4-trap",
        "prompt": "utils.js exports getConfig(key) which must return the value for key from the "
                  "config object, or null for missing keys — INCLUDING keys inherited from "
                  "Object.prototype (getConfig('toString') must return null, not a function). It "
                  "currently reads config[key] directly, leaking prototype members. Use "
                  "Object.prototype.hasOwnProperty.",
        "setup": lambda d: write(d, {"utils.js":
            "const config = { host: 'localhost', port: 8080 };\n"
            "function getConfig(key) { return config[key] !== undefined ? config[key] : null; }  // BUG: proto leak\n"
            "module.exports = { getConfig };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.getConfig('host') !== 'localhost') throw new Error('host');\n"
            "if (m.getConfig('toString') !== null) throw new Error('proto leak: ' + m.getConfig('toString'));\n"
            "if (m.getConfig('constructor') !== null) throw new Error('constructor leak');\n"
            "if (m.getConfig('nope') !== null) throw new Error('missing');\n"
            "if (m.getConfig('__proto__') !== null) throw new Error('proto2 leak');\n"
            "if (m.getConfig('hasOwnProperty') !== null) throw new Error('hop leak: ' + m.getConfig('hasOwnProperty'));\n"
            "if (m.getConfig('port') !== 8080) throw new Error('port');\n"),
        "_fixed": {"utils.js":
            "const config = { host: 'localhost', port: 8080 };\n"
            "function getConfig(key) { return Object.prototype.hasOwnProperty.call(config, key) ? config[key] : null; }\n"
            "module.exports = { getConfig };\n"},
    })
    tasks.append({
        "id": "g4t-cumulative-fix",
        "category": "gen4-trap",
        "prompt": "utils.js exports balance(ops) which computes a running balance from "
                  "[{type:'deposit'|'withdraw', amount}] ops. Two bugs: (A) deposits are SUBTRACTED, "
                  "withdrawals ADDED (inverted); (B) the returned value is the absolute value. If "
                  "you fix just the inversion, the abs() in (B) still hides the sign. Fix BOTH so "
                  "balance([{type:'deposit',amount:100},{type:'withdraw',amount:40}]) === 60.",
        "setup": lambda d: write(d, {"utils.js":
            "function balance(ops) {\n"
            "  let b = 0;\n"
            "  for (const op of ops) b += op.type === 'deposit' ? -op.amount : op.amount;  // BUG A: inverted\n"
            "  return Math.abs(b);  // BUG B: hides sign\n"
            "}\n"
            "module.exports = { balance };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.balance([{ type: 'deposit', amount: 100 }, { type: 'withdraw', amount: 40 }]) !== 60) throw new Error('got ' + m.balance([{ type: 'deposit', amount: 100 }, { type: 'withdraw', amount: 40 }]));\n"
            "if (m.balance([{ type: 'withdraw', amount: 10 }]) !== -10) throw new Error('negative got ' + m.balance([{ type: 'withdraw', amount: 10 }]));\n"
            "if (m.balance([{ type: 'deposit', amount: 100 }, { type: 'withdraw', amount: 40 }, { type: 'deposit', amount: 10 }]) !== 70) throw new Error('three got ' + m.balance([{ type: 'deposit', amount: 100 }, { type: 'withdraw', amount: 40 }, { type: 'deposit', amount: 10 }]));\n"
            "if (m.balance([]) !== 0) throw new Error('empty got ' + m.balance([]));\n"
            "if (m.balance([{ type: 'deposit', amount: 0.5 }, { type: 'deposit', amount: 0.25 }]) !== 0.75) throw new Error('fraction got ' + m.balance([{ type: 'deposit', amount: 0.5 }, { type: 'deposit', amount: 0.25 }]));\n"),
        "_fixed": {"utils.js":
            "function balance(ops) {\n"
            "  let b = 0;\n"
            "  for (const op of ops) b += op.type === 'deposit' ? op.amount : -op.amount;\n"
            "  return b;\n"
            "}\n"
            "module.exports = { balance };\n"},
    })
    tasks.append({
        "id": "g4t-stateful-crash",
        "category": "gen4-trap",
        "prompt": "utils.js exports next(n) which must return n+1. It has a HIDDEN side effect: it "
                  "deletes a property from the module state and then reads it, so every call "
                  "returns NaN. Remove the side effect so next() works repeatedly.",
        "setup": lambda d: write(d, {"utils.js":
            "const state = { val: 1 };\n"
            "function next(n) { delete state.val; return state.val + n + 1; }  // BUG: reads deleted state\n"
            "module.exports = { next };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.next(1) !== 2) throw new Error('first got ' + m.next(1));\n"
            "if (m.next(2) !== 3) throw new Error('second got ' + m.next(2));\n"
            "if (m.next(3) !== 4) throw new Error('third got ' + m.next(3));\n"
            "if (m.next(0) !== 1) throw new Error('zero got ' + m.next(0));\n"
            "if (m.next(-5) !== -4) throw new Error('negative got ' + m.next(-5));\n"),
        "_fixed": {"utils.js":
            "function next(n) { return n + 1; }\n"
            "module.exports = { next };\n"},
    })
    tasks.append({
        "id": "g4t-regex-escape",
        "category": "gen4-trap",
        "prompt": "utils.js exports countOccurrences(text, word) which must count whole-word "
                  "occurrences using a REGEX built from `word`. The current code builds the regex "
                  "WITHOUT escaping — countOccurrences('a.b', '.') should be 1 (the literal dot) "
                  "but the regex treats '.' as any-char. Escape special characters in word.",
        "setup": lambda d: write(d, {"utils.js":
            "function countOccurrences(text, word) { return (text.match(new RegExp(word, 'g')) || []).length; }  // BUG: unescaped\n"
            "module.exports = { countOccurrences };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.countOccurrences('a.b', '.') !== 1) throw new Error('dot got ' + m.countOccurrences('a.b', '.'));\n"
            "if (m.countOccurrences('cat cat', 'cat') !== 2) throw new Error('word got ' + m.countOccurrences('cat cat', 'cat'));\n"
            "if (m.countOccurrences('a+b', '+') !== 1) throw new Error('plus got ' + m.countOccurrences('a+b', '+'));\n"
            "if (m.countOccurrences('a.b.c', '.') !== 2) throw new Error('two dots got ' + m.countOccurrences('a.b.c', '.'));\n"
            "if (m.countOccurrences('x[y]z', '[') !== 1) throw new Error('bracket got ' + m.countOccurrences('x[y]z', '['));\n"
            "if (m.countOccurrences('abc', 'd') !== 0) throw new Error('absent got ' + m.countOccurrences('abc', 'd'));\n"
            "if (m.countOccurrences('a\\\\.b', 'a\\\\.b') !== 1) throw new Error('escaped got ' + m.countOccurrences('a\\\\.b', 'a\\\\.b'));\n"),
        "_fixed": {"utils.js":
            "function countOccurrences(text, word) { const esc = word.replace(/[.*+?^${}()|[\\]\\\\]/g, '\\\\$&'); return (text.match(new RegExp(esc, 'g')) || []).length; }\n"
            "module.exports = { countOccurrences };\n"},
    })
    tasks.append({
        "id": "g4t-soft-fail",
        "category": "gen4-trap",
        "prompt": "utils.js exports maxNum(nums) which must return the maximum of a NON-EMPTY "
                  "array. For an EMPTY array it must return undefined (not crash, not 0). The "
                  "current code returns 0 for empty arrays because it initializes max = 0 — a "
                  "silent wrong answer, not a crash. Fix it.",
        "setup": lambda d: write(d, {"utils.js":
            "function maxNum(nums) { let max = 0; for (const n of nums) if (n > max) max = n; return max; }  // BUG: empty -> 0\n"
            "module.exports = { maxNum };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.maxNum([3, 9, 4]) !== 9) throw new Error('got ' + m.maxNum([3, 9, 4]));\n"
            "if (m.maxNum([-5, -2]) !== -2) throw new Error('negatives got ' + m.maxNum([-5, -2]));\n"
            "if (m.maxNum([]) !== undefined) throw new Error('empty should be undefined, got ' + m.maxNum([]));\n"
            "if (m.maxNum([-1]) !== -1) throw new Error('single-neg got ' + m.maxNum([-1]));\n"
            "if (m.maxNum([0, -3, 5]) !== 5) throw new Error('mixed got ' + m.maxNum([0, -3, 5]));\n"
            "if (m.maxNum([42]) !== 42) throw new Error('single got ' + m.maxNum([42]));\n"),
        "_fixed": {"utils.js":
            "function maxNum(nums) { if (!nums.length) return undefined; return Math.max(...nums); }\n"
            "module.exports = { maxNum };\n"},
    })
    tasks.append({
        "id": "g4t-mutate-caller",
        "category": "gen4-trap",
        "prompt": "utils.js exports addItem(items, item) which must return a NEW array with item "
                  "appended, leaving the caller's array unchanged. The current code mutates the "
                  "input (push). The caller relies on the original — fix it to return a copy.",
        "setup": lambda d: write(d, {"utils.js":
            "function addItem(items, item) { items.push(item); return items; }  // BUG: mutates caller's array\n"
            "module.exports = { addItem };\n"}),
        "test": lambda d: rt(d, "utils",
            "const orig = [1, 2];\n"
            "const r = m.addItem(orig, 3);\n"
            "if (r.join(',') !== '1,2,3') throw new Error('result ' + r.join(','));\n"
            "if (orig.join(',') !== '1,2') throw new Error('CALLER MUTATED: ' + orig.join(','));\n"
            "const orig2 = [1];\n"
            "const r2 = m.addItem(orig2, 2);\n"
            "if (r2.join(',') !== '1,2') throw new Error('r2 ' + r2.join(','));\n"
            "if (orig2.join(',') !== '1') throw new Error('orig2 MUTATED: ' + orig2.join(','));\n"
            "if (m.addItem([], 'x').join(',') !== 'x') throw new Error('empty in');\n"),
        "_fixed": {"utils.js":
            "function addItem(items, item) { return [...items, item]; }\n"
            "module.exports = { addItem };\n"},
    })
    tasks.append({
        "id": "g4t-double-apply",
        "category": "gen4-trap",
        "prompt": "utils.js exports applyDiscount(price, pct) which returns price minus pct%. "
                  "It currently applies the discount TWICE (price * (1-pct) * (1-pct)), because a "
                  "helper was called twice. Fix it to apply once (applyDiscount(100, 10) === 90).",
        "setup": lambda d: write(d, {"utils.js":
            "function discount(price, pct) { return price * (1 - pct / 100); }\n"
            "function applyDiscount(price, pct) { return discount(discount(price, pct), pct); }  // BUG: applied twice\n"
            "module.exports = { applyDiscount };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.applyDiscount(100, 10) !== 90) throw new Error('got ' + m.applyDiscount(100, 10));\n"
            "if (m.applyDiscount(200, 50) !== 100) throw new Error('half got ' + m.applyDiscount(200, 50));\n"
            "if (m.applyDiscount(50, 0) !== 50) throw new Error('zero got ' + m.applyDiscount(50, 0));\n"
            "if (m.applyDiscount(100, 100) !== 0) throw new Error('full got ' + m.applyDiscount(100, 100));\n"
            "if (m.applyDiscount(99.99, 10) !== 89.991) throw new Error('float got ' + m.applyDiscount(99.99, 10));\n"),
        "_fixed": {"utils.js":
            "function discount(price, pct) { return price * (1 - pct / 100); }\n"
            "function applyDiscount(price, pct) { return discount(price, pct); }\n"
            "module.exports = { applyDiscount };\n"},
    })
    tasks.append({
        "id": "g4t-undefined-vs-null",
        "category": "gen4-trap",
        "prompt": "utils.js exports firstNonEmpty(items) which must return the first "
                  "non-empty-string item, or undefined when none exist. It currently returns null "
                  "when nothing found (firstNonEmpty(['', '']) === null). Return undefined, not "
                  "null, for 'nothing found'.",
        "setup": lambda d: write(d, {"utils.js":
            "function firstNonEmpty(items) { return items.find(x => x && x.trim() !== '') || null; }  // BUG: null instead of undefined\n"
            "module.exports = { firstNonEmpty };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.firstNonEmpty(['', 'hi', '']) !== 'hi') throw new Error('found got ' + m.firstNonEmpty(['', 'hi', '']));\n"
            "if (m.firstNonEmpty(['', '']) !== undefined) throw new Error('none should be undefined, got ' + m.firstNonEmpty(['', '']));\n"
            "if (m.firstNonEmpty([]) !== undefined) throw new Error('empty list');\n"
            "if (m.firstNonEmpty(['  ', 'x']) !== 'x') throw new Error('whitespace got ' + m.firstNonEmpty(['  ', 'x']));\n"
            "if (m.firstNonEmpty(['', 0, 'y']) !== 'y') throw new Error('falsy-elem skip got ' + m.firstNonEmpty(['', 0, 'y']));\n"),
        "_fixed": {"utils.js":
            "function firstNonEmpty(items) { const f = items.find(x => x && x.trim() !== ''); return f === undefined ? undefined : f; }\n"
            "module.exports = { firstNonEmpty };\n"},
    })
    tasks.append({
        "id": "g4t-index-shift",
        "category": "gen4-trap",
        "prompt": "utils.js exports rankOf(items, value) which must return the 1-BASED index of "
                  "value in items, or -1 if absent (rankOf(['a','b','c'], 'b') === 2). It "
                  "currently returns the 0-based index. Fix it.",
        "setup": lambda d: write(d, {"utils.js":
            "function rankOf(items, value) { return items.indexOf(value); }  // BUG: 0-based\n"
            "module.exports = { rankOf };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.rankOf(['a', 'b', 'c'], 'b') !== 2) throw new Error('got ' + m.rankOf(['a', 'b', 'c'], 'b'));\n"
            "if (m.rankOf(['a'], 'a') !== 1) throw new Error('first got ' + m.rankOf(['a'], 'a'));\n"
            "if (m.rankOf(['a'], 'z') !== -1) throw new Error('absent got ' + m.rankOf(['a'], 'z'));\n"
            "if (m.rankOf(['a', 'b', 'c'], 'c') !== 3) throw new Error('last got ' + m.rankOf(['a', 'b', 'c'], 'c'));\n"
            "if (m.rankOf([], 'x') !== -1) throw new Error('empty got ' + m.rankOf([], 'x'));\n"
            "if (m.rankOf(['a', 'a', 'b'], 'a') !== 1) throw new Error('dup got ' + m.rankOf(['a', 'a', 'b'], 'a'));\n"),
        "_fixed": {"utils.js":
            "function rankOf(items, value) { const i = items.indexOf(value); return i === -1 ? -1 : i + 1; }\n"
            "module.exports = { rankOf };\n"},
    })
    tasks.append({
        "id": "g4t-var-hoist",
        "category": "gen4-trap",
        "prompt": "utils.js exports createLogger(prefix) which must return a function that logs "
                  "prefix + msg. It currently uses var in a way that makes ALL loggers share the "
                  "last prefix (var hoisting / closure over loop). Fix the closure capture.",
        "setup": lambda d: write(d, {"utils.js":
            "function createLogger(prefix) { var log = function (msg) { return prefix + ': ' + msg; }; return log; }  // fine?\n"
            "function createLoggers(prefixes) { var fns = []; for (var i = 0; i < prefixes.length; i++) { var p = prefixes[i]; fns.push(function (msg) { return p + ': ' + msg; }); } return fns; }  // BUG: all share last p\n"
            "module.exports = { createLogger, createLoggers };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "const fns = m.createLoggers(['A', 'B', 'C']);\n"
            "if (fns[0]('x') !== 'A: x') throw new Error('f0 got ' + fns[0]('x'));\n"
            "if (fns[1]('x') !== 'B: x') throw new Error('f1 got ' + fns[1]('x'));\n"
            "if (fns[2]('x') !== 'C: x') throw new Error('f2 got ' + fns[2]('x'));\n"
            "if (m.createLogger('D')('y') !== 'D: y') throw new Error('single');\n"
            "const f2 = m.createLoggers(['X']);\n"
            "if (f2[0]('y') !== 'X: y') throw new Error('one-list got ' + f2[0]('y'));\n"
            "const f3 = m.createLoggers(['P', 'Q', 'R', 'S']);\n"
            "if (f3[3]('m') !== 'S: m') throw new Error('f4 got ' + f3[3]('m'));\n"),
        "_fixed": {"utils.js":
            "function createLogger(prefix) { var log = function (msg) { return prefix + ': ' + msg; }; return log; }\n"
            "function createLoggers(prefixes) { var fns = []; for (var i = 0; i < prefixes.length; i++) { (function (p) { fns.push(function (msg) { return p + ': ' + msg; }); })(prefixes[i]); } return fns; }\n"
            "module.exports = { createLogger, createLoggers };\n"},
    })
    tasks.append({
        "id": "g4t-float-meta",
        "category": "gen4-trap",
        "prompt": "utils.js exports mean(nums) which must return the mean rounded to 2 decimals "
                  "using the epsilon trick (mean([0.1, 0.2]) === 0.15). The current code divides "
                  "but never rounds, returning 0.15000000000000002. Fix it to round to 2 decimals "
                  "with Number.EPSILON.",
        "setup": lambda d: write(d, {"utils.js":
            "function mean(nums) { return nums.reduce((a, b) => a + b, 0) / nums.length; }  // BUG: no rounding\n"
            "module.exports = { mean };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.mean([0.1, 0.2]) !== 0.15) throw new Error('got ' + m.mean([0.1, 0.2]));\n"
            "if (m.mean([1, 2, 3]) !== 2) throw new Error('plain got ' + m.mean([1, 2, 3]));\n"
            "if (m.mean([0.1, 0.2, 0.3]) !== 0.2) throw new Error('noise got ' + m.mean([0.1, 0.2, 0.3]));\n"
            "if (m.mean([2.505]) !== 2.51) throw new Error('eps got ' + m.mean([2.505]));\n"
            "if (m.mean([-1, 1]) !== 0) throw new Error('zero got ' + m.mean([-1, 1]));\n"),
        "_fixed": {"utils.js":
            "function mean(nums) { const m = nums.reduce((a, b) => a + b, 0) / nums.length; return Math.round((m + Number.EPSILON) * 100) / 100; }\n"
            "module.exports = { mean };\n"},
    })

    # ══════════════════════════════════════════════════════════════════════
    # gen4-chain — 5 mini-repos x 3 sequential tasks, harder invariants
    # ══════════════════════════════════════════════════════════════════════
    def build_chain(cid, base_files, steps):
        out = []
        fixed_so_far = {}
        for i, st in enumerate(steps):
            fixed_so_far.update(st["fixed"])
            buggy_files = dict(base_files)
            for rel, content in fixed_so_far.items():
                buggy_files[rel] = content
            buggy_files.update(st["bug"])
            tid = st["id"]
            out.append({
                "id": tid,
                "category": "gen4-chain",
                "prompt": st["prompt"],
                "setup": lambda d, _b=buggy_files: (write(d, _b), ensure_deps(d)),
                "test": lambda d, _t=st["test"]: _t(d),
                "_fixed": dict(base_files, **fixed_so_far),
            })
        return out

    # ── chain 1: cart with discount rules ──
    ch1_base = {
        "cart.js":
            "let items = [];\n"
            "exports.add = function (name, price) { items.push({ name, price }); };\n"
            "exports.list = function () { return items; };\n"
            "exports.total = function () { return items.reduce((a, i) => a + i.price, 0); };\n",
        "server.js": _expr(
            "const cart = require('./cart');\n"
            "app.post('/api/cart', (req, res) => { cart.add(req.body.name, Number(req.body.price)); res.status(201).json({ ok: true }); });\n"
            "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch1", ch1_base, [
        {
            "id": "g4c1-1",
            "prompt": "The cart must reject invalid additions: name missing/empty or price NaN or "
                      "price <= 0 must return 400. The current POST accepts anything. Add "
                      "validation in cart.js (add returns {ok:false,error} for bad input) and have "
                      "the route return 400. Valid adds must keep working (201).",
            "bug": {"cart.js":
                "let items = [];\n"
                "exports.add = function (name, price) { items.push({ name, price }); return { ok: true }; };  // BUG: no validation\n"
                "exports.list = function () { return items; };\n"
                "exports.total = function () { return items.reduce((a, i) => a + i.price, 0); };\n",
                "server.js": _expr(
                "const cart = require('./cart');\n"
                "app.post('/api/cart', (req, res) => { cart.add(req.body.name, Number(req.body.price)); res.status(201).json({ ok: true }); });\n"
                "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n")},
            "fixed": {"cart.js":
                "let items = [];\n"
                "exports.add = function (name, price) {\n"
                "  if (typeof name !== 'string' || name.trim() === '') return { ok: false, error: 'name required' };\n"
                "  if (typeof price !== 'number' || !isFinite(price) || price <= 0) return { ok: false, error: 'invalid price' };\n"
                "  items.push({ name, price });\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.list = function () { return items; };\n"
                "exports.total = function () { return items.reduce((a, i) => a + i.price, 0); };\n",
                "server.js": _expr(
                "const cart = require('./cart');\n"
                "app.post('/api/cart', (req, res) => { const r = cart.add(req.body.name, Number(req.body.price)); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ ok: true }); });\n"
                "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n")},
            "test": lambda d: api(d, (
                "    let r = await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: '', price: 5 }) });\n"
                "    if (r.status !== 400) throw new Error('empty name should 400, got ' + r.status);\n"
                "    r = await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'x', price: 0 }) });\n"
                "    if (r.status !== 400) throw new Error('zero price should 400, got ' + r.status);\n"
                "    r = await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'apple', price: 3.5 }) });\n"
                "    if (r.status !== 201) throw new Error('valid should 201, got ' + r.status);\n"
                "    let j = await (await fetch(base + '/api/cart')).json();\n"
                "    if (j.items.length !== 1 || j.total !== 3.5) throw new Error('cart ' + JSON.stringify(j));\n"
            )),
        },
        {
            "id": "g4c1-2",
            "prompt": "Add a discount: when the cart subtotal is above 100, apply 10% off to the "
                      "total shown by GET /api/cart (total must reflect the discount; items stay "
                      "unchanged). The current total() has no discount logic. Implement it in "
                      "cart.js. Validation from before must keep working.",
            "bug": {"cart.js":
                "let items = [];\n"
                "exports.add = function (name, price) {\n"
                "  if (typeof name !== 'string' || name.trim() === '') return { ok: false, error: 'name required' };\n"
                "  if (typeof price !== 'number' || !isFinite(price) || price <= 0) return { ok: false, error: 'invalid price' };\n"
                "  items.push({ name, price });\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.list = function () { return items; };\n"
                "exports.total = function () { return items.reduce((a, i) => a + i.price, 0); };  // BUG: no discount\n",
                "server.js": _expr(
                "const cart = require('./cart');\n"
                "app.post('/api/cart', (req, res) => { const r = cart.add(req.body.name, Number(req.body.price)); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ ok: true }); });\n"
                "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n")},
            "fixed": {"cart.js":
                "let items = [];\n"
                "exports.add = function (name, price) {\n"
                "  if (typeof name !== 'string' || name.trim() === '') return { ok: false, error: 'name required' };\n"
                "  if (typeof price !== 'number' || !isFinite(price) || price <= 0) return { ok: false, error: 'invalid price' };\n"
                "  items.push({ name, price });\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.list = function () { return items; };\n"
                "exports.total = function () {\n"
                "  const sub = items.reduce((a, i) => a + i.price, 0);\n"
                "  return sub > 100 ? Math.round(sub * 0.9 * 100) / 100 : sub;\n"
                "};\n",
                "server.js": _expr(
                "const cart = require('./cart');\n"
                "app.post('/api/cart', (req, res) => { const r = cart.add(req.body.name, Number(req.body.price)); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ ok: true }); });\n"
                "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n")},
            "test": lambda d: api(d, (
                "    for (let i = 0; i < 3; i++) await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'item' + i, price: 40 }) });\n"
                "    let j = await (await fetch(base + '/api/cart')).json();\n"
                "    if (j.total !== 108) throw new Error('discounted got ' + JSON.stringify(j));\n"
                "    if (j.items.length !== 3) throw new Error('items must be unchanged');\n"
                "    await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'bad', price: 0 }) });\n"
                "    j = await (await fetch(base + '/api/cart')).json();\n"
                "    if (j.items.length !== 3) throw new Error('bad add leaked in');\n"
            )),
        },
        {
            "id": "g4c1-3",
            "prompt": "Add a 5% loyalty bonus: every cart total (after discount) is rounded DOWN "
                      "to the nearest integer as a bonus deduction — total() must return "
                      "Math.floor(discountedTotal). The current total() returns the discounted "
                      "float (e.g. 108.0 stays 108, but 105.3 must become 105). Implement the "
                      "floor. Discount + validation must keep working.",
            "bug": {"cart.js":
                "let items = [];\n"
                "exports.add = function (name, price) {\n"
                "  if (typeof name !== 'string' || name.trim() === '') return { ok: false, error: 'name required' };\n"
                "  if (typeof price !== 'number' || !isFinite(price) || price <= 0) return { ok: false, error: 'invalid price' };\n"
                "  items.push({ name, price });\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.list = function () { return items; };\n"
                "exports.total = function () {\n"
                "  const sub = items.reduce((a, i) => a + i.price, 0);\n"
                "  return sub > 100 ? Math.round(sub * 0.9 * 100) / 100 : sub;  // BUG: no floor\n"
                "};\n",
                "server.js": _expr(
                "const cart = require('./cart');\n"
                "app.post('/api/cart', (req, res) => { const r = cart.add(req.body.name, Number(req.body.price)); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ ok: true }); });\n"
                "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n")},
            "fixed": {"cart.js":
                "let items = [];\n"
                "exports.add = function (name, price) {\n"
                "  if (typeof name !== 'string' || name.trim() === '') return { ok: false, error: 'name required' };\n"
                "  if (typeof price !== 'number' || !isFinite(price) || price <= 0) return { ok: false, error: 'invalid price' };\n"
                "  items.push({ name, price });\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.list = function () { return items; };\n"
                "exports.total = function () {\n"
                "  const sub = items.reduce((a, i) => a + i.price, 0);\n"
                "  const discounted = sub > 100 ? sub * 0.9 : sub;\n"
                "  return Math.floor(discounted);\n"
                "};\n",
                "server.js": _expr(
                "const cart = require('./cart');\n"
                "app.post('/api/cart', (req, res) => { const r = cart.add(req.body.name, Number(req.body.price)); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ ok: true }); });\n"
                "app.get('/api/cart', (req, res) => res.json({ items: cart.list(), total: cart.total() }));\n")},
            "test": lambda d: api(d, (
                "    // subtotal 106 -> discounted 95.4 -> floor 95\n"
                "    await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'a', price: 53 }) });\n"
                "    await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'b', price: 53 }) });\n"
                "    let j = await (await fetch(base + '/api/cart')).json();\n"
                "    if (j.total !== 95) throw new Error('floored got ' + JSON.stringify(j));\n"
                "    // subtotal 165.9 -> discounted 149.31 -> floor 149\n"
                "    await fetch(base + '/api/cart', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'c', price: 59.9 }) });\n"
                "    j = await (await fetch(base + '/api/cart')).json();\n"
                "    if (j.total !== 149) throw new Error('after 3rd got ' + JSON.stringify(j));\n"
            )),
        },
    ])

    # ── chain 2: shared ledger (deposit/withdraw with invariants) ──
    ch2_base = {
        "ledger.js":
            "let balance = 0;\n"
            "let txnCount = 0;\n"
            "exports.deposit = function (amt) { balance += amt; txnCount += 1; return { ok: true }; };\n"
            "exports.withdraw = function (amt) { balance -= amt; txnCount += 1; return { ok: true }; };\n"
            "exports.get = function () { return { balance, txnCount }; };\n",
        "server.js": _expr(
            "const ledger = require('./ledger');\n"
            "app.post('/api/deposit', (req, res) => { ledger.deposit(Number(req.body.amount)); res.json({ ok: true }); });\n"
            "app.post('/api/withdraw', (req, res) => { ledger.withdraw(Number(req.body.amount)); res.json({ ok: true }); });\n"
            "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch2", ch2_base, [
        {
            "id": "g4c2-1",
            "prompt": "Add validation: deposits with amount <= 0 or NaN must return 400 (do NOT "
                      "record the txn). The current deposit records anything. Fix ledger.js "
                      "deposit() to validate (return {ok:false,error}) and the route to 400.",
            "bug": {"ledger.js":
                "let balance = 0;\n"
                "let txnCount = 0;\n"
                "exports.deposit = function (amt) { balance += amt; txnCount += 1; return { ok: true }; };  // BUG: no validation\n"
                "exports.withdraw = function (amt) { balance -= amt; txnCount += 1; return { ok: true }; };\n"
                "exports.get = function () { return { balance, txnCount }; };\n",
                "server.js": _expr(
                "const ledger = require('./ledger');\n"
                "app.post('/api/deposit', (req, res) => { ledger.deposit(Number(req.body.amount)); res.json({ ok: true }); });\n"
                "app.post('/api/withdraw', (req, res) => { ledger.withdraw(Number(req.body.amount)); res.json({ ok: true }); });\n"
                "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n")},
            "fixed": {"ledger.js":
                "let balance = 0;\n"
                "let txnCount = 0;\n"
                "exports.deposit = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  balance += amt; txnCount += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.withdraw = function (amt) { balance -= amt; txnCount += 1; return { ok: true }; };\n"
                "exports.get = function () { return { balance, txnCount }; };\n",
                "server.js": _expr(
                "const ledger = require('./ledger');\n"
                "app.post('/api/deposit', (req, res) => { const r = ledger.deposit(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.post('/api/withdraw', (req, res) => { ledger.withdraw(Number(req.body.amount)); res.json({ ok: true }); });\n"
                "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n")},
            "test": lambda d: api(d, (
                "    let r = await fetch(base + '/api/deposit', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: -5 }) });\n"
                "    if (r.status !== 400) throw new Error('negative should 400, got ' + r.status);\n"
                "    r = await fetch(base + '/api/deposit', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 0 }) });\n"
                "    if (r.status !== 400) throw new Error('zero should 400, got ' + r.status);\n"
                "    r = await fetch(base + '/api/deposit', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 100 }) });\n"
                "    if (r.status !== 200) throw new Error('valid should 200, got ' + r.status);\n"
                "    let j = await (await fetch(base + '/api/ledger')).json();\n"
                "    if (j.balance !== 100 || j.txnCount !== 1) throw new Error('ledger ' + JSON.stringify(j));\n"
            )),
        },
        {
            "id": "g4c2-2",
            "prompt": "Add an overdraft invariant: withdrawals must NOT make balance negative — "
                      "withdrawing more than the balance returns 400 and records NO transaction. "
                      "The current withdraw allows negative balance. Fix withdraw() to check "
                      "sufficient funds. Deposit validation must keep working.",
            "bug": {"ledger.js":
                "let balance = 0;\n"
                "let txnCount = 0;\n"
                "exports.deposit = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  balance += amt; txnCount += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.withdraw = function (amt) { balance -= amt; txnCount += 1; return { ok: true }; };  // BUG: no overdraft check\n"
                "exports.get = function () { return { balance, txnCount }; };\n",
                "server.js": _expr(
                "const ledger = require('./ledger');\n"
                "app.post('/api/deposit', (req, res) => { const r = ledger.deposit(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.post('/api/withdraw', (req, res) => { ledger.withdraw(Number(req.body.amount)); res.json({ ok: true }); });\n"
                "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n")},
            "fixed": {"ledger.js":
                "let balance = 0;\n"
                "let txnCount = 0;\n"
                "exports.deposit = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  balance += amt; txnCount += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.withdraw = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  if (amt > balance) return { ok: false, error: 'insufficient funds' };\n"
                "  balance -= amt; txnCount += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.get = function () { return { balance, txnCount }; };\n",
                "server.js": _expr(
                "const ledger = require('./ledger');\n"
                "app.post('/api/deposit', (req, res) => { const r = ledger.deposit(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.post('/api/withdraw', (req, res) => { const r = ledger.withdraw(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n")},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/deposit', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 50 }) });\n"
                "    let r = await fetch(base + '/api/withdraw', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 100 }) });\n"
                "    if (r.status !== 400) throw new Error('overdraft should 400, got ' + r.status);\n"
                "    let j = await (await fetch(base + '/api/ledger')).json();\n"
                "    if (j.balance !== 50 || j.txnCount !== 1) throw new Error('overdraft recorded: ' + JSON.stringify(j));\n"
                "    r = await fetch(base + '/api/withdraw', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 30 }) });\n"
                "    if (r.status !== 200) throw new Error('valid withdraw got ' + r.status);\n"
                "    j = await (await fetch(base + '/api/ledger')).json();\n"
                "    if (j.balance !== 20 || j.txnCount !== 2) throw new Error('after withdraw ' + JSON.stringify(j));\n"
            )),
        },
        {
            "id": "g4c2-3",
            "prompt": "Add a daily limit: the ledger may record at most 5 transactions per day — "
                      "the 6th deposit/withdraw must return 400 with 'daily limit reached'. The "
                      "current code has no limit. Implement it in ledger.js. Overdraft + deposit "
                      "validation must keep working.",
            "bug": {"ledger.js":
                "let balance = 0;\n"
                "let txnCount = 0;\n"
                "exports.deposit = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  balance += amt; txnCount += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.withdraw = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  if (amt > balance) return { ok: false, error: 'insufficient funds' };\n"
                "  balance -= amt; txnCount += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.get = function () { return { balance, txnCount }; };  // BUG: no daily limit\n",
                "server.js": _expr(
                "const ledger = require('./ledger');\n"
                "app.post('/api/deposit', (req, res) => { const r = ledger.deposit(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.post('/api/withdraw', (req, res) => { const r = ledger.withdraw(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n")},
            "fixed": {"ledger.js":
                "let balance = 0;\n"
                "let txnCount = 0;\n"
                "const DAILY_LIMIT = 5;\n"
                "function guarded(fn) {\n"
                "  if (txnCount >= DAILY_LIMIT) return { ok: false, error: 'daily limit reached' };\n"
                "  return fn();\n"
                "}\n"
                "exports.deposit = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  return guarded(() => { balance += amt; txnCount += 1; return { ok: true }; });\n"
                "};\n"
                "exports.withdraw = function (amt) {\n"
                "  if (typeof amt !== 'number' || !isFinite(amt) || amt <= 0) return { ok: false, error: 'invalid amount' };\n"
                "  if (amt > balance) return { ok: false, error: 'insufficient funds' };\n"
                "  return guarded(() => { balance -= amt; txnCount += 1; return { ok: true }; });\n"
                "};\n"
                "exports.get = function () { return { balance, txnCount }; };\n",
                "server.js": _expr(
                "const ledger = require('./ledger');\n"
                "app.post('/api/deposit', (req, res) => { const r = ledger.deposit(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.post('/api/withdraw', (req, res) => { const r = ledger.withdraw(Number(req.body.amount)); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.get('/api/ledger', (req, res) => res.json(ledger.get()));\n")},
            "test": lambda d: api(d, (
                "    for (let i = 0; i < 5; i++) {\n"
                "      const r = await fetch(base + '/api/deposit', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 10 }) });\n"
                "      if (r.status !== 200) throw new Error('deposit ' + i + ' -> ' + r.status);\n"
                "    }\n"
                "    let r = await fetch(base + '/api/deposit', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ amount: 10 }) });\n"
                "    if (r.status !== 400) throw new Error('6th should 400, got ' + r.status);\n"
                "    let j = await (await fetch(base + '/api/ledger')).json();\n"
                "    if (j.balance !== 50 || j.txnCount !== 5) throw new Error('ledger ' + JSON.stringify(j));\n"
            )),
        },
    ])

    # ── chain 3: sessions with expiry ──
    ch3_base = {
        "sessions.js":
            "const sessions = new Map();\n"
            "exports.create = function (user) { const token = 't' + (sessions.size + 1); sessions.set(token, { user, at: Date.now() }); return token; };\n"
            "exports.get = function (token) { return sessions.has(token) ? sessions.get(token) : null; };\n"
            "exports.remove = function (token) { sessions.delete(token); };\n",
        "server.js": _expr(
            "const sessions = require('./sessions');\n"
            "app.post('/api/login', (req, res) => { const token = sessions.create(req.body.user); res.json({ token }); });\n"
            "app.get('/api/me', (req, res) => {\n"
            "  const s = sessions.get(req.headers['x-token']);\n"
            "  if (!s) return res.status(401).json({ error: 'no session' });\n"
            "  res.json({ user: s.user });\n"
            "});\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch3", ch3_base, [
        {
            "id": "g4c3-1",
            "prompt": "Add session expiry: sessions older than 30 seconds must be treated as "
                      "invalid (get() returns null). The current get() never expires. Store the "
                      "creation time (already stored as `at`) and check it in get(). Login and "
                      "/api/me must keep working.",
            "bug": {"sessions.js":
                "const sessions = new Map();\n"
                "exports.create = function (user) { const token = 't' + (sessions.size + 1); sessions.set(token, { user, at: Date.now() }); return token; };\n"
                "exports.get = function (token) { return sessions.has(token) ? sessions.get(token) : null; };  // BUG: no expiry\n"
                "exports.remove = function (token) { sessions.delete(token); };\n"},
            "fixed": {"sessions.js":
                "const sessions = new Map();\n"
                "const TTL = 30000;\n"
                "exports.create = function (user) { const token = 't' + (sessions.size + 1); sessions.set(token, { user, at: Date.now() }); return token; };\n"
                "exports.get = function (token) {\n"
                "  if (!sessions.has(token)) return null;\n"
                "  const s = sessions.get(token);\n"
                "  if (Date.now() - s.at > TTL) { sessions.delete(token); return null; }\n"
                "  return s;\n"
                "};\n"
                "exports.remove = function (token) { sessions.delete(token); };\n"},
            "test": lambda d: rat(d, "sessions",
                "    const realNow = Date.now;\n"
                "    const token = m.create('alice');\n"
                "    if (m.get(token) === null) throw new Error('fresh must exist');\n"
                "    Date.now = () => realNow() + 40000;  // pretend 40s passed (> 30s TTL)\n"
                "    if (m.get(token) !== null) throw new Error('expired must be null');\n"
                "    Date.now = realNow;\n"
                "    if (m.get(token) !== null) throw new Error('after restore must still be null (deleted)');\n"),
        },
        {
            "id": "g4c3-2",
            "prompt": "Add logout: POST /api/logout removes the session; after logout, /api/me with "
                      "the old token must return 401. The current app has no logout route. Add it. "
                      "Login + expiry must keep working.",
            "bug": {"server.js": _expr(
                "const sessions = require('./sessions');\n"
                "app.post('/api/login', (req, res) => { const token = sessions.create(req.body.user); res.json({ token }); });\n"
                "app.get('/api/me', (req, res) => {\n"
                "  const s = sessions.get(req.headers['x-token']);\n"
                "  if (!s) return res.status(401).json({ error: 'no session' });\n"
                "  res.json({ user: s.user });\n"
                "});\n"
                "// BUG: no logout route\n")},
            "fixed": {"server.js": _expr(
                "const sessions = require('./sessions');\n"
                "app.post('/api/login', (req, res) => { const token = sessions.create(req.body.user); res.json({ token }); });\n"
                "app.get('/api/me', (req, res) => {\n"
                "  const s = sessions.get(req.headers['x-token']);\n"
                "  if (!s) return res.status(401).json({ error: 'no session' });\n"
                "  res.json({ user: s.user });\n"
                "});\n"
                "app.post('/api/logout', (req, res) => { sessions.remove(req.headers['x-token']); res.json({ ok: true }); });\n")},
            "test": lambda d: api(d, (
                "    let j = await (await fetch(base + '/api/login', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ user: 'bob' }) })).json();\n"
                "    let r = await fetch(base + '/api/me', { headers: { 'x-token': j.token } });\n"
                "    if (r.status !== 200) throw new Error('pre-logout should 200, got ' + r.status);\n"
                "    r = await fetch(base + '/api/logout', { method: 'POST', headers: { 'x-token': j.token } });\n"
                "    if (r.status !== 200) throw new Error('logout should 200, got ' + r.status);\n"
                "    r = await fetch(base + '/api/me', { headers: { 'x-token': j.token } });\n"
                "    if (r.status !== 401) throw new Error('post-logout should 401, got ' + r.status);\n"
            )),
        },
        {
            "id": "g4c3-3",
            "prompt": "Make sessions single-use: calling /api/me with a token must CONSUME it — "
                      "the SECOND call with the same token must return 401 (like a one-time "
                      "auth). The current /api/me leaves the token usable forever (until expiry). "
                      "Remove the session after a successful /api/me. Logout must keep working.",
            "bug": {"server.js": _expr(
                "const sessions = require('./sessions');\n"
                "app.post('/api/login', (req, res) => { const token = sessions.create(req.body.user); res.json({ token }); });\n"
                "app.get('/api/me', (req, res) => {\n"
                "  const s = sessions.get(req.headers['x-token']);\n"
                "  if (!s) return res.status(401).json({ error: 'no session' });\n"
                "  res.json({ user: s.user });  // BUG: token stays valid\n"
                "});\n"
                "app.post('/api/logout', (req, res) => { sessions.remove(req.headers['x-token']); res.json({ ok: true }); });\n")},
            "fixed": {"server.js": _expr(
                "const sessions = require('./sessions');\n"
                "app.post('/api/login', (req, res) => { const token = sessions.create(req.body.user); res.json({ token }); });\n"
                "app.get('/api/me', (req, res) => {\n"
                "  const s = sessions.get(req.headers['x-token']);\n"
                "  if (!s) return res.status(401).json({ error: 'no session' });\n"
                "  sessions.remove(req.headers['x-token']);\n"
                "  res.json({ user: s.user });\n"
                "});\n"
                "app.post('/api/logout', (req, res) => { sessions.remove(req.headers['x-token']); res.json({ ok: true }); });\n")},
            "test": lambda d: api(d, (
                "    let j = await (await fetch(base + '/api/login', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ user: 'carol' }) })).json();\n"
                "    let r = await fetch(base + '/api/me', { headers: { 'x-token': j.token } });\n"
                "    if (r.status !== 200) throw new Error('first use should 200, got ' + r.status);\n"
                "    r = await fetch(base + '/api/me', { headers: { 'x-token': j.token } });\n"
                "    if (r.status !== 401) throw new Error('second use must 401, got ' + r.status);\n"
            )),
        },
    ])

    # ── chain 4: key-value store with TTL cache ──
    ch4_base = {
        "kv.js":
            "const store = new Map();\n"
            "exports.set = function (k, v) { store.set(k, { value: v, at: Date.now() }); };\n"
            "exports.get = function (k) { return store.has(k) ? store.get(k).value : null; };\n"
            "exports.del = function (k) { store.delete(k); };\n"
            "exports.keys = function () { return [...store.keys()]; };\n",
        "server.js": _expr(
            "const kv = require('./kv');\n"
            "app.put('/api/kv/:key', (req, res) => { kv.set(req.params.key, req.body.value); res.json({ ok: true }); });\n"
            "app.get('/api/kv/:key', (req, res) => res.json({ value: kv.get(req.params.key) }));\n"
            "app.delete('/api/kv/:key', (req, res) => { kv.del(req.params.key); res.json({ ok: true }); });\n"
            "app.get('/api/kv', (req, res) => res.json({ keys: kv.keys() }));\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch4", ch4_base, [
        {
            "id": "g4c4-1",
            "prompt": "kv.get(k, ttlMs) must return null when the stored entry is older than ttlMs. "
                      "The current get() ignores ttl. Add the age check (entry has `at`). The "
                      "routes must keep working.",
            "bug": {"kv.js":
                "const store = new Map();\n"
                "exports.set = function (k, v) { store.set(k, { value: v, at: Date.now() }); };\n"
                "exports.get = function (k, ttlMs) { return store.has(k) ? store.get(k).value : null; };  // BUG: ignores ttl\n"
                "exports.del = function (k) { store.delete(k); };\n"
                "exports.keys = function () { return [...store.keys()]; };\n"},
            "fixed": {"kv.js":
                "const store = new Map();\n"
                "exports.set = function (k, v) { store.set(k, { value: v, at: Date.now() }); };\n"
                "exports.get = function (k, ttlMs) {\n"
                "  if (!store.has(k)) return null;\n"
                "  const e = store.get(k);\n"
                "  if (ttlMs && Date.now() - e.at > ttlMs) { store.delete(k); return null; }\n"
                "  return e.value;\n"
                "};\n"
                "exports.del = function (k) { store.delete(k); };\n"
                "exports.keys = function () { return [...store.keys()]; };\n"},
            "test": lambda d: rat(d, "kv",
                "    m.set('a', 1);\n"
                "    if (m.get('a') !== 1) throw new Error('plain get');\n"
                "    if (m.get('a', 10000) !== 1) throw new Error('fresh ttl');\n"
                "    await new Promise(r => setTimeout(r, 60));\n"
                "    if (m.get('a', 20) !== null) throw new Error('expired should be null, got ' + JSON.stringify(m.get('a', 20)));\n"),
        },
        {
            "id": "g4c4-2",
            "prompt": "Add a size cap of 3 entries: set() must evict the OLDEST entry when the "
                      "store has 3 already (FIFO eviction). The current set() grows unbounded. "
                      "Implement FIFO eviction in kv.js. TTL get must keep working.",
            "bug": {"kv.js":
                "const store = new Map();\n"
                "exports.set = function (k, v) { store.set(k, { value: v, at: Date.now() }); };  // BUG: unbounded\n"
                "exports.get = function (k, ttlMs) {\n"
                "  if (!store.has(k)) return null;\n"
                "  const e = store.get(k);\n"
                "  if (ttlMs && Date.now() - e.at > ttlMs) { store.delete(k); return null; }\n"
                "  return e.value;\n"
                "};\n"
                "exports.del = function (k) { store.delete(k); };\n"
                "exports.keys = function () { return [...store.keys()]; };\n"},
            "fixed": {"kv.js":
                "const store = new Map();\n"
                "const MAX = 3;\n"
                "exports.set = function (k, v) {\n"
                "  if (store.has(k)) store.delete(k);\n"
                "  store.set(k, { value: v, at: Date.now() });\n"
                "  while (store.size > MAX) store.delete(store.keys().next().value);\n"
                "};\n"
                "exports.get = function (k, ttlMs) {\n"
                "  if (!store.has(k)) return null;\n"
                "  const e = store.get(k);\n"
                "  if (ttlMs && Date.now() - e.at > ttlMs) { store.delete(k); return null; }\n"
                "  return e.value;\n"
                "};\n"
                "exports.del = function (k) { store.delete(k); };\n"
                "exports.keys = function () { return [...store.keys()]; };\n"},
            "test": lambda d: rt(d, "kv",
                "m.set('a', 1); m.set('b', 2); m.set('c', 3);\n"
                "m.set('d', 4);\n"
                "if (m.get('a') !== null) throw new Error('oldest must be evicted');\n"
                "if (m.get('b') !== 2 || m.get('c') !== 3 || m.get('d') !== 4) throw new Error('rest must survive');\n"
                "if (m.keys().length !== 3) throw new Error('size ' + m.keys().length);\n"),
        },
        {
            "id": "g4c4-3",
            "prompt": "The express routes must use TTL on GET: GET /api/kv/:key must return "
                      "{value: null} when the entry is older than 5 seconds (5000ms). Currently the "
                      "route calls kv.get(key) with no ttl. Add the ttl to the route. FIFO + "
                      "logout of the store must keep working.",
            "bug": {"server.js": _expr(
                "const kv = require('./kv');\n"
                "app.put('/api/kv/:key', (req, res) => { kv.set(req.params.key, req.body.value); res.json({ ok: true }); });\n"
                "app.get('/api/kv/:key', (req, res) => res.json({ value: kv.get(req.params.key) }));  // BUG: no ttl\n"
                "app.delete('/api/kv/:key', (req, res) => { kv.del(req.params.key); res.json({ ok: true }); });\n"
                "app.get('/api/kv', (req, res) => res.json({ keys: kv.keys() }));\n")},
            "fixed": {"server.js": _expr(
                "const kv = require('./kv');\n"
                "app.put('/api/kv/:key', (req, res) => { kv.set(req.params.key, req.body.value); res.json({ ok: true }); });\n"
                "app.get('/api/kv/:key', (req, res) => res.json({ value: kv.get(req.params.key, 5000) }));\n"
                "app.delete('/api/kv/:key', (req, res) => { kv.del(req.params.key); res.json({ ok: true }); });\n"
                "app.get('/api/kv', (req, res) => res.json({ keys: kv.keys() }));\n")},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/kv/x', { method: 'PUT', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ value: 'v1' }) });\n"
                "    let j = await (await fetch(base + '/api/kv/x')).json();\n"
                "    if (j.value !== 'v1') throw new Error('fresh got ' + JSON.stringify(j));\n"
                "    await new Promise(res => setTimeout(res, 5300));\n"
                "    j = await (await fetch(base + '/api/kv/x')).json();\n"
                "    if (j.value !== null) throw new Error('expired should be null, got ' + JSON.stringify(j));\n"
            )),
        },
    ])

    # ── chain 5: task queue with retry ──
    ch5_base = {
        "queue.js":
            "let jobs = [];\n"
            "let processed = [];\n"
            "exports.enqueue = function (task) { jobs.push({ id: jobs.length + 1, task, status: 'pending' }); return jobs.length; };\n"
            "exports.next = function () { const j = jobs.find(x => x.status === 'pending'); if (j) j.status = 'processing'; return j || null; };\n"
            "exports.complete = function (id) { const j = jobs.find(x => x.id === id); if (j) { j.status = 'done'; processed.push(id); } };\n"
            "exports.stats = function () { return { pending: jobs.filter(x => x.status === 'pending').length, done: processed.length, total: jobs.length }; };\n",
        "server.js": _expr(
            "const q = require('./queue');\n"
            "app.post('/api/jobs', (req, res) => { const n = q.enqueue(req.body.task); res.status(201).json({ id: n }); });\n"
            "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
            "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch5", ch5_base, [
        {
            "id": "g4c5-1",
            "prompt": "Enqueue must reject empty tasks: POST /api/jobs with a missing/empty task "
                      "must return 400. The current enqueue accepts anything. Validate in "
                      "queue.js (return {ok:false,error}) and the route must 400.",
            "bug": {"queue.js":
                "let jobs = [];\n"
                "let processed = [];\n"
                "exports.enqueue = function (task) { jobs.push({ id: jobs.length + 1, task, status: 'pending' }); return jobs.length; };  // BUG: no validation\n"
                "exports.next = function () { const j = jobs.find(x => x.status === 'pending'); if (j) j.status = 'processing'; return j || null; };\n"
                "exports.complete = function (id) { const j = jobs.find(x => x.id === id); if (j) { j.status = 'done'; processed.push(id); } };\n"
                "exports.stats = function () { return { pending: jobs.filter(x => x.status === 'pending').length, done: processed.length, total: jobs.length }; };\n",
                "server.js": _expr(
                "const q = require('./queue');\n"
                "app.post('/api/jobs', (req, res) => { const n = q.enqueue(req.body.task); res.status(201).json({ id: n }); });\n"
                "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
                "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n")},
            "fixed": {"queue.js":
                "let jobs = [];\n"
                "let processed = [];\n"
                "exports.enqueue = function (task) {\n"
                "  if (typeof task !== 'string' || task.trim() === '') return { ok: false, error: 'task required' };\n"
                "  jobs.push({ id: jobs.length + 1, task, status: 'pending' });\n"
                "  return { ok: true, id: jobs.length };\n"
                "};\n"
                "exports.next = function () { const j = jobs.find(x => x.status === 'pending'); if (j) j.status = 'processing'; return j || null; };\n"
                "exports.complete = function (id) { const j = jobs.find(x => x.id === id); if (j) { j.status = 'done'; processed.push(id); } };\n"
                "exports.stats = function () { return { pending: jobs.filter(x => x.status === 'pending').length, done: processed.length, total: jobs.length }; };\n",
                "server.js": _expr(
                "const q = require('./queue');\n"
                "app.post('/api/jobs', (req, res) => { const r = q.enqueue(req.body.task); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ id: r.id }); });\n"
                "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
                "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n")},
            "test": lambda d: api(d, (
                "    let r = await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ task: '' }) });\n"
                "    if (r.status !== 400) throw new Error('empty should 400, got ' + r.status);\n"
                "    r = await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({}) });\n"
                "    if (r.status !== 400) throw new Error('missing should 400, got ' + r.status);\n"
                "    r = await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ task: 'build' }) });\n"
                "    if (r.status !== 201) throw new Error('valid should 201, got ' + r.status);\n"
                "    let j = await (await fetch(base + '/api/jobs')).json();\n"
                "    if (j.total !== 1) throw new Error('stats ' + JSON.stringify(j));\n"
            )),
        },
        {
            "id": "g4c5-2",
            "prompt": "Add a max queue size of 3: enqueue must return 400 with 'queue full' when "
                      "there are already 3 PENDING jobs. The current enqueue grows unbounded. "
                      "Validation from before must keep working.",
            "bug": {"queue.js":
                "let jobs = [];\n"
                "let processed = [];\n"
                "exports.enqueue = function (task) {\n"
                "  if (typeof task !== 'string' || task.trim() === '') return { ok: false, error: 'task required' };\n"
                "  jobs.push({ id: jobs.length + 1, task, status: 'pending' });\n"
                "  return { ok: true, id: jobs.length };\n"
                "};\n"
                "exports.next = function () { const j = jobs.find(x => x.status === 'pending'); if (j) j.status = 'processing'; return j || null; };\n"
                "exports.complete = function (id) { const j = jobs.find(x => x.id === id); if (j) { j.status = 'done'; processed.push(id); } };\n"
                "exports.stats = function () { return { pending: jobs.filter(x => x.status === 'pending').length, done: processed.length, total: jobs.length }; };  // BUG: no size cap\n",
                "server.js": _expr(
                "const q = require('./queue');\n"
                "app.post('/api/jobs', (req, res) => { const r = q.enqueue(req.body.task); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ id: r.id }); });\n"
                "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
                "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n")},
            "fixed": {"queue.js":
                "let jobs = [];\n"
                "let processed = [];\n"
                "const MAX_PENDING = 3;\n"
                "exports.enqueue = function (task) {\n"
                "  if (typeof task !== 'string' || task.trim() === '') return { ok: false, error: 'task required' };\n"
                "  if (jobs.filter(x => x.status === 'pending').length >= MAX_PENDING) return { ok: false, error: 'queue full' };\n"
                "  jobs.push({ id: jobs.length + 1, task, status: 'pending' });\n"
                "  return { ok: true, id: jobs.length };\n"
                "};\n"
                "exports.next = function () { const j = jobs.find(x => x.status === 'pending'); if (j) j.status = 'processing'; return j || null; };\n"
                "exports.complete = function (id) { const j = jobs.find(x => x.id === id); if (j) { j.status = 'done'; processed.push(id); } };\n"
                "exports.stats = function () { return { pending: jobs.filter(x => x.status === 'pending').length, done: processed.length, total: jobs.length }; };\n",
                "server.js": _expr(
                "const q = require('./queue');\n"
                "app.post('/api/jobs', (req, res) => { const r = q.enqueue(req.body.task); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ id: r.id }); });\n"
                "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
                "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n")},
            "test": lambda d: api(d, (
                "    for (let i = 0; i < 3; i++) {\n"
                "      const r = await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ task: 't' + i }) });\n"
                "      if (r.status !== 201) throw new Error('enqueue ' + i + ' -> ' + r.status);\n"
                "    }\n"
                "    let r = await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ task: 'overflow' }) });\n"
                "    if (r.status !== 400) throw new Error('4th should 400, got ' + r.status);\n"
                "    // process one -> pending frees up\n"
                "    await fetch(base + '/api/jobs/process', { method: 'POST' });\n"
                "    r = await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ task: 'again' }) });\n"
                "    if (r.status !== 201) throw new Error('after process should 201, got ' + r.status);\n"
            )),
        },
        {
            "id": "g4c5-3",
            "prompt": "Add a retry: POST /api/jobs/:id/retry must re-enqueue a DONE job (status "
                      "back to pending, same task). The current app has no retry route. Add it. "
                      "Queue size cap must keep working (a done job doesn't count against pending).",
            "bug": {"server.js": _expr(
                "const q = require('./queue');\n"
                "app.post('/api/jobs', (req, res) => { const r = q.enqueue(req.body.task); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ id: r.id }); });\n"
                "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
                "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n"
                "// BUG: no retry route\n")},
            "fixed": {"server.js": _expr(
                "const q = require('./queue');\n"
                "app.post('/api/jobs', (req, res) => { const r = q.enqueue(req.body.task); if (!r.ok) return res.status(400).json({ error: r.error }); res.status(201).json({ id: r.id }); });\n"
                "app.get('/api/jobs', (req, res) => res.json(q.stats()));\n"
                "app.post('/api/jobs/process', (req, res) => { const j = q.next(); if (!j) return res.status(404).json({ error: 'none' }); q.complete(j.id); res.json({ id: j.id }); });\n"
                "app.post('/api/jobs/:id/retry', (req, res) => { const r = q.retry(Number(req.params.id)); if (!r.ok) return res.status(404).json({ error: r.error }); res.json({ ok: true }); });\n"),
                "queue.js":
                "let jobs = [];\n"
                "let processed = [];\n"
                "const MAX_PENDING = 3;\n"
                "exports.enqueue = function (task) {\n"
                "  if (typeof task !== 'string' || task.trim() === '') return { ok: false, error: 'task required' };\n"
                "  if (jobs.filter(x => x.status === 'pending').length >= MAX_PENDING) return { ok: false, error: 'queue full' };\n"
                "  jobs.push({ id: jobs.length + 1, task, status: 'pending' });\n"
                "  return { ok: true, id: jobs.length };\n"
                "};\n"
                "exports.next = function () { const j = jobs.find(x => x.status === 'pending'); if (j) j.status = 'processing'; return j || null; };\n"
                "exports.complete = function (id) { const j = jobs.find(x => x.id === id); if (j) { j.status = 'done'; processed.push(id); } };\n"
                "exports.retry = function (id) {\n"
                "  const j = jobs.find(x => x.id === id);\n"
                "  if (!j || j.status !== 'done') return { ok: false, error: 'not done' };\n"
                "  j.status = 'pending';\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.stats = function () { return { pending: jobs.filter(x => x.status === 'pending').length, done: processed.length, total: jobs.length }; };\n"},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/jobs', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ task: 'x' }) });\n"
                "    let j = await (await fetch(base + '/api/jobs/process', { method: 'POST' })).json();\n"
                "    let stats = await (await fetch(base + '/api/jobs')).json();\n"
                "    if (stats.done !== 1) throw new Error('pre-retry EXPECTED done=1, GOT ' + JSON.stringify(stats));\n"
                "    let r = await fetch(base + '/api/jobs/' + j.id + '/retry', { method: 'POST' });\n"
                "    if (r.status !== 200) throw new Error('retry got ' + r.status);\n"
                "    stats = await (await fetch(base + '/api/jobs')).json();\n"
                "    if (stats.pending !== 1 || stats.done !== 1) throw new Error('post-retry EXPECTED pending=1 done=1, GOT ' + JSON.stringify(stats));\n"
            )),
        },
    ])

    # ══════════════════════════════════════════════════════════════════════
    # gen4-log — deeper debug-from-log
    # ══════════════════════════════════════════════════════════════════════
    def log_task(tid, prompt, log_content, buggy_files, fixed_files, test_lambda):
        files = dict(buggy_files)
        files["debug.log"] = log_content
        return {
            "id": tid,
            "category": "gen4-log",
            "prompt": prompt + "\n\nA captured debug.log is included in the project — READ IT FIRST; "
                      "it contains the exact failure from a real run.",
            "setup": lambda d, _f=files: (write(d, _f), ensure_deps(d)),
            "test": test_lambda,
            "_fixed": fixed_files,
        }

    tasks.append(log_task(
        "g4l-multiline-trace",
        "utils.js exports processOrder(order) which should return the order with total filled in. "
        "The debug.log has a MULTILINE traceback that ends with the real error. The real cause: "
        "processOrder calls items.map on order.items but one order has items undefined. Fix it to "
        "default items to [].",
        "[15:00:11] INFO  processing order #42\n"
        "[15:00:11] ERROR TypeError: Cannot read properties of undefined (reading 'map')\n"
        "    at processOrder (utils.js:3:26)\n"
        "    at main.js:12:9\n"
        "    at processOrder (utils.js:2:3)\n"
        "[15:00:11] ERROR order #42 failed\n",
        {"utils.js":
            "function processOrder(order) {\n"
            "  const items = order.items.map(i => i.price);  // BUG: items may be undefined\n"
            "  const total = items.reduce((a, b) => a + b, 0);\n"
            "  return { ...order, total };\n"
            "}\n"
            "module.exports = { processOrder };\n",
            },
        {"utils.js":
            "function processOrder(order) {\n"
            "  const items = (order.items || []).map(i => i.price);\n"
            "  const total = items.reduce((a, b) => a + b, 0);\n"
            "  return { ...order, total };\n"
            "}\n"
            "module.exports = { processOrder };\n"},
        lambda d: rt(d, "utils",
            "if (m.processOrder({ id: 1, items: [{ price: 5 }, { price: 7 }] }).total !== 12) throw new Error('normal');\n"
            "if (m.processOrder({ id: 2 }).total !== 0) throw new Error('no items got ' + m.processOrder({ id: 2 }).total);\n"),
    ))
    tasks.append(log_task(
        "g4l-misleading-message",
        "utils.js exports parseLine(line) — the debug.log says 'TypeError: Cannot read properties "
        "of undefined (reading 'trim')' which POINTS at line 4, but the actual bug is earlier: the "
        "function splits on ',' but the input may have no comma at all, so parts[1] is undefined. "
        "The error message is MISLEADING (it blames .trim on line 4). Fix the guard so parts[1] "
        "missing → age defaults to 0.",
        "[15:05:22] ERROR TypeError: Cannot read properties of undefined (reading 'trim')\n"
        "    at parseLine (utils.js:4:29)\n"
        "    at main.js:6:11\n",
        {"utils.js":
            "function parseLine(line) {\n"
            "  const parts = line.split(',');\n"
            "  return { name: parts[0].trim(), age: Number(parts[1].trim()) };  // BUG: parts[1] undefined when no comma\n"
            "}\n"
            "module.exports = { parseLine };\n"},
        {"utils.js":
            "function parseLine(line) {\n"
            "  const parts = line.split(',');\n"
            "  const age = parts[1] !== undefined ? Number(parts[1].trim()) : 0;\n"
            "  return { name: parts[0].trim(), age };\n"
            "}\n"
            "module.exports = { parseLine };\n"},
        lambda d: rt(d, "utils",
            "if (JSON.stringify(m.parseLine('bob')) !== '{\"name\":\"bob\",\"age\":0}') throw new Error('no comma got ' + JSON.stringify(m.parseLine('bob')));\n"
            "if (m.parseLine('alice,30').age !== 30) throw new Error('comma got ' + m.parseLine('alice,30').age);\n"),
    ))
    tasks.append(log_task(
        "g4l-wrong-frame",
        "utils.js exports render(data) which should return data.text uppercased. The debug.log "
        "shows the traceback pointing at render (utils.js:2) but the REAL problem is that callers "
        "pass a STRING, and the code treats it as an object — data.text is undefined for a string. "
        "Wait — actually the log shows 'undefined' because render receives data as a string. The "
        "bug is IN render: it must handle both. Fix render so render('hello') === 'HELLO' and "
        "render({text:'hi'}) === 'HI'.",
        "[15:10:45] WARN  render returned 'undefined' — input was a raw string, not {text}\n"
        "[15:10:45]      at render (utils.js:2:10)\n",
        {"utils.js":
            "function render(data) { return data.text.toUpperCase(); }  // BUG: string input -> data.text undefined\n"
            "module.exports = { render };\n"},
        {"utils.js":
            "function render(data) { return (typeof data === 'string' ? data : data.text).toUpperCase(); }\n"
            "module.exports = { render };\n"},
        lambda d: rt(d, "utils",
            "if (m.render('hello') !== 'HELLO') throw new Error('string got ' + m.render('hello'));\n"
            "if (m.render({ text: 'hi' }) !== 'HI') throw new Error('object got ' + m.render({ text: 'hi' }));\n"),
    ))
    tasks.append(log_task(
        "g4l-stateful-crash",
        "utils.js exports open(file) and read(). The debug.log shows open succeeds then read "
        "crashes with 'Cannot read properties of undefined' — because open() stores the handle in "
        "a MODULE variable but read() shadows it with a local one. Remove the shadow so read() "
        "uses the stored handle.",
        "[15:15:33] INFO  opened file.txt\n"
        "[15:15:33] ERROR TypeError: Cannot read properties of undefined (reading 'content')\n"
        "    at read (utils.js:9:20)\n",
        {"utils.js":
            "let handle = null;\n"
            "exports.open = function (file) { handle = { name: file, content: 'data' }; return true; };\n"
            "exports.read = function () { const handle = handle; return handle.content; }  // BUG: shadows module handle\n"},
        {"utils.js":
            "let handle = null;\n"
            "exports.open = function (file) { handle = { name: file, content: 'data' }; return true; };\n"
            "exports.read = function () { return handle ? handle.content : null; }"},
        lambda d: rt(d, "utils",
            "if (m.read() !== null) throw new Error('before open got ' + m.read());\n"
            "m.open('f.txt');\n"
            "if (m.read() !== 'data') throw new Error('after open got ' + m.read());\n"
            "m.open('g.txt');\n"
            "if (m.read() !== 'data') throw new Error('after reopen got ' + m.read());\n"
            "if (m.read() !== 'data') throw new Error('repeat got ' + m.read());\n"),
    ))
    tasks.append(log_task(
        "g4l-race-promise",
        "utils.js exports loadBoth() which must return {a, b} where a = fetchA result and b = "
        "fetchB result. The debug.log shows b is undefined — the code resolves b BEFORE a's "
        "promise settles (Promise.all order bug: it maps and indexes wrong). Fix loadBoth to await "
        "both correctly.",
        "[15:20:11] WARN  loadBoth returned {a:{...}, b:undefined} — b resolved early\n"
        "[15:20:11]      at loadBoth (utils.js:6)\n",
        {"utils.js":
            "async function fetchA() { return new Promise(r => setTimeout(() => r({ id: 'a' }), 30)); }\n"
            "async function fetchB() { return new Promise(r => setTimeout(() => r({ id: 'b' }), 10)); }\n"
            "async function loadBoth() { const both = await Promise.all([fetchA(), fetchB()]); return { a: both[1], b: both[0] }; }  // BUG: swapped indices\n"
            "module.exports = { loadBoth };\n"},
        {"utils.js":
            "async function fetchA() { return new Promise(r => setTimeout(() => r({ id: 'a' }), 30)); }\n"
            "async function fetchB() { return new Promise(r => setTimeout(() => r({ id: 'b' }), 10)); }\n"
            "async function loadBoth() { const both = await Promise.all([fetchA(), fetchB()]); return { a: both[0], b: both[1] }; }\n"
            "module.exports = { loadBoth };\n"},
        lambda d: rat(d, "utils",
            "    const r = await m.loadBoth();\n"
            "    if (!r.a || r.a.id !== 'a' || !r.b || r.b.id !== 'b') throw new Error('got ' + JSON.stringify(r));\n"),
    ))
    tasks.append(log_task(
        "g4l-soft-fail-log",
        "utils.js exports discount(price) which should apply 10% off. The debug.log shows it "
        "returned a NEGATIVE price for discount(5) — the code subtracts 10% of price but the "
        "implementation mistakenly does price - (price + 10% of price) → negative. Fix the "
        "arithmetic so discount(100) === 90.",
        "[15:25:08] WARN  discount(5) returned -5.5 — negative price!\n",
        {"utils.js":
            "function discount(price) { return price - (price + price * 0.1); }  // BUG: wrong arithmetic\n"
            "module.exports = { discount };\n"},
        {"utils.js":
            "function discount(price) { return price - price * 0.1; }\n"
            "module.exports = { discount };\n"},
        lambda d: rt(d, "utils",
            "if (m.discount(100) !== 90) throw new Error('got ' + m.discount(100));\n"
            "if (m.discount(10) !== 9) throw new Error('ten got ' + m.discount(10));\n"),
    ))
    tasks.append(log_task(
        "g4l-shadow-global",
        "utils.js exports makeSuffix(suffix) which returns a function appending suffix. The "
        "debug.log shows ALL returned functions use the LAST suffix — a loop-closure bug in "
        "makeSuffixes(prefixes). Fix the closure capture (use a let or IIFE).",
        "[15:30:21] WARN  makeSuffixes(['-a','-b']) -> both functions returned '-b' suffix\n",
        {"utils.js":
            "function makeSuffixes(prefixes) {\n"
            "  const fns = [];\n"
            "  for (var i = 0; i < prefixes.length; i++) {\n"
            "    var p = prefixes[i];\n"
            "    fns.push(function (s) { return s + p; });  // BUG: all close over last p\n"
            "  }\n"
            "  return fns;\n"
            "}\n"
            "module.exports = { makeSuffixes };\n"},
        {"utils.js":
            "function makeSuffixes(prefixes) {\n"
            "  const fns = [];\n"
            "  for (let i = 0; i < prefixes.length; i++) {\n"
            "    const p = prefixes[i];\n"
            "    fns.push(function (s) { return s + p; });\n"
            "  }\n"
            "  return fns;\n"
            "}\n"
            "module.exports = { makeSuffixes };\n"},
        lambda d: rt(d, "utils",
            "const fns = m.makeSuffixes(['-a', '-b']);\n"
            "if (fns[0]('x') !== 'x-a') throw new Error('f0 got ' + fns[0]('x'));\n"
            "if (fns[1]('x') !== 'x-b') throw new Error('f1 got ' + fns[1]('x'));\n"),
    ))
    tasks.append(log_task(
        "g4l-type-coerce",
        "utils.js exports compareVersions(a, b) which returns 1 if a > b, -1 if a < b, 0 if equal "
        "(as VERSION strings: '1.10' > '1.9'). The debug.log shows compareVersions('1.9','1.10') "
        "returned 1 (WRONG — should be -1) because it compares strings lexicographically. Split "
        "into numeric parts and compare numerically.",
        "[15:35:12] ERROR compareVersions('1.9','1.10') returned 1, expected -1\n",
        {"utils.js":
            "function compareVersions(a, b) { return a < b ? -1 : a > b ? 1 : 0; }  // BUG: lexicographic\n"
            "module.exports = { compareVersions };\n"},
        {"utils.js":
            "function compareVersions(a, b) {\n"
            "  const pa = a.split('.').map(Number), pb = b.split('.').map(Number);\n"
            "  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {\n"
            "    const x = pa[i] || 0, y = pb[i] || 0;\n"
            "    if (x !== y) return x < y ? -1 : 1;\n"
            "  }\n"
            "  return 0;\n"
            "}\n"
            "module.exports = { compareVersions };\n"},
        lambda d: rt(d, "utils",
            "if (m.compareVersions('1.10', '1.9') !== 1) throw new Error('1.10 got ' + m.compareVersions('1.10', '1.9'));\n"
            "if (m.compareVersions('1.9', '1.10') !== -1) throw new Error('1.9 got ' + m.compareVersions('1.9', '1.10'));\n"
            "if (m.compareVersions('2.0', '2.0') !== 0) throw new Error('equal');\n"),
    ))
    tasks.append(log_task(
        "g4l-null-guard",
        "utils.js exports merge(a, b) which must deep-merge two objects (b wins on conflicts), "
        "and must NOT throw when a or b is null (return the other). The debug.log shows a crash "
        "on merge(null, {x:1}) — the code reads a.length. Guard both args.",
        "[15:40:44] ERROR TypeError: Cannot read properties of null (reading 'length')\n"
        "    at merge (utils.js:1)\n",
        {"utils.js":
            "function merge(a, b) { return { ...a, ...b, len: a.length }; }  // BUG: a null -> crash\n"
            "module.exports = { merge };\n"},
        {"utils.js":
            "function merge(a, b) { return { ...(a || {}), ...(b || {}) }; }\n"
            "module.exports = { merge };\n"},
        lambda d: rt(d, "utils",
            "if (JSON.stringify(m.merge(null, { x: 1 })) !== '{\"x\":1}') throw new Error('null a');\n"
            "if (JSON.stringify(m.merge({ y: 2 }, null)) !== '{\"y\":2}') throw new Error('null b');\n"
            "if (JSON.stringify(m.merge({ a: 1 }, { a: 2 })) !== '{\"a\":2}') throw new Error('conflict');\n"),
    ))
    tasks.append(log_task(
        "g4l-array-limit",
        "utils.js exports firstN(items, n) which returns the first n items, clamped to array "
        "length (firstN([1,2,3], 99) === [1,2,3]). The debug.log shows firstN returned undefined "
        "elements for an out-of-range n — it uses slice(n) wrong (returns LAST n). Fix it.",
        "[15:45:30] WARN  firstN([1,2,3], 99) returned [undefined x 99]\n",
        {"utils.js":
            "function firstN(items, n) { return Array.from({ length: n }, (_, i) => items[i]); }  // BUG: pads with undefined\n"
            "module.exports = { firstN };\n"},
        {"utils.js":
            "function firstN(items, n) { return items.slice(0, n); }\n"
            "module.exports = { firstN };\n"},
        lambda d: rt(d, "utils",
            "if (m.firstN([1, 2, 3], 2).join(',') !== '1,2') throw new Error('two got ' + m.firstN([1, 2, 3], 2).join(','));\n"
            "if (m.firstN([1, 2, 3], 99).join(',') !== '1,2,3') throw new Error('clamp got ' + m.firstN([1, 2, 3], 99).join(','));\n"
            "if (m.firstN([], 5).length !== 0) throw new Error('empty');\n"
            "if (m.firstN([1, 2, 3], 0).length !== 0) throw new Error('zero got ' + JSON.stringify(m.firstN([1, 2, 3], 0)));\n"
            "if (m.firstN([1, 2, 3], 3).join(',') !== '1,2,3') throw new Error('exact got ' + m.firstN([1, 2, 3], 3).join(','));\n"),
    ))
    tasks.append(log_task(
        "g4l-prototype-key",
        "utils.js exports pick(obj, keys) which returns {key: value} for each key present in obj "
        "— keys NOT present must be OMITTED, even prototype keys like 'toString'. The debug.log "
        "shows pick leaking prototype members. Use hasOwnProperty.",
        "[15:50:02] WARN  pick({a:1}, ['toString']) returned a function — proto leak\n",
        {"utils.js":
            "function pick(obj, keys) { const out = {}; for (const k of keys) if (obj[k] !== undefined) out[k] = obj[k]; return out; }  // BUG: proto leak\n"
            "module.exports = { pick };\n"},
        {"utils.js":
            "function pick(obj, keys) { const out = {}; for (const k of keys) if (Object.prototype.hasOwnProperty.call(obj, k)) out[k] = obj[k]; return out; }\n"
            "module.exports = { pick };\n"},
        lambda d: rt(d, "utils",
            "if (JSON.stringify(m.pick({ a: 1 }, ['a'])) !== '{\"a\":1}') throw new Error('own');\n"
            "const p = m.pick({ a: 1 }, ['toString']);\n"
            "if (Object.prototype.hasOwnProperty.call(p, 'toString')) throw new Error('proto leaked: ' + JSON.stringify(p));\n"),
    ))
    tasks.append(log_task(
        "g4l-mutated-input",
        "utils.js exports sortedKeys(obj) which must return the object's keys sorted, WITHOUT "
        "modifying obj. The debug.log shows the ORIGINAL object's key order changed (the code "
        "deleted a key during iteration). Fix it to not mutate the input.",
        "[15:55:19] WARN  original object lost key 'b' after sortedKeys\n",
        {"utils.js":
            "function sortedKeys(obj) { const keys = Object.keys(obj); keys.sort(); delete obj[keys[0]]; return keys; }  // BUG: deletes a key!\n"
            "module.exports = { sortedKeys };\n"},
        {"utils.js":
            "function sortedKeys(obj) { return Object.keys(obj).sort(); }\n"
            "module.exports = { sortedKeys };\n"},
        lambda d: rt(d, "utils",
            "const o = { b: 2, a: 1 };\n"
            "const k = m.sortedKeys(o);\n"
            "if (k.join(',') !== 'a,b') throw new Error('keys ' + k.join(','));\n"
            "if (JSON.stringify(o) !== '{\"b\":2,\"a\":1}') throw new Error('MUTATED: ' + JSON.stringify(o));\n"),
    ))
    tasks.append(log_task(
        "g4l-deep-nesting",
        "utils.js exports flatten(items) which flattens one level of nesting "
          "(flatten([[1,2],[3]]) === [1,2,3]). The debug.log shows it returns a STRING when "
          "items is a single nested array — the code does items.join(',') instead of concat. Fix "
          "it to return an array.",
        "[16:00:05] ERROR flatten([[1,2],[3]]) returned '1,2,3' (string, not array)\n",
        {"utils.js":
            "function flatten(items) { return items.join(','); }  // BUG: returns string\n"
            "module.exports = { flatten };\n"},
        {"utils.js":
            "function flatten(items) { return items.reduce((a, b) => a.concat(b), []); }\n"
            "module.exports = { flatten };\n"},
        lambda d: rt(d, "utils",
            "const r = m.flatten([[1, 2], [3]]);\n"
            "if (!Array.isArray(r) || r.join(',') !== '1,2,3') throw new Error('got ' + JSON.stringify(r));\n"),
    ))
    tasks.append(log_task(
        "g4l-wrong-param",
        "utils.js exports formatCurrency(n) which returns '$' + n.toFixed(2). The debug.log shows "
        "it returned '$10.00' for formatCurrency(10.5)?? No — the real bug: callers pass a STRING "
        "'10.5' and toFixed doesn't exist on strings → crash. Fix formatCurrency to coerce with "
        "Number().",
        "[16:05:44] ERROR TypeError: n.toFixed is not a function\n"
        "    at formatCurrency (utils.js:1)\n"
        "    at main.js:4 (called with '10.5')\n",
        {"utils.js":
            "function formatCurrency(n) { return '$' + n.toFixed(2); }  // BUG: string input crashes\n"
            "module.exports = { formatCurrency };\n"},
        {"utils.js":
            "function formatCurrency(n) { return '$' + Number(n).toFixed(2); }\n"
            "module.exports = { formatCurrency };\n"},
        lambda d: rt(d, "utils",
            "if (m.formatCurrency(10.5) !== '$10.50') throw new Error('num got ' + m.formatCurrency(10.5));\n"
            "if (m.formatCurrency('10.5') !== '$10.50') throw new Error('string got ' + m.formatCurrency('10.5'));\n"
            "if (m.formatCurrency(0) !== '$0.00') throw new Error('zero got ' + m.formatCurrency(0));\n"
            "if (m.formatCurrency('0') !== '$0.00') throw new Error('zero-str got ' + m.formatCurrency('0'));\n"
            "if (m.formatCurrency(1.999) !== '$2.00') throw new Error('round got ' + m.formatCurrency(1.999));\n"),
    ))
    tasks.append(log_task(
        "g4l-chained-state",
        "utils.js exports counter() which returns an object {inc, get}. The debug.log shows TWO "
          "counters created by counter() SHARE state — the module variable is shared. Each "
          "counter() call must return an INDEPENDENT counter (closures, not module state).",
        "[16:10:09] WARN  counter A and B share the same count (A:1, B:1 after only A.inc)\n",
        {"utils.js":
            "let count = 0;\n"
            "function counter() { return { inc: () => { count += 1; }, get: () => count }; }  // BUG: shared module state\n"
            "module.exports = { counter };\n"},
        {"utils.js":
            "function counter() { let count = 0; return { inc: () => { count += 1; }, get: () => count }; }\n"
            "module.exports = { counter };\n"},
        lambda d: rt(d, "utils",
            "const a = m.counter(); const b = m.counter();\n"
            "a.inc(); a.inc();\n"
            "if (a.get() !== 2) throw new Error('a got ' + a.get());\n"
            "if (b.get() !== 0) throw new Error('b got ' + b.get());\n"),
    ))
    tasks.append(log_task(
        "g4l-sort-copy",
        "utils.js exports topScores(scores, n) which returns the top n scores (descending) as a "
        "NEW array, leaving scores unchanged. The debug.log shows the original array was sorted "
        "in place. Fix it to work on a copy.",
        "[16:15:30] WARN  original scores array was reordered by topScores\n",
        {"utils.js":
            "function topScores(scores, n) { scores.sort((a, b) => b - a); return scores.slice(0, n); }  // BUG: sorts in place\n"
            "module.exports = { topScores };\n"},
        {"utils.js":
            "function topScores(scores, n) { return [...scores].sort((a, b) => b - a).slice(0, n); }\n"
            "module.exports = { topScores };\n"},
        lambda d: rt(d, "utils",
            "const s = [3, 9, 4, 7];\n"
            "if (m.topScores(s, 2).join(',') !== '9,7') throw new Error('top got ' + m.topScores(s, 2).join(','));\n"
            "if (s.join(',') !== '3,9,4,7') throw new Error('MUTATED: ' + s.join(','));\n"
            "const s2 = [1];\n"
            "if (m.topScores(s2, 1).join(',') !== '1') throw new Error('single got ' + m.topScores(s2, 1).join(','));\n"
            "if (s2.join(',') !== '1') throw new Error('single MUTATED');\n"
            "if (m.topScores([], 3).length !== 0) throw new Error('empty got ' + JSON.stringify(m.topScores([], 3)));\n"),
    ))
    tasks.append(log_task(
        "g4l-closure-capture",
        "utils.js exports makeGreeter(name) which must return a function greeting with that name. "
        "The debug.log shows the name is UNDEFINED at call time — the parameter was captured by "
        "reference in a way that fails after the factory returns. Fix the closure.",
        "[16:20:11] ERROR makeGreeter('bob')() returned 'Hello undefined'\n",
        {"utils.js":
            "function makeGreeter(name) { return function () { return 'Hello ' + name; }; }  // looks fine...\n"
            "function makeGreeters(names) { const fns = []; for (const n of names) { fns.push(() => 'Hello ' + name); } return fns; }  // BUG: uses `name` not `n`\n"
            "module.exports = { makeGreeter, makeGreeters };\n"},
        {"utils.js":
            "function makeGreeter(name) { return function () { return 'Hello ' + name; }; }\n"
            "function makeGreeters(names) { const fns = []; for (const n of names) { fns.push(() => 'Hello ' + n); } return fns; }\n"
            "module.exports = { makeGreeter, makeGreeters };\n"},
        lambda d: rt(d, "utils",
            "if (m.makeGreeter('bob')() !== 'Hello bob') throw new Error('single got ' + m.makeGreeter('bob')());\n"
            "const gs = m.makeGreeters(['a', 'b']);\n"
            "if (gs[0]() !== 'Hello a') throw new Error('g0 got ' + gs[0]());\n"
            "if (gs[1]() !== 'Hello b') throw new Error('g1 got ' + gs[1]());\n"
            "const g3 = m.makeGreeters(['x', 'y', 'z']);\n"
            "if (g3[2]() !== 'Hello z') throw new Error('g2 got ' + g3[2]());\n"
            "if (m.makeGreeter('')() !== 'Hello ') throw new Error('empty got ' + m.makeGreeter('')());\n"),
    ))

    # ══════════════════════════════════════════════════════════════════════
    # gen4-long — GENERATED long-horizon projects (context stress)
    # The setup generates 25-45 files with filler so Dev must explore efficiently.
    # ══════════════════════════════════════════════════════════════════════
    def gen_modules(n, buggy_idx, buggy_fn, good_fn, prefix="mod"):
        """Generate n modules each exporting compute(x) with one buggy index.
        buggy_fn / good_fn are function-body STRING templates with {i} substituted."""
        files = {}
        for i in range(n):
            if i == buggy_idx:
                body = buggy_fn.format(i=i)
            else:
                body = good_fn.format(i=i)
            files[f"lib/modules/{prefix}{i}.js"] = (
                "// Module " + str(i) + " — utilities for the pipeline.\n"
                "// Each module exposes compute(x) used by index.js.\n"
                "function compute(x) { " + body + " }\n"
                "module.exports = { compute };\n"
            )
        return files

    def gen_index(n, prefix="mod"):
        lines = ["module.exports = {"]
        for i in range(n):
            lines.append(f"  m{i}: require('./lib/modules/{prefix}{i}').compute({i}),")
        lines.append("};")
        return "\n".join(lines) + "\n"

    files = gen_modules(25, 17, "return x;", "return x * 2;", "mod")
    files["index.js"] = gen_index(25, "mod")
    _files_25 = files
    tasks.append({
        "id": "g4lng-25-modules",
        "category": "gen4-long",
        "prompt": "A project has 25 modules in lib/modules/, each exporting compute(x). All return "
                  "x * 2 EXCEPT ONE which returns x unchanged (it forgot the multiplication). "
                  "index.js calls all 25 and collects results. Find the ONE wrong module and fix "
                  "it so every compute(i) returns i * 2.",
        "setup": lambda d, _f=_files_25: write(d, _f),
        "test": lambda d: rt(d, "index",
            "const r = m;\n"
            "for (const k of Object.keys(r)) { if (r[k] !== Number(k.slice(1)) * 2) throw new Error(k + ' got ' + r[k]); }\n"),
        "_fixed": {"lib/modules/mod17.js":
            "// Module 17 — utilities for the pipeline.\n"
            "// Each module exposes compute(x) used by index.js.\n"
            "function compute(x) { return x * 2; }\n"
            "module.exports = { compute };\n"},
    })

    files = gen_modules(35, 29, "return x + 1;", "return x * 3;", "mod")
    files["index.js"] = gen_index(35, "mod")
    _files_35 = files
    tasks.append({
        "id": "g4lng-35-modules",
        "category": "gen4-long",
        "prompt": "A project has 35 modules in lib/modules/. compute(x) should return x * 3. One "
                  "module (the one with the highest index among those that return wrong values) "
                  "returns x + 1 instead. Find it and fix it.",
        "setup": lambda d, _f=_files_35: write(d, _f),
        "test": lambda d: rt(d, "index",
            "const r = m;\n"
            "for (const k of Object.keys(r)) { if (r[k] !== Number(k.slice(1)) * 3) throw new Error(k + ' got ' + r[k]); }\n"),
        "_fixed": {"lib/modules/mod29.js":
            "// Module 29 — utilities for the pipeline.\n"
            "// Each module exposes compute(x) used by index.js.\n"
            "function compute(x) { return x * 3; }\n"
            "module.exports = { compute };\n"},
    })

    # 40 modules, TWO bugs (indices 11 and 33)
    files = gen_modules(40, 11, "return 0;", "return x * 4;", "mod")
    files.update(gen_modules(40, 33, "return x * 5;", "return x * 4;", "mod"))
    files["index.js"] = gen_index(40, "mod")
    _files_40 = files
    tasks.append({
        "id": "g4lng-40-two-bugs",
        "category": "gen4-long",
        "prompt": "A project has 40 modules in lib/modules/. compute(x) should return x * 4. TWO "
                  "modules are wrong: one returns 0 always, another returns x * 5. Find BOTH and "
                  "fix them — the hidden test checks all 40.",
        "setup": lambda d, _f=_files_40: write(d, _f),
        "test": lambda d: rt(d, "index",
            "const r = m;\n"
            "for (const k of Object.keys(r)) { if (r[k] !== Number(k.slice(1)) * 4) throw new Error(k + ' got ' + r[k]); }\n"),
        "_fixed": {
            "lib/modules/mod11.js":
                "// Module 11 — utilities for the pipeline.\n"
                "// Each module exposes compute(x) used by index.js.\n"
                "function compute(x) { return x * 4; }\n"
                "module.exports = { compute };\n",
            "lib/modules/mod33.js":
                "// Module 33 — utilities for the pipeline.\n"
                "// Each module exposes compute(x) used by index.js.\n"
                "function compute(x) { return x * 4; }\n"
                "module.exports = { compute };\n",
        },
    })

    # Deep directory tree: 4 levels, bug at the deepest level
    tasks.append({
        "id": "g4lng-deep-tree",
        "category": "gen4-long",
        "prompt": "The bug is at the BOTTOM of a deep directory tree. lib/services/a/b/c/d/"
                  "worker.js exports run(n) which must return n + 10 — it currently returns n. "
                  "Every other file is correct. index.js calls run(5) through the chain and "
                  "expects 15. Find the buggy file by exploring the tree.",
        "setup": lambda d: write(d, {
            "index.js": "const { go } = require('./lib/services/go');\nmodule.exports = { result: go(5) };\n",
            "lib/services/go.js": "const { step1 } = require('./a/step1');\nexports.go = (n) => step1(n);\n",
            "lib/services/a/step1.js": "const { step2 } = require('./b/step2');\nexports.step1 = (n) => step2(n);\n",
            "lib/services/a/b/step2.js": "const { step3 } = require('./c/step3');\nexports.step2 = (n) => step3(n);\n",
            "lib/services/a/b/c/step3.js": "const { run } = require('./d/worker');\nexports.step3 = (n) => run(n);\n",
            "lib/services/a/b/c/d/worker.js":
                "function run(n) { return n; }  // BUG: should be n + 10\n"
                "module.exports = { run };\n",
        }),
        "test": lambda d: rt(d, "index",
            "if (m.result !== 15) throw new Error('got ' + m.result);\n"),
        "_fixed": {"lib/services/a/b/c/d/worker.js":
            "function run(n) { return n + 10; }\n"
            "module.exports = { run };\n"},
    })

    # Big single file with many functions
    tasks.append({
        "id": "g4lng-big-single",
        "category": "gen4-long",
        "prompt": "utils.js contains 40 small functions (fn00..fn39). All return n + i except ONE: "
                  "fn25 returns n - 25 instead of n + 25. Find fn25 and fix it. The hidden test "
                  "calls all 40.",
        "setup": lambda d: write(d, {"utils.js":
            "".join(
                f"function fn{i:02d}(n) {{ return n {('- ' + str(i)) if i == 25 else ('+ ' + str(i))}; }}\n"
                for i in range(40)
            ) +
            "module.exports = { " + ", ".join(f"fn{i:02d}" for i in range(40)) + " };\n"}),
        "test": lambda d: rt(d, "utils",
            "".join(
                f"if (m.fn{i:02d}(10) !== {10 + i}) throw new Error('fn{i:02d} got ' + m.fn{i:02d}(10));\n"
                for i in range(40)
            )),
        "_fixed": {"utils.js":
            "".join(f"function fn{i:02d}(n) {{ return n + {i}; }}\n" for i in range(40)) +
            "module.exports = { " + ", ".join(f"fn{i:02d}" for i in range(40)) + " };\n"},
    })

    # Nested dirs + 30 files, one bug in a specific file
    files = {}
    for d in range(5):
        for i in range(6):
            idx = d * 6 + i
            files[f"src/features/feat{d}/module{idx}.js"] = (
                "const { helper } = require('./helper');\n"
                "exports.value = helper(" + str(idx) + ");\n"
            )
        files[f"src/features/feat{d}/helper.js"] = (
            "function helper(x) { return x + 1; }\n"
            "module.exports = { helper };\n"
        )
    files["src/features/feat3/helper.js"] = (
        "function helper(x) { return x - 1; }  // BUG: this helper is wrong\n"
        "module.exports = { helper };\n"
    )
    files["index.js"] = (
        "module.exports = {\n" +
        "".join(f"  v{i}: require('./src/features/feat{i//6}/module{i}').value,\n" for i in range(30)) +
        "};\n"
    )
    _files_feat = files
    tasks.append({
        "id": "g4lng-feature-dirs",
        "category": "gen4-long",
        "prompt": "A project has 5 feature directories (feat0..feat4), each with 6 modules and a "
                  "helper.js. Every helper must return x + 1. ONE helper (in feat3) returns x - 1 "
                  "instead. index.js sums all 30 module values and must equal the sum of (i+1) for "
                  "i in 0..29 (which is 465). Find the wrong helper and fix it.",
        "setup": lambda d, _f=_files_feat: write(d, _f),
        "test": lambda d: rt(d, "index",
            "let total = 0; for (const k of Object.keys(m)) total += m[k];\n"
            "if (total !== 465) throw new Error('total ' + total);\n"),
        "_fixed": {"src/features/feat3/helper.js":
            "function helper(x) { return x + 1; }\n"
            "module.exports = { helper };\n"},
    })

    # Long dependency chain across many files with one broken link
    files = {"index.js": "module.exports = { result: require('./chain/link0').run(2) };\n"}
    for i in range(12):
        nxt = f"require('./link{i+1}')" if i < 11 else "null"
        files[f"chain/link{i}.js"] = (
            "const next = " + (f"require('./link{i+1}')" if i < 11 else "null") + ";\n"
            "exports.run = function (x) { const v = x + 1; return next ? next.run(v) : v; };\n"
        )
    files["chain/link6.js"] = (
        "const next = require('./link7');\n"
        "exports.run = function (x) { return next.run(x); };  // BUG: forgot + 1\n"
    )
    _files_chain = files
    tasks.append({
        "id": "g4lng-chain-links",
        "category": "gen4-long",
        "prompt": "chain/ has 12 links (link0..link11) forming a pipeline: each link.run(x) adds 1 "
                  "and passes to the next. ONE link (link6) FORGOT to add 1 — it passes x "
                  "through unchanged, so the final result is 1 less than expected. index.js starts "
                  "at link0 with 2; the correct final result is 14 (2 + 12). Find the broken link "
                  "and fix it.",
        "setup": lambda d, _f=_files_chain: write(d, _f),
        "test": lambda d: rt(d, "index",
            "if (m.result !== 14) throw new Error('got ' + m.result);\n"),
        "_fixed": {"chain/link6.js":
            "const next = require('./link7');\n"
            "exports.run = function (x) { const v = x + 1; return next.run(v); };\n"},
    })

    return tasks
