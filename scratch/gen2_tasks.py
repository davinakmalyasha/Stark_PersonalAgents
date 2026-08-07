"""gen2_tasks.py — 60-task "gen2" eval suite (4 categories x 15).

Categories:
  gen2-bugfix    : deep Node edge-case bugs (fp precision, surrogates, circular refs, ...)
  gen2-refactor  : behavior-preserving refactors with STRUCTURAL hidden checks
  gen2-python    : same class of bugs, Python stdlib only
  gen2-multifile : bugs spanning 2-3 files (shared state, require paths, caching, ...)

Each task dict may carry a `_fixed` field: the corrected files, used ONLY by the
pre-flight verifier (scratch/preflight_gen2.py) to prove the task is solvable.
The agent never sees `_fixed`.

`build_tasks(H)` receives the helper dict from eval_runner to avoid a circular import.
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
    py = H["py_test"]
    ref = H["refactor_test"]

    def express_setup(files):
        return lambda d: (write(d, files), ensure_deps(d))

    tasks = []

    # ══════════════════════════════════════════════════════════════════════
    # gen2-bugfix — deep Node edge cases
    # ══════════════════════════════════════════════════════════════════════
    tasks.append({
        "id": "g2-fp-precision",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports sumFloats(nums) which should return the sum of floating point "
                  "numbers rounded to 2 decimals (sumFloats([0.1, 0.2]) === 0.3). It currently returns "
                  "raw floating-point sums like 0.30000000000000004. Fix it so results are rounded to "
                  "2 decimal places.",
        "setup": lambda d: write(d, {"utils.js":
            "function sumFloats(nums) { return nums.reduce((a, b) => a + b, 0); }  // BUG: fp error\n"
            "module.exports = { sumFloats };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.sumFloats([0.1, 0.2]) !== 0.3) throw new Error('0.1+0.2 got ' + m.sumFloats([0.1, 0.2]));\n"
            "if (m.sumFloats([0.7, 0.1]) !== 0.8) throw new Error('0.7+0.1 got ' + m.sumFloats([0.7, 0.1]));\n"
            "if (m.sumFloats([1.5, 2.25]) !== 3.75) throw new Error('plain got ' + m.sumFloats([1.5, 2.25]));\n"
            "if (m.sumFloats([0.565]) !== 0.57) throw new Error('0.565 got ' + m.sumFloats([0.565]));\n"
            "if (m.sumFloats([1.005]) !== 1.01) throw new Error('1.005 got ' + m.sumFloats([1.005]));\n"
            "if (m.sumFloats([0.1, 0.1, 0.1]) !== 0.3) throw new Error('three-tenths got ' + m.sumFloats([0.1, 0.1, 0.1]));\n"
            "if (m.sumFloats([]) !== 0) throw new Error('empty got ' + m.sumFloats([]));\n"
            "if (m.sumFloats([2.675]) !== 2.68) throw new Error('2.675 got ' + m.sumFloats([2.675]));\n"),
        "_fixed": {"utils.js":
            "function sumFloats(nums) { return Math.round((nums.reduce((a, b) => a + b, 0) + Number.EPSILON) * 100) / 100; }\n"
            "module.exports = { sumFloats };\n"},
    })
    tasks.append({
        "id": "g2-async-race",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports firstSuccess(tasks) — an async function taking an array of async "
                  "functions. It must resolve with the result of the FIRST function that RESOLVES, "
                  "ignoring earlier rejections (if a fast task rejects and a slower one resolves, "
                  "return the resolved value). It currently uses Promise.race, which rejects if the "
                  "fastest task rejects. Fix it to return the first successful result.",
        "setup": lambda d: write(d, {"utils.js":
            "async function firstSuccess(tasks) { return Promise.race(tasks.map(t => t())); }  // BUG: rejects if fastest rejects\n"
            "module.exports = { firstSuccess };\n",
            }),
        "test": lambda d: rat(d, "utils",
            "    const slow = () => new Promise(res => setTimeout(() => res('SLOW'), 30));\n"
            "    const fast = async () => { throw new Error('fast fail'); };\n"
            "    const r = await m.firstSuccess([fast, slow]);\n"
            "    if (r !== 'SLOW') throw new Error('got ' + r);\n"
            "    const r2 = await m.firstSuccess([async () => 'OK', async () => 'OK2']);\n"
            "    if (r2 !== 'OK') throw new Error('first resolve got ' + r2);\n"),
        "_fixed": {"utils.js":
            "async function firstSuccess(tasks) {\n"
            "  return new Promise((resolve, reject) => {\n"
            "    let pending = tasks.length;\n"
            "    if (pending === 0) return reject(new Error('no tasks'));\n"
            "    for (const t of tasks) {\n"
            "      Promise.resolve().then(t).then(resolve, () => { if (--pending === 0) reject(new Error('all failed')); });\n"
            "    }\n"
            "  });\n"
            "}\n"
            "module.exports = { firstSuccess };\n"},
    })
    tasks.append({
        "id": "g2-date-parse",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports formatUTCDate(ms) which returns 'YYYY-MM-DD' for an epoch-milliseconds "
                  "timestamp in UTC (formatUTCDate(Date.UTC(2026, 0, 5, 23, 30)) === '2026-01-05'). It "
                  "currently uses LOCAL timezone getters, so the date can shift by a day. Fix it to use "
                  "UTC getters.",
        "setup": lambda d: write(d, {"utils.js":
            "function formatUTCDate(ms) { const d = new Date(ms); return d.getFullYear() + '-' + String(d.getMonth() + 1).padStart(2, '0') + '-' + String(d.getDate()).padStart(2, '0'); }  // BUG: local tz\n"
            "module.exports = { formatUTCDate };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.formatUTCDate(Date.UTC(2026, 0, 5, 23, 30)) !== '2026-01-05') throw new Error('tz bug: ' + m.formatUTCDate(Date.UTC(2026, 0, 5, 23, 30)));\n"
            "if (m.formatUTCDate(Date.UTC(2026, 11, 31, 1, 0)) !== '2026-12-31') throw new Error('yearend: ' + m.formatUTCDate(Date.UTC(2026, 11, 31, 1, 0)));\n"),
        "_fixed": {"utils.js":
            "function formatUTCDate(ms) { const d = new Date(ms); return d.getUTCFullYear() + '-' + String(d.getUTCMonth() + 1).padStart(2, '0') + '-' + String(d.getUTCDate()).padStart(2, '0'); }\n"
            "module.exports = { formatUTCDate };\n"},
    })
    tasks.append({
        "id": "g2-sort-tiebreak",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports sortTasks(tasks) which sorts tasks by priority (the 'p' field) "
                  "DESCENDING, and for EQUAL priorities by name ascending "
                  "(sortTasks([{name:'b',p:1},{name:'a',p:1},{name:'c',p:2}]) → c,a,b). It currently "
                  "sorts by priority only, so ties keep an arbitrary order. Fix the comparator to break "
                  "ties by name.",
        "setup": lambda d: write(d, {"utils.js":
            "function sortTasks(tasks) { return [...tasks].sort((x, y) => y.p - x.p); }  // BUG: no tiebreak\n"
            "module.exports = { sortTasks };\n"}),
        "test": lambda d: rt(d, "utils",
            "const r = m.sortTasks([{name:'b',p:1},{name:'a',p:1},{name:'c',p:2}]);\n"
            "if (r.map(t => t.name).join(',') !== 'c,a,b') throw new Error('got ' + r.map(t => t.name).join(','));\n"),
        "_fixed": {"utils.js":
            "function sortTasks(tasks) { return [...tasks].sort((x, y) => y.p - x.p || x.name.localeCompare(y.name)); }\n"
            "module.exports = { sortTasks };\n"},
    })
    tasks.append({
        "id": "g2-unicode-escape",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports escapeUnicode(s) which must escape ALL non-ASCII characters as "
                  "\\uXXXX (escapeUnicode('héllo→') === 'h\\\\u00e9llo\\\\u2192'). It currently only "
                  "escapes <, >, & — non-ASCII characters pass through raw. Fix it to escape every "
                  "non-ASCII code point (BMP).",
        "setup": lambda d: write(d, {"utils.js":
            "function escapeUnicode(s) { return s.replace(/[<>&]/g, c => ({'<':'&lt;','>':'&gt;','&':'&amp;'}[c])); }  // BUG: leaves non-ASCII\n"
            "module.exports = { escapeUnicode };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.escapeUnicode('h\u00e9llo\u2192') !== 'h\\\\u00e9llo\\\\u2192') throw new Error('got ' + JSON.stringify(m.escapeUnicode('h\u00e9llo\u2192')));\n"
            "if (m.escapeUnicode('plain') !== 'plain') throw new Error('plain changed');\n"
            "if (m.escapeUnicode('\u00fc\u00f1') !== '\\\\u00fc\\\\u00f1') throw new Error('umlauts ' + JSON.stringify(m.escapeUnicode('\u00fc\u00f1')));\n"
            "if (m.escapeUnicode('a b') !== 'a b') throw new Error('space broke');\n"),
        "_fixed": {"utils.js":
            "function escapeUnicode(s) { return s.replace(/[^\\x00-\\x7F]/g, c => '\\\\u' + c.charCodeAt(0).toString(16).padStart(4, '0')); }\n"
            "module.exports = { escapeUnicode };\n"},
    })
    tasks.append({
        "id": "g2-circular-json",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports safeStringify(obj) which must stringify ANY object without throwing, "
                  "replacing circular references with the string '[Circular]'. It currently calls "
                  "JSON.stringify directly, which throws TypeError on circular structures. Fix it with a "
                  "replacer that tracks already-visited objects.",
        "setup": lambda d: write(d, {"utils.js":
            "function safeStringify(obj) { return JSON.stringify(obj); }  // BUG: throws on circular\n"
            "module.exports = { safeStringify };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "const a = { x: 1 }; a.self = a;\n"
            "const out = m.safeStringify(a);\n"
            "if (typeof out !== 'string' || !out.includes('[Circular]')) throw new Error('got ' + out);\n"
            "const b = { y: 2 }; b.deep = { inner: b };\n"
            "const out2 = m.safeStringify(b);\n"
            "if (typeof out2 !== 'string' || !out2.includes('[Circular]')) throw new Error('deep circular: ' + out2);\n"
            "if (m.safeStringify({ a: 1 }) !== '{\"a\":1}') throw new Error('plain broke');\n"
            "const arr = [1]; arr.push(arr);\n"
            "if (!m.safeStringify(arr).includes('[Circular]')) throw new Error('array circular');\n"
            "const arr2 = [1]; const o2 = { inner: arr2 }; arr2.push(o2);\n"
            "const out3 = m.safeStringify(arr2);\n"
            "if (!out3.includes('[Circular]')) throw new Error('cross circular: ' + out3);\n"
            "if (m.safeStringify(null) !== 'null') throw new Error('null broke: ' + m.safeStringify(null));\n"
            "if (m.safeStringify({ s: 'hi', n: 5 }) !== '{\"s\":\"hi\",\"n\":5}') throw new Error('mixed broke');\n"),
        "_fixed": {"utils.js":
            "function safeStringify(obj) {\n"
            "  const seen = new WeakSet();\n"
            "  return JSON.stringify(obj, (k, v) => {\n"
            "    if (typeof v === 'object' && v !== null) {\n"
            "      if (seen.has(v)) return '[Circular]';\n"
            "      seen.add(v);\n"
            "    }\n"
            "    return v;\n"
            "  });\n"
            "}\n"
            "module.exports = { safeStringify };\n"},
    })
    tasks.append({
        "id": "g2-nan-zero",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports normalizeZeros(nums) which returns a new array where -0 becomes +0, "
                  "leaving everything else unchanged (Object.is(normalizeZeros([-0])[0], 0) must be "
                  "true). It currently returns the array as-is, keeping -0. Fix it to normalize negative "
                  "zeros.",
        "setup": lambda d: write(d, {"utils.js":
            "function normalizeZeros(nums) { return [...nums]; }  // BUG: keeps -0\n"
            "module.exports = { normalizeZeros };\n"}),
        "test": lambda d: rt(d, "utils",
            "const r = m.normalizeZeros([-0, 1, -0]);\n"
            "if (!Object.is(r[0], 0) || !Object.is(r[2], 0)) throw new Error('kept -0');\n"
            "if (r[1] !== 1) throw new Error('changed 1');\n"),
        "_fixed": {"utils.js":
            "function normalizeZeros(nums) { return nums.map(n => (Object.is(n, -0) ? 0 : n)); }\n"
            "module.exports = { normalizeZeros };\n"},
    })
    tasks.append({
        "id": "g2-deep-eq",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports deepEqual(a, b) which must compare nested objects and arrays by VALUE "
                  "(deepEqual({x:[1,2]}, {x:[1,2]}) === true, deepEqual({x:[1]}, {x:[1,2]}) === false). It "
                  "currently uses ===, so two structurally identical objects are never equal. Implement "
                  "deep equality (handle nested objects and arrays; assume JSON-like data).",
        "setup": lambda d: write(d, {"utils.js":
            "function deepEqual(a, b) { return a === b; }  // BUG: reference compare\n"
            "module.exports = { deepEqual };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (!m.deepEqual({x:[1,2]}, {x:[1,2]})) throw new Error('eq failed');\n"
            "if (m.deepEqual({x:[1]}, {x:[1,2]})) throw new Error('neq failed');\n"
            "if (!m.deepEqual({a:{b:{c:1}}}, {a:{b:{c:1}}})) throw new Error('nested failed');\n"
            "if (m.deepEqual([1,2], {0:1,1:2})) throw new Error('array vs obj');\n"
            "if (!m.deepEqual([1, 2], [1, 2])) throw new Error('arrays eq failed');\n"
            "if (m.deepEqual([1, 2], [2, 1])) throw new Error('arrays order');\n"
            "if (!m.deepEqual({x:[1,{y:2}]}, {x:[1,{y:2}]})) throw new Error('deep mixed failed');\n"
            "if (m.deepEqual({a:1}, {a:1,b:undefined})) throw new Error('extra undefined key');\n"
            "if (m.deepEqual(null, {})) throw new Error('null vs obj');\n"
            "if (!m.deepEqual(5, 5)) throw new Error('primitives');\n"),
        "_fixed": {"utils.js":
            "function deepEqual(a, b) {\n"
            "  if (a === b) return true;\n"
            "  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;\n"
            "  const ka = Array.isArray(a), kb = Array.isArray(b);\n"
            "  if (ka !== kb) return false;\n"
            "  const keysA = Object.keys(a), keysB = Object.keys(b);\n"
            "  if (keysA.length !== keysB.length) return false;\n"
            "  return keysA.every(k => deepEqual(a[k], b[k]));\n"
            "}\n"
            "module.exports = { deepEqual };\n"},
    })
    tasks.append({
        "id": "g2-neg-zero-format",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports formatMoney(n) which returns a 2-decimal string WITHOUT a minus sign "
                  "for negative-zero results (formatMoney(-0.001) === '0.00'). It currently returns "
                  "'-0.00' because (-0.001).toFixed(2) keeps the negative sign. Fix it so -0 formats as "
                  "'0.00'.",
        "setup": lambda d: write(d, {"utils.js":
            "function formatMoney(n) { return n.toFixed(2); }  // BUG: '-0.00'\n"
            "module.exports = { formatMoney };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.formatMoney(-0.001) !== '0.00') throw new Error('neg zero got ' + m.formatMoney(-0.001));\n"
            "if (m.formatMoney(1.5) !== '1.50') throw new Error('pos got ' + m.formatMoney(1.5));\n"
            "if (m.formatMoney(-1.5) !== '-1.50') throw new Error('neg got ' + m.formatMoney(-1.5));\n"
            "if (m.formatMoney(-0.004) !== '0.00') throw new Error('tiny neg got ' + m.formatMoney(-0.004));\n"
            "if (m.formatMoney(-0.006) !== '-0.01') throw new Error('rounds down got ' + m.formatMoney(-0.006));\n"),
        "_fixed": {"utils.js":
            "function formatMoney(n) { const s = n.toFixed(2); return Object.is(parseFloat(s), -0) ? '0.00' : s; }\n"
            "module.exports = { formatMoney };\n"},
    })
    tasks.append({
        "id": "g2-proto-pollution",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports safeMerge(target, source) which copies own enumerable properties from "
                  "source to target WITHOUT allowing __proto__ pollution (after safeMerge({}, "
                  "JSON.parse('{\"__proto__\": {\"polluted\": true}}')), the merged object must NOT have "
                  "a polluted prototype). It currently uses Object.assign, which triggers the __proto__ "
                  "setter and pollutes the prototype. Fix it to skip the __proto__ key.",
        "setup": lambda d: write(d, {"utils.js":
            "function safeMerge(target, source) { return Object.assign(target, source); }  // BUG: __proto__ pollution\n"
            "module.exports = { safeMerge };\n"}),
        "test": lambda d: rt(d, "utils",
            "const src = JSON.parse('{\"__proto__\": {\"polluted\": true}}');\n"
            "const t = m.safeMerge({}, src);\n"
            "if (t.polluted === true) throw new Error('POLLUTED');\n"
            "if (Object.keys(t).length !== 0) throw new Error('unexpected keys: ' + Object.keys(t));\n"
            "if (m.safeMerge({a:1}, {b:2}).b !== 2) throw new Error('normal merge broke');\n"),
        "_fixed": {"utils.js":
            "function safeMerge(target, source) {\n"
            "  for (const k of Object.keys(source)) {\n"
            "    if (k === '__proto__') continue;\n"
            "    target[k] = source[k];\n"
            "  }\n"
            "  return target;\n"
            "}\n"
            "module.exports = { safeMerge };\n"},
    })
    tasks.append({
        "id": "g2-dedupe-order",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports dedupePreserve(items) which removes duplicates keeping the FIRST "
                  "occurrence order (dedupePreserve(['a','b','a','c']) === ['a','b','c']). It currently "
                  "keeps the LAST occurrence because it reverses, dedupes, then reverses back. Fix it to "
                  "keep first occurrences.",
        "setup": lambda d: write(d, {"utils.js":
            "function dedupePreserve(items) { return [...new Set([...items].reverse())].reverse(); }  // BUG: keeps last\n"
            "module.exports = { dedupePreserve };\n"}),
        "test": lambda d: rt(d, "utils",
            "const r = m.dedupePreserve(['a','b','a','c']);\n"
            "if (r.join(',') !== 'a,b,c') throw new Error('got ' + r.join(','));\n"
            "if (m.dedupePreserve([1,2,1,3,2]).join(',') !== '1,2,3') throw new Error('nums');\n"
            "if (m.dedupePreserve(['x','x']).length !== 1) throw new Error('dup pair');\n"
            "if (m.dedupePreserve([]).length !== 0) throw new Error('empty');\n"),
        "_fixed": {"utils.js":
            "function dedupePreserve(items) { return [...new Set(items)]; }\n"
            "module.exports = { dedupePreserve };\n"},
    })
    tasks.append({
        "id": "g2-regex-dotall",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports extractCodeBlocks(s) which returns an array of the CONTENT of every "
                  "```fenced block``` in a string, including blocks spanning multiple lines. It currently "
                  "uses /```(.*?)```/g — the dot cannot match across newlines, so multi-line blocks come "
                  "back missing (or empty). Fix the regex to match across newlines (e.g. "
                  "/```([\\s\\S]*?)```/g).",
        "setup": lambda d: write(d, {"utils.js":
            "function extractCodeBlocks(s) { const r = []; for (const m of s.matchAll(/```(.*?)```/g)) r.push(m[1]); return r; }  // BUG: dot can't match newlines\n"
            "module.exports = { extractCodeBlocks };\n"}),
        "test": lambda d: rt(d, "utils",
            "const r = m.extractCodeBlocks('a\\n```\\nline1\\nline2\\n```\\nb');\n"
            "if (r.length !== 1) throw new Error('count ' + r.length);\n"
            "if (!r[0].includes('line1') || !r[0].includes('line2')) throw new Error('got ' + JSON.stringify(r));\n"
            "const r2 = m.extractCodeBlocks('x ```ok``` y');\n"
            "if (r2.length !== 1 || r2[0] !== 'ok') throw new Error('single ' + JSON.stringify(r2));\n"
            "const r3 = m.extractCodeBlocks('```a``` mid ```b```');\n"
            "if (r3.length !== 2 || r3[0] !== 'a' || r3[1] !== 'b') throw new Error('two blocks ' + JSON.stringify(r3));\n"
            "const r4 = m.extractCodeBlocks('no fences');\n"
            "if (r4.length !== 0) throw new Error('none ' + JSON.stringify(r4));\n"),
        "_fixed": {"utils.js":
            "function extractCodeBlocks(s) { const r = []; for (const m of s.matchAll(/```([\\s\\S]*?)```/g)) r.push(m[1]); return r; }\n"
            "module.exports = { extractCodeBlocks };\n"},
    })
    tasks.append({
        "id": "g2-string-codes",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports codePoints(s) which returns an array of Unicode code points "
                  "(codePoints('a\\u{1F600}') === [97, 0x1F600]). It currently uses charCodeAt per index, "
                  "which splits surrogate pairs — emoji come back as two values. Fix it to use codePointAt "
                  "and skip the low surrogate.",
        "setup": lambda d: write(d, {"utils.js":
            "function codePoints(s) { const out = []; for (let i = 0; i < s.length; i++) out.push(s.charCodeAt(i)); return out; }  // BUG: splits surrogates\n"
            "module.exports = { codePoints };\n"}),
        "test": lambda d: rt(d, "utils",
            "const r = m.codePoints('a\\u{1F600}');\n"
            "if (r.length !== 2) throw new Error('len ' + r.length);\n"
            "if (r[0] !== 97 || r[1] !== 0x1F600) throw new Error('got ' + r.join(','));\n"
            "if (m.codePoints('abc').length !== 3) throw new Error('plain');\n"
            "if (m.codePoints('\\u{1F600}a').length !== 2) throw new Error('emoji-first ' + m.codePoints('\\u{1F600}a').length);\n"
            "if (m.codePoints('\\u{1F600}\\u{1F600}').join(',') !== '128512,128512') throw new Error('two emoji ' + m.codePoints('\\u{1F600}\\u{1F600}').join(','));\n"),
        "_fixed": {"utils.js":
            "function codePoints(s) {\n"
            "  const out = [];\n"
            "  for (let i = 0; i < s.length; i++) {\n"
            "    out.push(s.codePointAt(i));\n"
            "    const c = s.charCodeAt(i);\n"
            "    if (c >= 0xD800 && c <= 0xDBFF) i++;\n"
            "  }\n"
            "  return out;\n"
            "}\n"
            "module.exports = { codePoints };\n"},
    })
    tasks.append({
        "id": "g2-promise-settle",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports settleAll(promises) which must resolve with an array of "
                  "{ok, value|error} for EVERY input promise, regardless of rejection "
                  "(settleAll([Promise.resolve(1), Promise.reject('e')]) resolves to "
                  "[{ok:true,value:1},{ok:false,error:'e'}]). It currently uses Promise.all, which "
                  "rejects if ANY input rejects. Implement allSettled-style behavior.",
        "setup": lambda d: write(d, {"utils.js":
            "async function settleAll(promises) { return Promise.all(promises.map(p => Promise.resolve(p))); }  // BUG: rejects on first failure\n"
            "module.exports = { settleAll };\n"}),
        "test": lambda d: rat(d, "utils",
            "    const r = await m.settleAll([Promise.resolve(1), Promise.reject('e')]);\n"
            "    if (r.length !== 2 || r[0].ok !== true || r[0].value !== 1 || r[1].ok !== false) throw new Error('expected [{ok:true,value:1},{ok:false,error:\"e\"}], got ' + JSON.stringify(r));\n"
            "    const r2 = await m.settleAll([Promise.resolve('x')]);\n"
            "    if (r2[0].value !== 'x') throw new Error('single');\n"),
        "_fixed": {"utils.js":
            "async function settleAll(promises) {\n"
            "  const out = [];\n"
            "  for (const p of promises) {\n"
            "    try { out.push({ ok: true, value: await p }); }\n"
            "    catch (e) { out.push({ ok: false, error: e }); }\n"
            "  }\n"
            "  return out;\n"
            "}\n"
            "module.exports = { settleAll };\n"},
    })
    tasks.append({
        "id": "g2-timeout-order",
        "category": "gen2-bugfix",
        "prompt": "utils.js exports runOrdered(fns) — an async function taking an array of async "
                  "functions; it must run them SEQUENTIALLY in order and resolve with their results in "
                  "that order. It currently starts all of them concurrently with Promise.all, so ordering "
                  "is lost. Fix it to await each function before starting the next.",
        "setup": lambda d: write(d, {"utils.js":
            "async function runOrdered(fns) { return Promise.all(fns.map(f => f())); }  // BUG: concurrent\n"
            "module.exports = { runOrdered };\n"}),
        "test": lambda d: rat(d, "utils",
            "    const order = [];\n"
            "    const mk = (n, ms) => async () => { await new Promise(r => setTimeout(r, ms)); order.push(n); return n; };\n"
            "    const r = await m.runOrdered([mk(1, 40), mk(2, 10), mk(3, 0)]);\n"
            "    if (order.join(',') !== '1,2,3') throw new Error('order ' + order.join(','));\n"
            "    if (r.join(',') !== '1,2,3') throw new Error('res ' + r.join(','));\n"),
        "_fixed": {"utils.js":
            "async function runOrdered(fns) { const out = []; for (const f of fns) out.push(await f()); return out; }\n"
            "module.exports = { runOrdered };\n"},
    })

    # ══════════════════════════════════════════════════════════════════════
    # gen2-refactor — behavior-preserving, hidden STRUCTURE checks
    # ══════════════════════════════════════════════════════════════════════
    def refac(entry, body, structure):
        return lambda d: ref(d, entry, body, structure)

    tasks.append({
        "id": "g2r-extract-helper",
        "category": "gen2-refactor",
        "prompt": "utils.js has validateEmail and checkEmail with IDENTICAL bodies. Refactor: extract the "
                  "shared logic into a private helper function isEmailOk(email) and have both functions "
                  "call it. Behavior must not change — the same inputs return the same outputs.",
        "setup": lambda d: write(d, {"utils.js":
            "function validateEmail(email) { return typeof email === 'string' && email.includes('@') ? 'ok' : 'invalid email'; }\n"
            "function checkEmail(email) { return typeof email === 'string' && email.includes('@') ? 'ok' : 'invalid email'; }\n"
            "module.exports = { validateEmail, checkEmail };\n"}),
        "test": refac("utils",
            "    if (m.validateEmail('a@b.com') !== 'ok') throw new Error('valid failed');\n"
            "    if (m.checkEmail('a@b.com') !== 'ok') throw new Error('check valid failed');\n"
            "    if (m.validateEmail('nope') !== 'invalid email') throw new Error('invalid failed');\n"
            "    if (m.checkEmail(42) !== 'invalid email') throw new Error('type failed');\n",
            [("has_text", "utils.js", "isEmailOk")]),
        "_fixed": {"utils.js":
            "function isEmailOk(email) { return typeof email === 'string' && email.includes('@'); }\n"
            "function validateEmail(email) { return isEmailOk(email) ? 'ok' : 'invalid email'; }\n"
            "function checkEmail(email) { return isEmailOk(email) ? 'ok' : 'invalid email'; }\n"
            "module.exports = { validateEmail, checkEmail };\n"},
    })
    tasks.append({
        "id": "g2r-rename-export",
        "category": "gen2-refactor",
        "prompt": "utils.js exports a function named compute(). Refactor: rename the export AND all "
                  "internal references to calculate(). The old name must be gone. Behavior must not "
                  "change.",
        "setup": lambda d: write(d, {"utils.js":
            "function compute(items) { return items.map(x => x * 2).filter(x => x > 3); }\n"
            "module.exports = { compute };\n"}),
        "test": refac("utils",
            "    if (m.calculate([1,2,3,4]).join(',') !== '4,6,8') throw new Error('got ' + JSON.stringify(m.calculate([1,2,3,4])));\n"
            "    if (m.calculate([]).length !== 0) throw new Error('empty');\n",
            [("has_export", "utils.js", "calculate"), ("lacks_export", "utils.js", "compute")]),
        "_fixed": {"utils.js":
            "function calculate(items) { return items.map(x => x * 2).filter(x => x > 3); }\n"
            "module.exports = { calculate };\n"},
    })
    tasks.append({
        "id": "g2r-split-module",
        "category": "gen2-refactor",
        "prompt": "utils.js mixes string formatting and math helpers. Refactor: split it into two new "
                  "files — format.js (formatName, formatDate) and math.js (average, total) — then have "
                  "utils.js RE-EXPORT everything from those two files so existing callers of utils.js "
                  "keep working. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function formatName(name) { return name.trim().charAt(0).toUpperCase() + name.trim().slice(1); }\n"
            "function formatDate(d) { return d.toISOString().slice(0, 10); }\n"
            "function average(nums) { return nums.length ? nums.reduce((a, b) => a + b, 0) / nums.length : 0; }\n"
            "function total(nums) { return nums.reduce((a, b) => a + b, 0); }\n"
            "module.exports = { formatName, formatDate, average, total };\n"}),
        "test": refac("utils",
            "    if (m.formatName('  bob ') !== 'Bob') throw new Error('formatName');\n"
            "    if (m.formatDate(new Date('2026-01-05T00:00:00Z')) !== '2026-01-05') throw new Error('formatDate');\n"
            "    if (m.average([1,2,3]) !== 2) throw new Error('avg');\n"
            "    if (m.average([]) !== 0) throw new Error('avg empty');\n"
            "    if (m.total([1,2,3]) !== 6) throw new Error('total');\n",
            [("file_exists", "format.js"), ("file_exists", "math.js"),
             ("has_text", "utils.js", "require('./format')"), ("has_text", "utils.js", "require('./math')")]),
        "_fixed": {"format.js":
            "function formatName(name) { return name.trim().charAt(0).toUpperCase() + name.trim().slice(1); }\n"
            "function formatDate(d) { return d.toISOString().slice(0, 10); }\n"
            "module.exports = { formatName, formatDate };\n",
            "math.js":
            "function average(nums) { return nums.length ? nums.reduce((a, b) => a + b, 0) / nums.length : 0; }\n"
            "function total(nums) { return nums.reduce((a, b) => a + b, 0); }\n"
            "module.exports = { average, total };\n",
            "utils.js":
            "module.exports = { ...require('./format'), ...require('./math') };\n"},
    })
    tasks.append({
        "id": "g2r-loop-map",
        "category": "gen2-refactor",
        "prompt": "utils.js implements doubleAll() with an explicit for-loop. Refactor: replace the loop "
                  "with Array.prototype.map. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function doubleAll(items) { const out = []; for (let i = 0; i < items.length; i++) out.push(items[i] * 2); return out; }\n"
            "module.exports = { doubleAll };\n"}),
        "test": refac("utils",
            "    if (m.doubleAll([1,2,3]).join(',') !== '2,4,6') throw new Error('got ' + JSON.stringify(m.doubleAll([1,2,3])));\n"
            "    if (m.doubleAll([]).length !== 0) throw new Error('empty');\n",
            [("lacks_text", "utils.js", "for ("), ("has_text", "utils.js", ".map(")]),
        "_fixed": {"utils.js":
            "function doubleAll(items) { return items.map(x => x * 2); }\n"
            "module.exports = { doubleAll };\n"},
    })
    tasks.append({
        "id": "g2r-const-config",
        "category": "gen2-refactor",
        "prompt": "utils.js has two magic numbers: 30000 and 3. Refactor: define named constants "
                  "TIMEOUT_MS = 30000 and MAX_RETRIES = 3 at the top of the file, and have the functions "
                  "return those constants. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function getTimeout() { return 30000; }\n"
            "function getRetries() { return 3; }\n"
            "module.exports = { getTimeout, getRetries };\n"}),
        "test": refac("utils",
            "    if (m.getTimeout() !== 30000) throw new Error('timeout');\n"
            "    if (m.getRetries() !== 3) throw new Error('retries');\n",
            [("has_text", "utils.js", "TIMEOUT_MS"), ("has_text", "utils.js", "MAX_RETRIES"),
             ("lacks_text", "utils.js", "return 30000"), ("lacks_text", "utils.js", "return 3")]),
        "_fixed": {"utils.js":
            "const TIMEOUT_MS = 30000;\n"
            "const MAX_RETRIES = 3;\n"
            "function getTimeout() { return TIMEOUT_MS; }\n"
            "function getRetries() { return MAX_RETRIES; }\n"
            "module.exports = { getTimeout, getRetries };\n"},
    })
    tasks.append({
        "id": "g2r-dedup-branch",
        "category": "gen2-refactor",
        "prompt": "utils.js has two functions with the SAME try/catch JSON parsing block. Refactor: "
                  "extract a shared helper parseJsonSafe(json) that returns the parsed value or "
                  "{ error: 'invalid json' } on failure, and use it in both functions. Behavior must not "
                  "change.",
        "setup": lambda d: write(d, {"utils.js":
            "function parseUser(json) {\n"
            "  try { const v = JSON.parse(json); if (!v || typeof v.name !== 'string') return { error: 'invalid user' }; return { name: v.name }; }\n"
            "  catch (e) { return { error: 'invalid json' }; }\n"
            "}\n"
            "function parseOrder(json) {\n"
            "  try { const v = JSON.parse(json); if (!Array.isArray(v)) return { error: 'invalid order' }; return { items: v }; }\n"
            "  catch (e) { return { error: 'invalid json' }; }\n"
            "}\n"
            "module.exports = { parseUser, parseOrder };\n"}),
        "test": refac("utils",
            "    if (JSON.stringify(m.parseUser('{\"name\":\"a\"}')) !== '{\"name\":\"a\"}') throw new Error('user ok');\n"
            "    if (JSON.stringify(m.parseUser('x')) !== '{\"error\":\"invalid json\"}') throw new Error('user bad');\n"
            "    if (JSON.stringify(m.parseOrder('[1,2]')) !== '{\"items\":[1,2]}') throw new Error('order ok');\n"
            "    if (JSON.stringify(m.parseOrder('x')) !== '{\"error\":\"invalid json\"}') throw new Error('order bad');\n",
            [("has_text", "utils.js", "parseJsonSafe")]),
        "_fixed": {"utils.js":
            "function parseJsonSafe(json) { try { return JSON.parse(json); } catch (e) { return { error: 'invalid json' }; } }\n"
            "function parseUser(json) { const v = parseJsonSafe(json); if (v.error) return v; if (typeof v.name !== 'string') return { error: 'invalid user' }; return { name: v.name }; }\n"
            "function parseOrder(json) { const v = parseJsonSafe(json); if (v.error) return v; if (!Array.isArray(v)) return { error: 'invalid order' }; return { items: v }; }\n"
            "module.exports = { parseUser, parseOrder };\n"},
    })
    tasks.append({
        "id": "g2r-extract-pure",
        "category": "gen2-refactor",
        "prompt": "utils.js computes sum and average inline inside getStats(). Refactor: extract two pure "
                  "helpers — sum(nums) and avg(nums) — and have getStats() use them. Both helpers must be "
                  "exported too. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function getStats(nums) { const sum = nums.reduce((a, b) => a + b, 0); const avg = nums.length ? sum / nums.length : 0; return { sum, avg }; }\n"
            "module.exports = { getStats };\n"}),
        "test": refac("utils",
            "    const s = m.getStats([1,2,3]);\n"
            "    if (s.sum !== 6 || s.avg !== 2) throw new Error('stats ' + JSON.stringify(s));\n"
            "    if (m.getStats([]).avg !== 0) throw new Error('empty');\n"
            "    if (m.sum([4,4]) !== 8) throw new Error('sum export');\n"
            "    if (m.avg([2,4]) !== 3) throw new Error('avg export');\n",
            [("has_export", "utils.js", "sum"), ("has_export", "utils.js", "avg")]),
        "_fixed": {"utils.js":
            "function sum(nums) { return nums.reduce((a, b) => a + b, 0); }\n"
            "function avg(nums) { return nums.length ? sum(nums) / nums.length : 0; }\n"
            "function getStats(nums) { return { sum: sum(nums), avg: avg(nums) }; }\n"
            "module.exports = { getStats, sum, avg };\n"},
    })
    tasks.append({
        "id": "g2r-merge-modules",
        "category": "gen2-refactor",
        "prompt": "The project has two tiny modules a.js and b.js. Refactor: merge both into a single "
                  "utils.js exporting { a: 1, b: 2 }, and DELETE a.js and b.js (callers will require "
                  "'./utils').",
        "setup": lambda d: write(d, {"a.js": "module.exports = { a: 1 };\n",
                                     "b.js": "module.exports = { b: 2 };\n",
                                     "utils.js": "module.exports = { ...require('./a'), ...require('./b') };\n"}),
        "test": refac("utils",
            "    if (m.a !== 1 || m.b !== 2) throw new Error('merge failed ' + JSON.stringify(m));\n",
            [("file_absent", "a.js"), ("file_absent", "b.js"), ("file_exists", "utils.js")]),
        "_fixed": {"utils.js": "module.exports = { a: 1, b: 2 };\n"},
        "_remove": ["a.js", "b.js"],
    })
    tasks.append({
        "id": "g2r-param-defaults",
        "category": "gen2-refactor",
        "prompt": "utils.js exports paginate(items, page, size). Refactor: add default parameters "
                  "page = 1 and size = 10 so paginate(items) returns the first 10 items. Behavior for "
                  "explicit args must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function paginate(items, page, size) { const start = (page - 1) * size; return items.slice(start, start + size); }\n"
            "module.exports = { paginate };\n"}),
        "test": refac("utils",
            "    const items = [1,2,3,4,5,6,7,8,9,10,11,12];\n"
            "    const p = m.paginate(items);\n"
            "    if (p.length !== 10 || p[0] !== 1) throw new Error('defaults ' + p.length + ' ' + p[0]);\n"
            "    if (m.paginate(items, 2, 5)[0] !== 6) throw new Error('explicit');\n",
            [("has_text", "utils.js", "page = 1"), ("has_text", "utils.js", "size = 10")]),
        "_fixed": {"utils.js":
            "function paginate(items, page = 1, size = 10) { const start = (page - 1) * size; return items.slice(start, start + size); }\n"
            "module.exports = { paginate };\n"},
    })
    tasks.append({
        "id": "g2r-template-literal",
        "category": "gen2-refactor",
        "prompt": "utils.js builds a greeting with string concatenation. Refactor: use a template literal "
                  "instead. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function greet(name, age) { return 'Hello ' + name + ', you are ' + age + ' years old.'; }\n"
            "module.exports = { greet };\n"}),
        "test": refac("utils",
            "    if (m.greet('Bob', 30) !== 'Hello Bob, you are 30 years old.') throw new Error('got ' + m.greet('Bob', 30));\n",
            [("has_text", "utils.js", "${")]),
        "_fixed": {"utils.js":
            "function greet(name, age) { return `Hello ${name}, you are ${age} years old.`; }\n"
            "module.exports = { greet };\n"},
    })
    tasks.append({
        "id": "g2r-constant-hoist",
        "category": "gen2-refactor",
        "prompt": "utils.js repeats the literal URL 'https://api.example.com/v1' in three functions. "
                  "Refactor: hoist it to a named constant BASE_URL at the top of the file and use it "
                  "everywhere. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function fetchUser(id) { return 'https://api.example.com/v1/users/' + id; }\n"
            "function fetchPosts(userId) { return 'https://api.example.com/v1/posts?user=' + userId; }\n"
            "function healthUrl() { return 'https://api.example.com/v1/health'; }\n"
            "module.exports = { fetchUser, fetchPosts, healthUrl };\n"}),
        "test": refac("utils",
            "    if (m.fetchUser(7) !== 'https://api.example.com/v1/users/7') throw new Error('user');\n"
            "    if (m.fetchPosts(3) !== 'https://api.example.com/v1/posts?user=3') throw new Error('posts');\n"
            "    if (m.healthUrl() !== 'https://api.example.com/v1/health') throw new Error('health');\n",
            [("has_text", "utils.js", "BASE_URL"), ("lacks_text", "utils.js", "https://api.example.com/v1/")]),
        "_fixed": {"utils.js":
            "const BASE_URL = 'https://api.example.com/v1';\n"
            "function fetchUser(id) { return BASE_URL + '/users/' + id; }\n"
            "function fetchPosts(userId) { return BASE_URL + '/posts?user=' + userId; }\n"
            "function healthUrl() { return BASE_URL + '/health'; }\n"
            "module.exports = { fetchUser, fetchPosts, healthUrl };\n"},
    })
    tasks.append({
        "id": "g2r-early-return",
        "category": "gen2-refactor",
        "prompt": "utils.js implements isAllowed() with deeply nested ifs. Refactor: flatten it using "
                  "guard clauses / early returns (return false for each failing condition, then return "
                  "true). Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function isAllowed(user) { if (user) { if (user.active) { if (user.plan) { return true; } } } return false; }\n"
            "module.exports = { isAllowed };\n"}),
        "test": refac("utils",
            "    if (m.isAllowed({ active: true, plan: 'x' }) !== true) throw new Error('allowed');\n"
            "    if (m.isAllowed(null) !== false) throw new Error('null');\n"
            "    if (m.isAllowed({ active: false, plan: 'x' }) !== false) throw new Error('inactive');\n"
            "    if (m.isAllowed({ active: true, plan: null }) !== false) throw new Error('noplan');\n",
            [("has_text", "utils.js", "if (!user)"), ("lacks_text", "utils.js", "if (user.active)")]),
        "_fixed": {"utils.js":
            "function isAllowed(user) {\n"
            "  if (!user) return false;\n"
            "  if (!user.active) return false;\n"
            "  if (!user.plan) return false;\n"
            "  return true;\n"
            "}\n"
            "module.exports = { isAllowed };\n"},
    })
    tasks.append({
        "id": "g2r-rename-param",
        "category": "gen2-refactor",
        "prompt": "utils.js exports countOccurrences() whose parameter is named d. Refactor: rename the "
                  "parameter to items and update the function body accordingly. Behavior must not "
                  "change.",
        "setup": lambda d: write(d, {"utils.js":
            "function countOccurrences(d, target) { let n = 0; for (const v of d) { if (v === target) n++; } return n; }\n"
            "module.exports = { countOccurrences };\n"}),
        "test": refac("utils",
            "    if (m.countOccurrences([1,2,1,3], 1) !== 2) throw new Error('got ' + m.countOccurrences([1,2,1,3], 1));\n"
            "    if (m.countOccurrences([], 9) !== 0) throw new Error('empty');\n",
            [("has_text", "utils.js", "countOccurrences(items"), ("lacks_text", "utils.js", "countOccurrences(d")]),
        "_fixed": {"utils.js":
            "function countOccurrences(items, target) { let n = 0; for (const v of items) { if (v === target) n++; } return n; }\n"
            "module.exports = { countOccurrences };\n"},
    })
    tasks.append({
        "id": "g2r-switch-map",
        "category": "gen2-refactor",
        "prompt": "utils.js implements getLabel() with a switch statement. Refactor: replace the switch "
                  "with a lookup object. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function getLabel(code) { switch (code) { case 1: return 'one'; case 2: return 'two'; default: return 'other'; } }\n"
            "module.exports = { getLabel };\n"}),
        "test": refac("utils",
            "    if (m.getLabel(1) !== 'one') throw new Error('one');\n"
            "    if (m.getLabel(2) !== 'two') throw new Error('two');\n"
            "    if (m.getLabel(9) !== 'other') throw new Error('other');\n",
            [("lacks_text", "utils.js", "switch")]),
        "_fixed": {"utils.js":
            "const LABELS = { 1: 'one', 2: 'two' };\n"
            "function getLabel(code) { return LABELS[code] || 'other'; }\n"
            "module.exports = { getLabel };\n"},
    })
    tasks.append({
        "id": "g2r-object-shorthand",
        "category": "gen2-refactor",
        "prompt": "utils.js builds an object with verbose key:value pairs. Refactor: use ES6 object "
                  "property shorthand. Behavior must not change.",
        "setup": lambda d: write(d, {"utils.js":
            "function buildUser(name, age) { return { name: name, age: age, active: true }; }\n"
            "module.exports = { buildUser };\n"}),
        "test": refac("utils",
            "    if (JSON.stringify(m.buildUser('a', 1)) !== '{\"name\":\"a\",\"age\":1,\"active\":true}') throw new Error('got ' + JSON.stringify(m.buildUser('a', 1)));\n",
            [("lacks_text", "utils.js", "name: name"), ("lacks_text", "utils.js", "age: age")]),
        "_fixed": {"utils.js":
            "function buildUser(name, age) { return { name, age, active: true }; }\n"
            "module.exports = { buildUser };\n"},
    })

    # ══════════════════════════════════════════════════════════════════════
    # gen2-python — stdlib-only Python bugs
    # ══════════════════════════════════════════════════════════════════════
    tasks.append({
        "id": "g2p-mutable-default",
        "category": "gen2-python",
        "prompt": "utils.py exports append_item(item, lst=[]). It currently uses a mutable default "
                  "argument, so items accumulate across calls (append_item(1) then append_item(2) gives "
                  "[1, 2]). Fix it so each call without lst starts with a fresh empty list.",
        "setup": lambda d: write(d, {"utils.py":
            "def append_item(item, lst=[]):\n"
            "    lst.append(item)\n"
            "    return lst\n"}),
        "test": lambda d: py(d, "utils",
            "r1 = m.append_item(1)\n"
            "r2 = m.append_item(2)\n"
            "assert r1 == [1], r1\n"
            "assert r2 == [2], r2\n"),
        "_fixed": {"utils.py":
            "def append_item(item, lst=None):\n"
            "    if lst is None:\n"
            "        lst = []\n"
            "    lst.append(item)\n"
            "    return lst\n"},
    })
    tasks.append({
        "id": "g2p-closure-late",
        "category": "gen2-python",
        "prompt": "utils.py exports make_multipliers(n) returning a list of n functions where the k-th "
                  "function multiplies by k (make_multipliers(3)[0](2) === 0, [1](2) === 2, [2](2) === 4). "
                  "It currently suffers from late-binding closure — all functions multiply by n-1. Fix it "
                  "(e.g. with default arguments).",
        "setup": lambda d: write(d, {"utils.py":
            "def make_multipliers(n):\n"
            "    return [lambda x: x * i for i in range(n)]\n"}),
        "test": lambda d: py(d, "utils",
            "t = m.make_multipliers(3)\n"
            "assert t[0](2) == 0, t[0](2)\n"
            "assert t[1](2) == 2, t[1](2)\n"
            "assert t[2](2) == 4, t[2](2)\n"),
        "_fixed": {"utils.py":
            "def make_multipliers(n):\n"
            "    return [lambda x, i=i: x * i for i in range(n)]\n"},
    })
    tasks.append({
        "id": "g2p-list-copy",
        "category": "gen2-python",
        "prompt": "utils.py exports get_sorted(arr) which must return a sorted COPY without modifying the "
                  "input. It currently sorts in place with arr.sort(). Fix it so the original list is "
                  "unchanged.",
        "setup": lambda d: write(d, {"utils.py":
            "def get_sorted(arr):\n"
            "    arr.sort()\n"
            "    return arr\n"}),
        "test": lambda d: py(d, "utils",
            "orig = [3, 1, 2]\n"
            "s = m.get_sorted(orig)\n"
            "assert s == [1, 2, 3], s\n"
            "assert orig == [3, 1, 2], orig\n"),
        "_fixed": {"utils.py":
            "def get_sorted(arr):\n"
            "    return sorted(arr)\n"},
    })
    tasks.append({
        "id": "g2p-dict-default",
        "category": "gen2-python",
        "prompt": "utils.py exports add_to_counts(counts, key) which must increment the count for key, "
                  "starting from 0 for missing keys. It currently crashes with KeyError when the key is "
                  "not present (counts[key] += 1). Fix it to use .get() with a default.",
        "setup": lambda d: write(d, {"utils.py":
            "def add_to_counts(counts, key):\n"
            "    counts[key] += 1\n"
            "    return counts\n"}),
        "test": lambda d: py(d, "utils",
            "c = {}\n"
            "m.add_to_counts(c, 'a')\n"
            "m.add_to_counts(c, 'a')\n"
            "assert c == {'a': 2}, c\n"),
        "_fixed": {"utils.py":
            "def add_to_counts(counts, key):\n"
            "    counts[key] = counts.get(key, 0) + 1\n"
            "    return counts\n"},
    })
    tasks.append({
        "id": "g2p-generator-next",
        "category": "gen2-python",
        "prompt": "utils.py exports safe_next(gen, default) which must return the next item from a "
                  "generator, or default when the generator is exhausted. It currently calls next(gen) "
                  "directly and raises StopIteration on empty generators. Fix it to catch StopIteration.",
        "setup": lambda d: write(d, {"utils.py":
            "def safe_next(gen, default):\n"
            "    return next(gen)\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.safe_next(iter([1]), 'x') == 1\n"
            "assert m.safe_next(iter([]), 'x') == 'x'\n"),
        "_fixed": {"utils.py":
            "def safe_next(gen, default):\n"
            "    try:\n"
            "        return next(gen)\n"
            "    except StopIteration:\n"
            "        return default\n"},
    })
    tasks.append({
        "id": "g2p-float-round",
        "category": "gen2-python",
        "prompt": "utils.py exports round_money(x) which must round half UP to 2 decimals using "
                  "Decimal with ROUND_HALF_UP (round_money(2.5) === 2.5, round_money(1.005) === 1.01). "
                  "It currently uses round(x, 2), which does banker's rounding and suffers float "
                  "imprecision. Fix it with the decimal module (stdlib only).",
        "setup": lambda d: write(d, {"utils.py":
            "def round_money(x):\n"
            "    return round(x, 2)\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.round_money(2.5) == 2.5, m.round_money(2.5)\n"
            "assert m.round_money(1.005) == 1.01, m.round_money(1.005)\n"
            "assert m.round_money(3.14159) == 3.14, m.round_money(3.14159)\n"),
        "_fixed": {"utils.py":
            "from decimal import Decimal, ROUND_HALF_UP\n"
            "def round_money(x):\n"
            "    return float(Decimal(str(x)).quantize(Decimal('0.01'), rounding=ROUND_HALF_UP))\n"},
    })
    tasks.append({
        "id": "g2p-parse-int",
        "category": "gen2-python",
        "prompt": "utils.py exports parse_int(s) which must return the int value of a numeric string, or "
                  "-1 for non-numeric input (parse_int('42') === 42, parse_int('abc') === -1). It "
                  "currently calls int(s) directly and raises ValueError. Fix it with try/except.",
        "setup": lambda d: write(d, {"utils.py":
            "def parse_int(s):\n"
            "    return int(s)\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.parse_int('42') == 42\n"
            "assert m.parse_int('abc') == -1\n"
            "assert m.parse_int('-7') == -7\n"
            "assert m.parse_int('') == -1\n"
            "assert m.parse_int('12.5') == -1\n"),
        "_fixed": {"utils.py":
            "def parse_int(s):\n"
            "    try:\n"
            "        return int(s)\n"
            "    except ValueError:\n"
            "        return -1\n"},
    })
    tasks.append({
        "id": "g2p-html-escape",
        "category": "gen2-python",
        "prompt": "utils.py exports escape_html(s) which must escape all five characters &, <, >, \" and "
                  "' (escape_html('<a href=\"x\">&') === '&lt;a href=&quot;x&quot;&gt;&amp;'). It "
                  "currently only escapes < and >, leaving & and quotes — an XSS hole. Fix it to escape "
                  "all five (escape & FIRST).",
        "setup": lambda d: write(d, {"utils.py":
            "def escape_html(s):\n"
            "    return s.replace('<', '&lt;').replace('>', '&gt;')\n"}),
        "test": lambda d: py(d, "utils",
            "r = m.escape_html('<a href=\"x\">&')\n"
            "assert r == '&lt;a href=&quot;x&quot;&gt;&amp;', 'expected &lt;a href=&quot;x&quot;&gt;&amp;, got ' + r\n"
            "assert m.escape_html(\"it's\") == 'it&#39;s', 'expected it&#39;s, got ' + m.escape_html(\"it's\")\n"
            "assert m.escape_html('a&b<c>') == 'a&amp;b&lt;c&gt;', 'expected a&amp;b&lt;c&gt;, got ' + m.escape_html('a&b<c>')\n"
            "assert m.escape_html('&lt;') == '&amp;lt;', 'expected &amp;lt;, got ' + m.escape_html('&lt;')\n"
            "assert m.escape_html('') == ''\n"),
        "_fixed": {"utils.py":
            "def escape_html(s):\n"
            "    return (s.replace('&', '&amp;')\n"
            "            .replace('<', '&lt;')\n"
            "            .replace('>', '&gt;')\n"
            "            .replace('\"', '&quot;')\n"
            "            .replace(\"'\", '&#39;'))\n"},
    })
    tasks.append({
        "id": "g2p-scope-global",
        "category": "gen2-python",
        "prompt": "utils.py exports next_id() which must return 1, then 2, then 3 on successive calls, "
                  "using a module-level counter. It currently raises UnboundLocalError because the "
                  "increment assigns to count without declaring it global. Fix it.",
        "setup": lambda d: write(d, {"utils.py":
            "count = 0\n"
            "def next_id():\n"
            "    count += 1\n"
            "    return count\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.next_id() == 1, m.next_id()\n"
            "assert m.next_id() == 2, m.next_id()\n"
            "assert m.next_id() == 3, m.next_id()\n"
            "assert m.next_id() == 4, m.next_id()\n"
            "assert m.next_id() == 5, m.next_id()\n"),
        "_fixed": {"utils.py":
            "count = 0\n"
            "def next_id():\n"
            "    global count\n"
            "    count += 1\n"
            "    return count\n"},
    })
    tasks.append({
        "id": "g2p-exc-swallow",
        "category": "gen2-python",
        "prompt": "utils.py exports fetch_value(d, key) which must return d[key], and return None ONLY "
                  "for a missing key (KeyError). It currently swallows ALL exceptions with except "
                  "Exception and returns None — hiding real bugs. Fix it to only catch KeyError.",
        "setup": lambda d: write(d, {"utils.py":
            "def fetch_value(d, key):\n"
            "    try:\n"
            "        return d[key]\n"
            "    except Exception:\n"
            "        return None\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.fetch_value({'a': 1}, 'a') == 1\n"
            "assert m.fetch_value({}, 'missing') is None\n"
            "try:\n"
            "    m.fetch_value({}, [])\n"
            "    raise AssertionError('should have raised TypeError')\n"
            "except TypeError:\n"
            "    pass\n"
            "try:\n"
            "    m.fetch_value(None, 'a')\n"
            "    raise AssertionError('None dict should raise TypeError, got swallowed')\n"
            "except TypeError:\n"
            "    pass\n"
            "assert m.fetch_value({'x': None}, 'x') is None, 'stored None value'\n"),
        "_fixed": {"utils.py":
            "def fetch_value(d, key):\n"
            "    try:\n"
            "        return d[key]\n"
            "    except KeyError:\n"
            "        return None\n"},
    })
    tasks.append({
        "id": "g2p-flatten",
        "category": "gen2-python",
        "prompt": "utils.py exports flatten(lst) which must recursively flatten nested lists "
                  "(flatten([1, [2, [3]]]) === [1, 2, 3]). It currently returns the list unchanged. Fix "
                  "it to recursively flatten nested lists.",
        "setup": lambda d: write(d, {"utils.py":
            "def flatten(lst):\n"
            "    return lst\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.flatten([1, [2, [3]]]) == [1, 2, 3], m.flatten([1, [2, [3]]])\n"
            "assert m.flatten([]) == []\n"
            "assert m.flatten([1, 2]) == [1, 2]\n"
            "assert m.flatten([[1], [[2, [3]]], 4]) == [1, 2, 3, 4], m.flatten([[1], [[2, [3]]], 4])\n"
            "assert m.flatten([[[1]]]) == [1], m.flatten([[[1]]])\n"
            "assert m.flatten([1, [2, [3, [4]]]]) == [1, 2, 3, 4], m.flatten([1, [2, [3, [4]]]])\n"
            "assert m.flatten([[[]]]) == []\n"),
        "_fixed": {"utils.py":
            "def flatten(lst):\n"
            "    out = []\n"
            "    for x in lst:\n"
            "        if isinstance(x, list):\n"
            "            out.extend(flatten(x))\n"
            "        else:\n"
            "            out.append(x)\n"
            "    return out\n"},
    })
    tasks.append({
        "id": "g2p-tail-mutates",
        "category": "gen2-python",
        "prompt": "utils.py exports tail(lst) which must return a copy of the list without its first "
                  "element, leaving the input unchanged. It currently uses lst.pop(0), which mutates the "
                  "input. Fix it to use slicing.",
        "setup": lambda d: write(d, {"utils.py":
            "def tail(lst):\n"
            "    lst.pop(0)\n"
            "    return lst\n"}),
        "test": lambda d: py(d, "utils",
            "orig = [1, 2, 3]\n"
            "assert m.tail(orig) == [2, 3]\n"
            "assert orig == [1, 2, 3], orig\n"),
        "_fixed": {"utils.py":
            "def tail(lst):\n"
            "    return lst[1:]\n"},
    })
    tasks.append({
        "id": "g2p-sort-ci",
        "category": "gen2-python",
        "prompt": "utils.py exports sort_ci(items) which must sort strings case-insensitively "
                  "(sort_ci(['b', 'a', 'C']) === ['a', 'b', 'C']). It currently sorts case-sensitively. "
                  "Fix it to sort with key=str.lower.",
        "setup": lambda d: write(d, {"utils.py":
            "def sort_ci(items):\n"
            "    return sorted(items)\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.sort_ci(['b', 'a', 'C']) == ['a', 'b', 'C'], m.sort_ci(['b', 'a', 'C'])\n"
            "assert m.sort_ci(['Banana', 'apple', 'Cherry']) == ['apple', 'Banana', 'Cherry'], m.sort_ci(['Banana', 'apple', 'Cherry'])\n"
            "assert m.sort_ci(['a', 'A', 'b']) == ['a', 'A', 'b'], m.sort_ci(['a', 'A', 'b'])\n"),
        "_fixed": {"utils.py":
            "def sort_ci(items):\n"
            "    return sorted(items, key=str.lower)\n"},
    })
    tasks.append({
        "id": "g2p-safe-get",
        "category": "gen2-python",
        "prompt": "utils.py exports safe_get(lst, i) which must return lst[i], supporting negative "
                  "indices (safe_get([10, 20, 30], -1) === 30). It currently returns lst[i + 1], which is "
                  "off by one. Fix the indexing.",
        "setup": lambda d: write(d, {"utils.py":
            "def safe_get(lst, i):\n"
            "    return lst[i + 1]\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.safe_get([10, 20, 30], 1) == 20, m.safe_get([10, 20, 30], 1)\n"
            "assert m.safe_get([10, 20, 30], -1) == 30, m.safe_get([10, 20, 30], -1)\n"
            "assert m.safe_get([10], 0) == 10\n"
            "assert m.safe_get([1, 2, 3], 2) == 3\n"
            "assert m.safe_get([1, 2, 3], -3) == 1\n"),
        "_fixed": {"utils.py":
            "def safe_get(lst, i):\n"
            "    return lst[i]\n"},
    })
    tasks.append({
        "id": "g2p-set-order",
        "category": "gen2-python",
        "prompt": "utils.py exports unique(lst) which must de-duplicate PRESERVING first-occurrence "
                  "order (unique([3, 1, 3, 2]) === [3, 1, 2]). It currently returns list(set(lst)), whose "
                  "order is arbitrary. Fix it to preserve order (e.g. dict.fromkeys).",
        "setup": lambda d: write(d, {"utils.py":
            "def unique(lst):\n"
            "    return list(set(lst))\n"}),
        "test": lambda d: py(d, "utils",
            "assert m.unique([3, 1, 3, 2]) == [3, 1, 2], m.unique([3, 1, 3, 2])\n"
            "assert m.unique(['a', 'b', 'a']) == ['a', 'b']\n"),
        "_fixed": {"utils.py":
            "def unique(lst):\n"
            "    return list(dict.fromkeys(lst))\n"},
    })

    # ══════════════════════════════════════════════════════════════════════
    # gen2-multifile — bugs spanning 2-3 files
    # ══════════════════════════════════════════════════════════════════════
    tasks.append({
        "id": "g2m-counter-shared",
        "category": "gen2-multifile",
        "prompt": "Express app across two files: counter.js keeps a module-level counter with inc() and "
                  "get(); server.js exposes POST /api/inc (increments) and GET /api/count (reads). The "
                  "counter NEVER increments — it stays 0 — because the increment is lost. Find and fix "
                  "the bug in counter.js.",
        "setup": express_setup({
            "counter.js":
                "let count = 0;\n"
                "exports.inc = function () { let count = 0; count += 1; };  // BUG: shadows module count\n"
                "exports.get = function () { return count; };\n",
            "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    await fetch(base + '/api/inc', { method: 'POST' });\n"
            "    await fetch(base + '/api/inc', { method: 'POST' });\n"
            "    await fetch(base + '/api/inc', { method: 'POST' });\n"
            "    let j = await (await fetch(base + '/api/count')).json();\n"
            "    if (j.count !== 3) throw new Error('count ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"counter.js":
            "let count = 0;\n"
            "exports.inc = function () { count += 1; };\n"
            "exports.get = function () { return count; };\n"},
    })
    tasks.append({
        "id": "g2m-circular-import",
        "category": "gen2-multifile",
        "prompt": "Two modules import each other: a.js requires b.js, b.js requires a.js. When b.js "
                  "LOADS it reads a.name at module top-level, but a hasn't finished initializing yet, so "
                  "it captures undefined ('a is undefined'). Fix b.js so the value is read lazily inside "
                  "greet() instead of at load time.",
        "setup": lambda d: write(d, {
            "a.js":
                "const b = require('./b');\n"
                "exports.name = 'a';\n"
                "exports.greet = () => 'hi from a, b is ' + b.name;\n",
            "b.js":
                "const a = require('./a');\n"
                "const greeting = 'a is ' + a.name;  // BUG: read at load time, a not ready\n"
                "exports.name = 'b';\n"
                "exports.greet = () => greeting;\n",
        }),
        "test": lambda d: rt(d, "a",
            "if (m.greet() !== 'hi from a, b is b') throw new Error('a got ' + m.greet());\n"
            "const m2 = require('./b');\n"
            "if (m2.greet() !== 'a is a') throw new Error('b got ' + m2.greet());\n"
            "if (m2.greet() !== 'a is a') throw new Error('b repeated got ' + m2.greet());\n"),
        "_fixed": {"b.js":
            "const a = require('./a');\n"
            "exports.name = 'b';\n"
            "exports.greet = () => 'a is ' + a.name;\n"},
    })
    tasks.append({
        "id": "g2m-stale-config",
        "category": "gen2-multifile",
        "prompt": "Express app in two files: config.js holds a mutable config object with set(); "
                  "server.js reads config.name in GET /api/name. POST /api/name updates the config via "
                  "set(), but GET always returns the OLD value ('default'). The update is not visible — "
                  "find and fix the bug in server.js (it takes a SNAPSHOT of the config at load time "
                  "instead of reading the live object).",
        "setup": express_setup({
            "config.js":
                "exports.config = { name: 'default' };\n"
                "exports.set = function (k, v) { exports.config[k] = v; };\n",
            "server.js": _expr(
                "const store = require('./config');\n"
                "const config = { ...store.config };  // BUG: snapshot at load\n"
                "app.get('/api/name', (req, res) => res.json({ name: config.name }));\n"
                "app.post('/api/name', (req, res) => { store.set('name', req.body.name); res.json({ ok: true }); });\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let r = await fetch(base + '/api/name', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'renamed' }) });\n"
            "    let j = await (await fetch(base + '/api/name')).json();\n"
            "    if (j.name !== 'renamed') throw new Error('stale config: ' + JSON.stringify(j));\n"
            "    r = await fetch(base + '/api/name', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'renamed2' }) });\n"
            "    j = await (await fetch(base + '/api/name')).json();\n"
            "    if (j.name !== 'renamed2') throw new Error('second update stale: ' + JSON.stringify(j));\n"
            "    r = await fetch(base + '/api/name', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'final' }) });\n"
            "    j = await (await fetch(base + '/api/name')).json();\n"
            "    if (j.name !== 'final') throw new Error('third update stale: ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const store = require('./config');\n"
            "const config = store.config;\n"
            "app.get('/api/name', (req, res) => res.json({ name: config.name }));\n"
            "app.post('/api/name', (req, res) => { store.set('name', req.body.name); res.json({ ok: true }); });\n")},
    })
    tasks.append({
        "id": "g2m-repo-service",
        "category": "gen2-multifile",
        "prompt": "Three files: repo.js (data), service.js (business logic), server.js (routes). "
                  "GET /api/items should return a JSON ARRAY of items but currently returns an object "
                  "wrapper ({items: [...]}). The bug is in service.js: list() wraps the repo result in an "
                  "object. Fix it to return the array directly.",
        "setup": express_setup({
            "repo.js":
                "const items = [{ id: 1, name: 'apple' }];\n"
                "exports.getItems = function () { return items; };\n"
                "exports.addItem = function (name) { items.push({ id: items.length + 1, name }); };\n",
            "service.js":
                "const repo = require('./repo');\n"
                "exports.list = function () { const r = repo.getItems(); return { items: r }; };  // BUG: wraps in object\n"
                "exports.create = function (name) { repo.addItem(name); };\n",
            "server.js": _expr(
                "const service = require('./service');\n"
                "app.get('/api/items', (req, res) => res.json(service.list()));\n"
                "app.post('/api/items', (req, res) => { service.create(req.body.name); res.status(201).json({ ok: true }); });\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || list.length === 0) throw new Error('not array: ' + JSON.stringify(list));\n"
            "    await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'cherry' }) });\n"
            "    list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || !list.some(i => i.name === 'cherry')) throw new Error('post lost: ' + JSON.stringify(list));\n"
        )),
        "_fixed": {"service.js":
            "const repo = require('./repo');\n"
            "exports.list = function () { return repo.getItems(); };\n"
            "exports.create = function (name) { repo.addItem(name); };\n"},
    })
    tasks.append({
        "id": "g2m-async-init",
        "category": "gen2-multifile",
        "prompt": "db.js requires init() to be called before query() works — query() returns null until "
                  "then. server.js's GET /api/items returns null because init() is never called. Fix "
                  "server.js to call db.init() before serving.",
        "setup": express_setup({
            "db.js":
                "let inited = false;\n"
                "exports.init = function () { inited = true; };\n"
                "exports.query = function () { return inited ? [{ id: 1, name: 'apple' }] : null; };\n",
            "server.js": _expr(
                "const db = require('./db');\n"
                "app.get('/api/items', (req, res) => res.json(db.query()));  // BUG: init() never called\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(j) || j.length !== 1) throw new Error('not inited: ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const db = require('./db');\n"
            "db.init();\n"
            "app.get('/api/items', (req, res) => res.json(db.query()));\n")},
    })
    tasks.append({
        "id": "g2m-event-ready",
        "category": "gen2-multifile",
        "prompt": "events.js emits 'ready' ONCE at module load — before server.js attaches its listener — "
                  "so the listener never fires and /api/status always reports started:false. Fix "
                  "events.js so the event can be emitted after listeners attach (e.g. expose markReady() "
                  "and have server.js call it after registering the listener).",
        "setup": express_setup({
            "events.js":
                "const { EventEmitter } = require('events');\n"
                "const emitter = new EventEmitter();\n"
                "emitter.emit('ready');  // BUG: emitted before anyone listens\n"
                "exports.onReady = (fn) => emitter.on('ready', fn);\n",
            "server.js": _expr(
                "const events = require('./events');\n"
                "let started = false;\n"
                "events.onReady(() => { started = true; });\n"
                "app.get('/api/status', (req, res) => res.json({ started }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let r = await fetch(base + '/api/status');\n"
            "    let t = await r.text();\n"
            "    let j;\n"
            "    try { j = JSON.parse(t); } catch (e) { throw new Error('expected JSON from /api/status, got: ' + t.slice(0, 200)); }\n"
            "    if (j.started !== true) throw new Error('expected {started:true}, got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"events.js":
            "const { EventEmitter } = require('events');\n"
            "const emitter = new EventEmitter();\n"
            "exports.onReady = (fn) => emitter.on('ready', fn);\n"
            "exports.markReady = () => emitter.emit('ready');\n",
            "server.js": _expr(
                "const events = require('./events');\n"
                "let started = false;\n"
                "events.onReady(() => { started = true; });\n"
                "events.markReady();\n"
                "app.get('/api/status', (req, res) => res.json({ started }));\n")},
    })
    tasks.append({
        "id": "g2m-utils-export",
        "category": "gen2-multifile",
        "prompt": "main.js requires ./utils and calls utils.add / utils.sub, but both are undefined and "
                  "calls crash. The bug is in utils.js: it assigns to `exports` directly, which breaks "
                  "the module.exports reference. Fix utils.js to use module.exports.",
        "setup": lambda d: write(d, {
            "utils.js":
                "exports = { add: (a, b) => a + b, sub: (a, b) => a - b };  // BUG: exports reassigned\n",
            "main.js":
                "const utils = require('./utils');\n"
                "exports.run = (a, b) => ({ sum: utils.add(a, b), diff: utils.sub(a, b) });\n",
        }),
        "test": lambda d: rt(d, "main",
            "const r = m.run(5, 3);\n"
            "if (r.sum !== 8 || r.diff !== 2) throw new Error('got ' + JSON.stringify(r));\n"),
        "_fixed": {"utils.js":
            "module.exports = { add: (a, b) => a + b, sub: (a, b) => a - b };\n"},
    })
    tasks.append({
        "id": "g2m-singleton-data",
        "category": "gen2-multifile",
        "prompt": "db.js is an in-memory store used by server.js: POST /api/items adds an item, "
                  "GET /api/items lists them. But GET always returns an empty list — the bug: db.js's "
                  "getItems() creates a NEW array on every call, so added items vanish. Fix db.js to keep "
                  "ONE shared module-level array.",
        "setup": express_setup({
            "db.js":
                "exports.getItems = function () { const items = []; return items; };  // BUG: new array every call\n"
                "exports.addItem = function (name) { const items = exports.getItems(); items.push({ id: items.length + 1, name }); };\n",
            "server.js": _expr(
                "const db = require('./db');\n"
                "app.get('/api/items', (req, res) => res.json(db.getItems()));\n"
                "app.post('/api/items', (req, res) => { db.addItem(req.body.name); res.status(201).json({ ok: true }); });\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'cherry' }) });\n"
            "    let list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || !list.some(i => i.name === 'cherry')) throw new Error('lost: ' + JSON.stringify(list));\n"
            "    await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'kiwi' }) });\n"
            "    list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!list.some(i => i.name === 'kiwi')) throw new Error('second lost: ' + JSON.stringify(list));\n"
        )),
        "_fixed": {"db.js":
            "const items = [];\n"
            "exports.getItems = function () { return items; };\n"
            "exports.addItem = function (name) { items.push({ id: items.length + 1, name }); };\n"},
    })
    tasks.append({
        "id": "g2m-export-rename",
        "category": "gen2-multifile",
        "prompt": "db.js exports queryAll() but server.js calls db.query() — a renamed export mismatch, "
                  "so GET /api/items crashes with a 500. Fix server.js to call the correct method name.",
        "setup": express_setup({
            "db.js":
                "exports.queryAll = function () { return [{ id: 1, name: 'apple' }]; };\n",
            "server.js": _expr(
                "const db = require('./db');\n"
                "app.get('/api/items', (req, res) => res.json(db.query()));  // BUG: wrong method name\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let r = await fetch(base + '/api/items');\n"
            "    if (r.status !== 200) throw new Error('status ' + r.status);\n"
            "    let list = await r.json();\n"
            "    if (!Array.isArray(list) || list.length !== 1) throw new Error('got ' + JSON.stringify(list));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const db = require('./db');\n"
            "app.get('/api/items', (req, res) => res.json(db.queryAll()));\n")},
    })
    tasks.append({
        "id": "g2m-shared-cache",
        "category": "gen2-multifile",
        "prompt": "Express app with an in-memory cache in cache.js: GET /api/items caches the list the "
                  "first time; POST /api/items adds an item and must update the cache. After POST, GET "
                  "still returns the OLD list — find and fix the bug in server.js (a typo in the cache "
                  "key used on the POST path).",
        "setup": express_setup({
            "cache.js":
                "const store = new Map();\n"
                "exports.get = function (key) { return store.get(key); };\n"
                "exports.set = function (key, value) { store.set(key, value); };\n",
            "server.js": _expr(
                "const cache = require('./cache');\n"
                "const items = [{ id: 1, name: 'apple' }];\n"
                "app.get('/api/items', (req, res) => {\n"
                "  const hit = cache.get('items');\n"
                "  if (hit) return res.json(hit);\n"
                "  const data = JSON.parse(JSON.stringify(items));\n"
                "  cache.set('items', data);\n"
                "  res.json(data);\n"
                "});\n"
                "app.post('/api/items', (req, res) => {\n"
                "  items.push({ id: items.length + 1, name: req.body.name });\n"
                "  cache.set('itmes', items);  // BUG: typo'd cache key\n"
                "  res.status(201).json({ ok: true });\n"
                "});\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    await fetch(base + '/api/items');\n"
            "    await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'cherry' }) });\n"
            "    let list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || !list.some(i => i.name === 'cherry')) throw new Error('stale cache: ' + JSON.stringify(list));\n"
            "    await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'kiwi' }) });\n"
            "    list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!list.some(i => i.name === 'kiwi')) throw new Error('cache hit stale: ' + JSON.stringify(list));\n"
            "    await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'fig' }) });\n"
            "    list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!list.some(i => i.name === 'fig')) throw new Error('third stale: ' + JSON.stringify(list));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const cache = require('./cache');\n"
            "const items = [{ id: 1, name: 'apple' }];\n"
            "app.get('/api/items', (req, res) => {\n"
            "  const hit = cache.get('items');\n"
            "  if (hit) return res.json(hit);\n"
            "  const data = JSON.parse(JSON.stringify(items));\n"
            "  cache.set('items', data);\n"
            "  res.json(data);\n"
            "});\n"
            "app.post('/api/items', (req, res) => {\n"
            "  items.push({ id: items.length + 1, name: req.body.name });\n"
            "  cache.set('items', items);\n"
            "  res.status(201).json({ ok: true });\n"
            "});\n")},
    })
    tasks.append({
        "id": "g2m-routes-split",
        "category": "gen2-multifile",
        "prompt": "Routes are split into routes.js (a router with POST/GET /api/items) and server.js "
                  "(the express app). POST /api/items 500s because the router is mounted BEFORE "
                  "express.json() parses bodies, so req.body is undefined. Fix server.js to register the "
                  "JSON parser BEFORE the router.",
        "setup": express_setup({
            "routes.js":
                "const express = require('express');\n"
                "const router = express.Router();\n"
                "const items = [];\n"
                "router.post('/api/items', (req, res) => { items.push({ id: items.length + 1, name: req.body.name }); res.status(201).json({ ok: true }); });\n"
                "router.get('/api/items', (req, res) => res.json(items));\n"
                "module.exports = router;\n",
            "server.js":
                "const express = require('express');\n"
                "const app = express();\n"
                "const router = require('./routes');\n"
                "app.use(router);  // BUG: router mounted BEFORE json parser\n"
                "app.use(express.json());\n"
                "module.exports = app;\n",
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let r = await fetch(base + '/api/items', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'cherry' }) });\n"
            "    if (r.status !== 201) throw new Error('POST status ' + r.status);\n"
            "    let list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || !list.some(i => i.name === 'cherry')) throw new Error('lost: ' + JSON.stringify(list));\n"
        )),
        "_fixed": {"server.js":
            "const express = require('express');\n"
            "const app = express();\n"
            "app.use(express.json());\n"
            "const router = require('./routes');\n"
            "app.use(router);\n"
            "module.exports = app;\n"},
    })
    tasks.append({
        "id": "g2m-global-reset",
        "category": "gen2-multifile",
        "prompt": "stats.js tracks per-page visit counts with inc(key) / get(key) / reset(). server.js's "
                  "POST /api/visit/:page calls stats.reset() before incrementing — so every visit resets "
                  "the counter and counts never exceed 1. Remove the reset() call in server.js so counts "
                  "accumulate.",
        "setup": express_setup({
            "stats.js":
                "const counts = {};\n"
                "exports.inc = function (k) { counts[k] = (counts[k] || 0) + 1; };\n"
                "exports.get = function (k) { return counts[k] || 0; };\n"
                "exports.reset = function () { for (const k of Object.keys(counts)) delete counts[k]; };\n",
            "server.js": _expr(
                "const stats = require('./stats');\n"
                "app.post('/api/visit/:page', (req, res) => { stats.reset(); stats.inc(req.params.page); res.json({ ok: true }); });  // BUG: resets every visit\n"
                "app.get('/api/visits/:page', (req, res) => res.json({ count: stats.get(req.params.page) }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    await fetch(base + '/api/visit/home', { method: 'POST' });\n"
            "    await fetch(base + '/api/visit/home', { method: 'POST' });\n"
            "    await fetch(base + '/api/visit/about', { method: 'POST' });\n"
            "    let j = await (await fetch(base + '/api/visits/home')).json();\n"
            "    if (j.count !== 2) throw new Error('home count ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/visits/about')).json();\n"
            "    if (j.count !== 1) throw new Error('about count ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const stats = require('./stats');\n"
            "app.post('/api/visit/:page', (req, res) => { stats.inc(req.params.page); res.json({ ok: true }); });\n"
            "app.get('/api/visits/:page', (req, res) => res.json({ count: stats.get(req.params.page) }));\n")},
    })
    tasks.append({
        "id": "g2m-relative-path",
        "category": "gen2-multifile",
        "prompt": "server.js requires ./helpers for formatName, but the file lives in utils/helpers.js — "
                  "the wrong relative path breaks the whole app (MODULE_NOT_FOUND 500). Fix the require "
                  "path in server.js.",
        "setup": express_setup({
            "utils/helpers.js":
                "exports.formatName = (n) => (n || '').toUpperCase();\n",
            "server.js": _expr(
                "const { formatName } = require('./helpers');  // BUG: wrong path (file is in utils/)\n"
                "app.get('/api/name', (req, res) => res.json({ name: formatName(req.query.name) }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let r = await fetch(base + '/api/name?name=bob');\n"
            "    if (r.status !== 200) throw new Error('status ' + r.status);\n"
            "    let j = await r.json();\n"
            "    if (j.name !== 'BOB') throw new Error('got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const { formatName } = require('./utils/helpers');\n"
            "app.get('/api/name', (req, res) => res.json({ name: formatName(req.query.name) }));\n")},
    })
    tasks.append({
        "id": "g2m-config-default",
        "category": "gen2-multifile",
        "prompt": "config.js reads settings.json and provides get(key, default). server.js asks "
                  "config.get('name', 'fallback') — it always returns the fallback instead of the real "
                  "value 'myapp' from settings.json. Fix config.js so defaults are only used when the "
                  "key is missing.",
        "setup": express_setup({
            "settings.json": '{\n  "name": "myapp",\n  "port": 8080\n}\n',
            "config.js":
                "const data = require('./settings.json');\n"
                "exports.get = function (k, def) { return def !== undefined ? def : data[k]; };  // BUG: default always wins\n",
            "server.js": _expr(
                "const config = require('./config');\n"
                "app.get('/api/app', (req, res) => res.json({ name: config.get('name', 'fallback'), port: config.get('port', 3000) }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/app')).json();\n"
            "    if (j.name !== 'myapp') throw new Error('name ' + JSON.stringify(j));\n"
            "    if (j.port !== 8080) throw new Error('port ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"config.js":
            "const data = require('./settings.json');\n"
            "exports.get = function (k, def) { const v = data[k]; return v === undefined ? def : v; };\n"},
    })
    tasks.append({
        "id": "g2m-dual-file-mutate",
        "category": "gen2-multifile",
        "prompt": "Two files share a module-level settings object: settings.js defines it, and BOTH "
                  "server.js and admin.js import it. The bug: admin.js does `const settings = "
                  "{...require('./settings')}` — a COPY — so when server.js updates settings via "
                  "update(), admin.js keeps seeing stale values and GET /api/admin/flags returns the old "
                  "ones. Fix admin.js to use the live shared object.",
        "setup": express_setup({
            "settings.js":
                "exports.settings = { flags: { darkMode: false } };\n"
                "exports.update = function (k, v) { exports.settings[k] = v; };\n",
            "admin.js":
                "const s = require('./settings');\n"
                "const settings = { ...s.settings };  // BUG: snapshot copy\n"
                "exports.flags = () => settings;\n",
            "server.js": _expr(
                "const s = require('./settings');\n"
                "const admin = require('./admin');\n"
                "app.post('/api/settings', (req, res) => { s.update(req.body.key, req.body.value); res.json({ ok: true }); });\n"
                "app.get('/api/admin/flags', (req, res) => res.json(admin.flags()));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/admin/flags')).json();\n"
            "    if (j.flags.darkMode !== false) throw new Error('initial ' + JSON.stringify(j));\n"
            "    await fetch(base + '/api/settings', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ key: 'flags', value: { darkMode: true } }) });\n"
            "    j = await (await fetch(base + '/api/admin/flags')).json();\n"
            "    if (j.flags.darkMode !== true) throw new Error('stale admin: ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"admin.js":
            "const s = require('./settings');\n"
            "const settings = s.settings;\n"
            "exports.flags = () => settings;\n"},
    })
    tasks.append({
        "id": "g2m-gate-order",
        "category": "gen2-multifile",
        "prompt": "guard.js exports an isAdmin(user) check. server.js has GET /api/admin that must return "
                  "403 for non-admin users, but the guard is applied AFTER the route responds — so the "
                  "protected data leaks to everyone. Fix server.js to check isAdmin BEFORE sending the "
                  "response.",
        "setup": express_setup({
            "guard.js":
                "exports.isAdmin = function (user) { return !!(user && user.role === 'admin'); };\n",
            "server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "const secret = { data: 'classified' };\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  res.json(secret);\n"
                "  if (!isAdmin(req.headers['x-user'] ? { role: req.headers['x-user'] } : null)) return res.status(403).json({ error: 'forbidden' });  // BUG: after response\n"
                "});\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let r = await fetch(base + '/api/admin');\n"
            "    if (r.status !== 403) throw new Error('anon should be 403, got ' + r.status);\n"
            "    r = await fetch(base + '/api/admin', { headers: { 'x-user': 'admin' } });\n"
            "    if (r.status !== 200) throw new Error('admin should be 200, got ' + r.status);\n"
        )),
        "_fixed": {"server.js": _expr(
            "const { isAdmin } = require('./guard');\n"
            "const secret = { data: 'classified' };\n"
            "app.get('/api/admin', (req, res) => {\n"
            "  if (!isAdmin(req.headers['x-user'] ? { role: req.headers['x-user'] } : null)) return res.status(403).json({ error: 'forbidden' });\n"
            "  res.json(secret);\n"
            "});\n")},
    })

    return tasks
