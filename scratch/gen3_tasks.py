"""gen3_tasks.py — 50-task "gen3" suite (4 skill axes).

  gen3-trap   (15) : the OBVIOUS fix is wrong — decoy comments, two bugs, wrong-file
                     error traces, surgical-fix traps, misleading names, shadowing...
  gen3-chain  (15) : 5 mini-repos x 3 sequential tasks — each task's hidden test depends
                     on the previous task's fix (cumulative state, real-repo style).
  gen3-log    (10) : the source looks fine; a debug.log captures the failure. Agent must
                     read the log -> trace root cause -> fix. SWE-bench ops style.
  gen3-long   (10) : long-horizon projects — 10-15 files, deep dirs, rename-across-files,
                     pipelines, interfaces. Stresses context growth, compaction, memory.

`build_tasks(H)` receives the helper dict from eval_runner (avoids circular import).
Every task carries `_fixed` (hand-written correct version) used ONLY by the pre-flight
verifier; the agent never sees it. Chain tasks also carry `_remove`.
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
    # gen3-trap — the obvious fix is wrong
    # ══════════════════════════════════════════════════════════════════════
    tasks.append({
        "id": "g3t-two-bugs",
        "category": "gen3-trap",
        "prompt": "utils.js exports processUser(u) which crashes with a 500 for some inputs. "
                  "Find ALL bugs and fix them. Requirements: (1) processUser(null) returns null; "
                  "(2) processUser({name:'  bob  '}) returns { name: 'bob' } — the name must be "
                  "trimmed and NOT uppercased; (3) processUser({}) must NOT crash — return "
                  "{ error: 'missing name' }.",
        "setup": lambda d: write(d, {"utils.js":
            "function processUser(u) {\n"
            "  if (!u) return null;\n"
            "  return { name: u.name.toUpperCase() };  // BUG A: crashes when name missing; BUG B: uppercases\n"
            "}\n"
            "module.exports = { processUser };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.processUser(null) !== null) throw new Error('null case');\n"
            "if (m.processUser({ name: '  bob  ' }).name !== 'bob') throw new Error('trim/lower got ' + JSON.stringify(m.processUser({ name: '  bob  ' })));\n"
            "if (m.processUser({}).error !== 'missing name') throw new Error('no crash ' + JSON.stringify(m.processUser({})));\n"
            "if (m.processUser({ name: '  A  ' }).name !== 'A') throw new Error('cap trim got ' + JSON.stringify(m.processUser({ name: '  A  ' })));\n"
            "if (m.processUser({ name: 'Bob' }).name !== 'Bob') throw new Error('must NOT uppercase: ' + JSON.stringify(m.processUser({ name: 'Bob' })));\n"
            "if (m.processUser({ name: 123 }).error !== 'missing name') throw new Error('non-string name got ' + JSON.stringify(m.processUser({ name: 123 })));\n"
            "if (m.processUser({ name: '   ' }).error !== 'missing name') throw new Error('blank name got ' + JSON.stringify(m.processUser({ name: '   ' })));\n"),
        "_fixed": {"utils.js":
            "function processUser(u) {\n"
            "  if (!u) return null;\n"
            "  const name = u.name;\n"
            "  if (typeof name !== 'string' || name.trim() === '') return { error: 'missing name' };\n"
            "  return { name: name.trim() };\n"
            "}\n"
            "module.exports = { processUser };\n"},
    })
    tasks.append({
        "id": "g3t-decoy-comment",
        "category": "gen3-trap",
        "prompt": "utils.js exports makeKey(user) which must return a stable key built from the "
                  "user's ID ('user:' + id) — two users with the same id but different names must "
                  "produce the SAME key. It currently returns 'user:' + name, so the key changes "
                  "with the name. Fix makeKey to use the user's id.",
        "setup": lambda d: write(d, {"utils.js":
            "// NOTE: this line is verified CORRECT — do not change it\n"
            "function getId(user) { return user.id; }\n"
            "function makeKey(user) { return 'user:' + user.name; }  // BUG: should use getId(user)\n"
            "module.exports = { getId, makeKey };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.makeKey({ id: 5, name: 'x' }) !== 'user:5') throw new Error('k1 got ' + m.makeKey({ id: 5, name: 'x' }));\n"
            "if (m.makeKey({ id: 5, name: 'y' }) !== 'user:5') throw new Error('k2 not stable: ' + m.makeKey({ id: 5, name: 'y' }));\n"
            "if (m.makeKey({ id: 0, name: 'z' }) !== 'user:0') throw new Error('zero id got ' + m.makeKey({ id: 0, name: 'z' }));\n"
            "if (m.makeKey({ id: 5, name: 'x' }) !== m.makeKey({ id: 5, name: 'completely different' })) throw new Error('id not stable');\n"
            "if (m.getId({ id: 9 }) !== 9) throw new Error('getId broken');\n"),
        "_fixed": {"utils.js":
            "// NOTE: this line is verified CORRECT — do not change it\n"
            "function getId(user) { return user.id; }\n"
            "function makeKey(user) { return 'user:' + getId(user); }\n"
            "module.exports = { getId, makeKey };\n"},
    })
    tasks.append({
        "id": "g3t-wrong-file-error",
        "category": "gen3-trap",
        "prompt": "server.js has a route GET /api/name that returns the wrong value — the reported "
                  "error points at server.js line 12, but the real bug is in the imported helper. "
                  "Trace the root cause: helper.js formatName() must return 'user-' + name in UPPER "
                  "case ('user-BOB'). It currently returns 'user-' + lowercase. Fix the ROOT CAUSE "
                  "(the helper), not the symptom in server.js.",
        "setup": express_setup({
            "helper.js":
                "exports.formatName = (n) => 'user-' + (n || '').toLowerCase();  // BUG: lowercase\n",
            "server.js": _expr(
                "const { formatName } = require('./helper');\n"
                "app.get('/api/name', (req, res) => res.json({ name: formatName(req.query.name) }));  // reported: line 12\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/name?name=bob')).json();\n"
            "    if (j.name !== 'user-BOB') throw new Error('got ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/name?name=ALICE')).json();\n"
            "    if (j.name !== 'user-ALICE') throw new Error('already-upper got ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/name?name=')).json();\n"
            "    if (j.name !== 'user-') throw new Error('empty got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"helper.js":
            "exports.formatName = (n) => 'user-' + (n || '').toUpperCase();\n"},
    })
    tasks.append({
        "id": "g3t-surgical-fix",
        "category": "gen3-trap",
        "prompt": "utils.js exports validateUser(input) which rejects the VALID name 'bob' with "
                  "'name too short'. The minimum length check is too strict. Fix the validation so "
                  "names of length >= 3 pass, but KEEP rejecting names shorter than 3 ('xy' must "
                  "still be rejected with 'name too short').",
        "setup": lambda d: write(d, {"utils.js":
            "function validateUser(input) {\n"
            "  if (typeof input.name !== 'string' || input.name.length === 0) return { error: 'name required' };\n"
            "  if (input.name.length < 5) return { error: 'name too short' };  // BUG: threshold too strict\n"
            "  return { ok: true };\n"
            "}\n"
            "module.exports = { validateUser };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.validateUser({ name: 'bob' }).ok !== true) throw new Error('bob rejected: ' + JSON.stringify(m.validateUser({ name: 'bob' })));\n"
            "if (m.validateUser({ name: 'xy' }).error !== 'name too short') throw new Error('xy must fail: ' + JSON.stringify(m.validateUser({ name: 'xy' })));\n"
            "if (m.validateUser({ name: 'abcd' }).ok !== true) throw new Error('abcd rejected');\n"
            "if (m.validateUser({ name: '' }).error !== 'name required') throw new Error('empty must fail');\n"
            "if (m.validateUser({ name: 'abc' }).ok !== true) throw new Error('exactly 3 must pass');\n"
            "if (m.validateUser({ name: 'ab' }).error !== 'name too short') throw new Error('2 chars must fail');\n"
            "if (m.validateUser({ name: 42 }).error !== 'name required') throw new Error('non-string must fail: ' + JSON.stringify(m.validateUser({ name: 42 })));\n"),
        "_fixed": {"utils.js":
            "function validateUser(input) {\n"
            "  if (typeof input.name !== 'string' || input.name.length === 0) return { error: 'name required' };\n"
            "  if (input.name.length < 3) return { error: 'name too short' };\n"
            "  return { ok: true };\n"
            "}\n"
            "module.exports = { validateUser };\n"},
    })
    tasks.append({
        "id": "g3t-symptom-vs-cause",
        "category": "gen3-trap",
        "prompt": "Express app: GET /api/items returns an EMPTY list, but the bug is NOT in the GET "
                  "route — something at startup wipes the data. Trace the real cause: items.js "
                  "seeds data on load, but server.js calls store.clear() during startup. Fix the "
                  "startup code so the seeded data survives.",
        "setup": express_setup({
            "items.js":
                "let items = [{ id: 1, name: 'apple' }, { id: 2, name: 'banana' }];\n"
                "exports.get = function () { return items; };\n"
                "exports.clear = function () { items = []; };\n",
            "server.js": _expr(
                "const store = require('./items');\n"
                "store.clear();  // BUG: wipes seeded data at boot\n"
                "app.get('/api/items', (req, res) => res.json(store.get()));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || list.length !== 2) throw new Error('wiped: ' + JSON.stringify(list));\n"
            "    if (list[0].name !== 'apple' || list[1].name !== 'banana') throw new Error('wrong data: ' + JSON.stringify(list));\n"
            "    list = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(list) || list.length !== 2) throw new Error('second read wiped: ' + JSON.stringify(list));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const store = require('./items');\n"
            "app.get('/api/items', (req, res) => res.json(store.get()));\n")},
    })
    tasks.append({
        "id": "g3t-misleading-name",
        "category": "gen3-trap",
        "prompt": "utils.js exports size() which should return the NUMBER of items added via "
                  "addItem(). It currently returns the array itself instead of its length — the "
                  "variable named `count` is misleading (it holds an array). Fix size() to return "
                  "count.length.",
        "setup": lambda d: write(d, {"utils.js":
            "const count = [];  // misleading name: it's an ARRAY\n"
            "function addItem(x) { count.push(x); }\n"
            "function size() { return count; }  // BUG: returns array not length\n"
            "module.exports = { addItem, size };\n"}),
        "test": lambda d: rt(d, "utils",
            "m.addItem(1); m.addItem(2); m.addItem(3);\n"
            "if (m.size() !== 3) throw new Error('got ' + JSON.stringify(m.size()));\n"
            "m.addItem(4);\n"
            "if (m.size() !== 4) throw new Error('after add got ' + JSON.stringify(m.size()));\n"
            "m.addItem(undefined); m.addItem(null);\n"
            "if (m.size() !== 6) throw new Error('after odd adds got ' + JSON.stringify(m.size()));\n"),
        "_fixed": {"utils.js":
            "const count = [];\n"
            "function addItem(x) { count.push(x); }\n"
            "function size() { return count.length; }\n"
            "module.exports = { addItem, size };\n"},
    })
    tasks.append({
        "id": "g3t-double-inversion",
        "category": "gen3-trap",
        "prompt": "utils.js exports isAllowed(user) which must return true only for a non-null user "
                  "with active === true, and false otherwise (including null). It currently returns "
                  "the OPPOSITE due to inverted logic with a precedence trap. Fix the boolean logic "
                  "carefully — changing just one ! will not fix it.",
        "setup": lambda d: write(d, {"utils.js":
            "function isAllowed(user) { return !user || user.active ? false : true; }  // BUG: precedence + inversion\n"
            "module.exports = { isAllowed };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.isAllowed({ active: true }) !== true) throw new Error('active got ' + m.isAllowed({ active: true }));\n"
            "if (m.isAllowed({ active: false }) !== false) throw new Error('inactive got ' + m.isAllowed({ active: false }));\n"
            "if (m.isAllowed(null) !== false) throw new Error('null got ' + m.isAllowed(null));\n"
            "if (m.isAllowed({}) !== false) throw new Error('empty got ' + m.isAllowed({}));\n"
            "if (m.isAllowed(undefined) !== false) throw new Error('undefined got ' + m.isAllowed(undefined));\n"
            "if (m.isAllowed({ active: true, extra: 1 }) !== true) throw new Error('extra field broke');\n"
            "if (m.isAllowed({ active: true, active2: false }) !== true) throw new Error('nested field trap');\n"),
        "_fixed": {"utils.js":
            "function isAllowed(user) { return !!(user && user.active); }\n"
            "module.exports = { isAllowed };\n"},
    })
    tasks.append({
        "id": "g3t-shadow-variable",
        "category": "gen3-trap",
        "prompt": "utils.js exports addItem(newItem) which should append to the module-level list. "
                  "It currently crashes with a ReferenceError because the function declares a LOCAL "
                  "variable with the same name (`items`) and reads it before it is initialized — a "
                  "shadowing/hoisting trap. Fix it so the module-level list is used.",
        "setup": lambda d: write(d, {"utils.js":
            "let items = ['base'];\n"
            "function addItem(newItem) { const items = items.concat([newItem]); return items; }  // BUG: shadows outer, TDZ crash\n"
            "exports.addItem = addItem;\n"
            "exports.list = function () { return items; };\n"}),
        "test": lambda d: rt(d, "utils",
            "m.addItem('a');\n"
            "m.addItem('b');\n"
            "if (m.list().join(',') !== 'base,a,b') throw new Error('got ' + m.list().join(','));\n"
            "m.addItem('c');\n"
            "if (m.list().join(',') !== 'base,a,b,c') throw new Error('third got ' + m.list().join(','));\n"
            "if (typeof m.addItem !== 'function') throw new Error('export shape');\n"),
        "_fixed": {"utils.js":
            "let items = ['base'];\n"
            "function addItem(newItem) { items.push(newItem); return items; }\n"
            "exports.addItem = addItem;\n"
            "exports.list = function () { return items; };\n"},
    })
    tasks.append({
        "id": "g3t-copy-paste",
        "category": "gen3-trap",
        "prompt": "utils.js exports filterOdds(nums) which must return the ODD numbers "
                  "(filterOdds([1,2,3]) === [1,3]). It returns the EVENS — it looks like the body "
                  "of filterEvens was copy-pasted. Fix filterOdds only; do NOT change filterEvens.",
        "setup": lambda d: write(d, {"utils.js":
            "function filterEvens(nums) { return nums.filter(n => n % 2 === 0); }\n"
            "function filterOdds(nums) { return nums.filter(n => n % 2 === 0); }  // BUG: copy-paste of evens\n"
            "module.exports = { filterEvens, filterOdds };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.filterOdds([1,2,3]).join(',') !== '1,3') throw new Error('odds got ' + m.filterOdds([1,2,3]).join(','));\n"
            "if (m.filterEvens([1,2,3]).join(',') !== '2') throw new Error('evens got ' + m.filterEvens([1,2,3]).join(','));\n"
            "if (m.filterOdds([1,2,3,4,5]).join(',') !== '1,3,5') throw new Error('odds5 got ' + m.filterOdds([1,2,3,4,5]).join(','));\n"
            "if (m.filterEvens([1,2,3,4,5]).join(',') !== '2,4') throw new Error('evens5 got ' + m.filterEvens([1,2,3,4,5]).join(','));\n"
            "if (m.filterOdds([]).length !== 0) throw new Error('empty');\n"),
        "_fixed": {"utils.js":
            "function filterEvens(nums) { return nums.filter(n => n % 2 === 0); }\n"
            "function filterOdds(nums) { return nums.filter(n => n % 2 !== 0); }\n"
            "module.exports = { filterEvens, filterOdds };\n"},
    })
    tasks.append({
        "id": "g3t-false-positive-test",
        "category": "gen3-trap",
        "prompt": "utils.js exports concat(list, sep) which must join list items with sep, IGNORING "
                  "null/undefined items (concat([1, null, 3], ',') === '1,3'). It currently lets "
                  "nulls through as empty strings, producing '1,,3'. A naive test with only string "
                  "inputs passes — the bug only shows with nulls. Fix concat to skip null/undefined.",
        "setup": lambda d: write(d, {"utils.js":
            "function concat(list, sep) { return list.join(sep); }  // BUG: nulls join as empty strings\n"
            "module.exports = { concat };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.concat([1, null, 3], ',') !== '1,3') throw new Error('nulls got ' + JSON.stringify(m.concat([1, null, 3], ',')));\n"
            "if (m.concat(['a', undefined, 'b'], '-') !== 'a-b') throw new Error('undefined got ' + JSON.stringify(m.concat(['a', undefined, 'b'], '-')));\n"
            "if (m.concat([1, 2], ',') !== '1,2') throw new Error('plain got ' + m.concat([1, 2], ','));\n"
            "if (m.concat([null, null], '-') !== '') throw new Error('all-null got ' + JSON.stringify(m.concat([null, null], '-')));\n"
            "if (m.concat([], ',') !== '') throw new Error('empty got ' + JSON.stringify(m.concat([], ',')));\n"
            "if (m.concat([null, 5, undefined, 6], ':') !== '5:6') throw new Error('mixed got ' + JSON.stringify(m.concat([null, 5, undefined, 6], ':')));\n"),
        "_fixed": {"utils.js":
            "function concat(list, sep) { return list.filter(x => x != null).join(sep); }\n"
            "module.exports = { concat };\n"},
    })
    tasks.append({
        "id": "g3t-order-trap",
        "category": "gen3-trap",
        "prompt": "utils.js exports parseLine(line) which parses 'name,age' CSV lines into "
                  "{ name, age }. It crashes when the line has NO age (just 'bob'). The correct "
                  "behavior for 'bob' is { name: 'bob', age: 0 } — NOT null, NOT an exception. "
                  "Fix the missing-field handling (guard parts[1]), do not just wrap everything in "
                  "try/catch returning null.",
        "setup": lambda d: write(d, {"utils.js":
            "function parseLine(line) {\n"
            "  const parts = line.split(',');\n"
            "  return { name: parts[0].trim(), age: Number(parts[1].trim()) };  // BUG: parts[1] undefined crashes\n"
            "}\n"
            "module.exports = { parseLine };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (JSON.stringify(m.parseLine('bob')) !== '{\"name\":\"bob\",\"age\":0}') throw new Error('no age got ' + JSON.stringify(m.parseLine('bob')));\n"
            "if (m.parseLine('alice,30').age !== 30) throw new Error('age got ' + m.parseLine('alice,30').age);\n"
            "if (JSON.stringify(m.parseLine(' bob , 30')) !== '{\"name\":\"bob\",\"age\":30}') throw new Error('spaces got ' + JSON.stringify(m.parseLine(' bob , 30')));\n"
            "if (m.parseLine('bob,').age !== 0) throw new Error('empty age got ' + m.parseLine('bob,').age);\n"
            "if (m.parseLine('alice,0').age !== 0) throw new Error('zero age got ' + m.parseLine('alice,0').age);\n"),
        "_fixed": {"utils.js":
            "function parseLine(line) {\n"
            "  const parts = line.split(',');\n"
            "  const age = parts[1] !== undefined && parts[1].trim() !== '' ? Number(parts[1].trim()) : 0;\n"
            "  return { name: parts[0].trim(), age };\n"
            "}\n"
            "module.exports = { parseLine };\n"},
    })
    tasks.append({
        "id": "g3t-escape-order",
        "category": "gen3-trap",
        "prompt": "utils.js exports escapeHtml(s) which must escape &, <, >, \" and ' "
                  "(escapeHtml('<b>&</b>') === '&lt;b&gt;&amp;&lt;/b&gt;'). It currently escapes in "
                  "the wrong ORDER — '&' is replaced LAST, so the '<' and '>' replacements produce "
                  "NEW '&' characters that get escaped again. Fix the replacement order (escape & "
                  "FIRST).",
        "setup": lambda d: write(d, {"utils.js":
            "function escapeHtml(s) { return s.replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/\"/g, '&quot;').replace(/'/g, '&#39;').replace(/&/g, '&amp;'); }  // BUG: & last -> double-escapes\n"
            "module.exports = { escapeHtml };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.escapeHtml('<b>&</b>') !== '&lt;b&gt;&amp;&lt;/b&gt;') throw new Error('got ' + m.escapeHtml('<b>&</b>'));\n"
            "if (m.escapeHtml('a&b') !== 'a&amp;b') throw new Error('amp got ' + m.escapeHtml('a&b'));\n"
            "if (m.escapeHtml('plain') !== 'plain') throw new Error('plain');\n"
            "if (m.escapeHtml('a&b&c') !== 'a&amp;b&amp;c') throw new Error('multi-amp got ' + m.escapeHtml('a&b&c'));\n"
            "if (m.escapeHtml(\"'<&\\\"\") !== '&#39;&lt;&amp;&quot;') throw new Error('all-five got ' + m.escapeHtml(\"'<&\\\"\"));\n"
            "if (m.escapeHtml('<a>&lt;</a>') !== '&lt;a&gt;&amp;lt;&lt;/a&gt;') throw new Error('pre-escaped got ' + m.escapeHtml('<a>&lt;</a>'));\n"),
        "_fixed": {"utils.js":
            "function escapeHtml(s) { return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/\"/g, '&quot;').replace(/'/g, '&#39;'); }\n"
            "module.exports = { escapeHtml };\n"},
    })
    tasks.append({
        "id": "g3t-type-trap",
        "category": "gen3-trap",
        "prompt": "utils.js exports lookupById(users, id) which must find a user whose id EQUALS "
                  "the given id. ids are NUMBERS in the data. The function currently compares with "
                  "loose == and also treats string ids as valid — so lookupById(users, '5') wrongly "
                  "finds the user with numeric id 5. Fix it to use STRICT comparison (===) so string "
                  "ids never match numeric ids.",
        "setup": lambda d: write(d, {"utils.js":
            "function lookupById(users, id) { return users.find(u => u.id == id) || null; }  // BUG: loose == matches strings\n"
            "module.exports = { lookupById };\n"}),
        "test": lambda d: rt(d, "utils",
            "const users = [{ id: 5, name: 'alice' }, { id: 10, name: 'bob' }];\n"
            "if (m.lookupById(users, 5).name !== 'alice') throw new Error('num lookup');\n"
            "if (m.lookupById(users, '5') !== null) throw new Error('string must not match: ' + JSON.stringify(m.lookupById(users, '5')));\n"
            "if (m.lookupById(users, 99) !== null) throw new Error('missing');\n"
            "if (m.lookupById(users, '05') !== null) throw new Error('padded string matched: ' + JSON.stringify(m.lookupById(users, '05')));\n"
            "const u0 = [{ id: 0, name: 'zero' }];\n"
            "if (m.lookupById(u0, '0') !== null) throw new Error('string 0 matched');\n"
            "if (m.lookupById(u0, 0).name !== 'zero') throw new Error('numeric 0 must match');\n"),
        "_fixed": {"utils.js":
            "function lookupById(users, id) { return users.find(u => u.id === id) || null; }\n"
            "module.exports = { lookupById };\n"},
    })
    tasks.append({
        "id": "g3t-greedy-regex",
        "category": "gen3-trap",
        "prompt": "utils.js exports stripTags(s) which must remove HTML tags, keeping text between "
                  "DIFFERENT tags (stripTags('<a>one</a><b>two</b>') === 'onetwo'). It currently "
                  "uses a GREEDY regex that swallows everything between the first < and the last > "
                  "('onetwo' would collapse to ''). Fix the regex to be non-greedy (e.g. /<[^>]*>/g).",
        "setup": lambda d: write(d, {"utils.js":
            "function stripTags(s) { return s.replace(/<.*>/g, ''); }  // BUG: greedy\n"
            "module.exports = { stripTags };\n",
            }),
        "test": lambda d: rt(d, "utils",
            "if (m.stripTags('<a>one</a><b>two</b>') !== 'onetwo') throw new Error('got ' + JSON.stringify(m.stripTags('<a>one</a><b>two</b>')));\n"
            "if (m.stripTags('plain') !== 'plain') throw new Error('plain');\n"
            "if (m.stripTags('<i>x</i>') !== 'x') throw new Error('single');\n"
            "if (m.stripTags('<p>a<br>b</p>') !== 'ab') throw new Error('nested got ' + JSON.stringify(m.stripTags('<p>a<br>b</p>')));\n"
            "if (m.stripTags('<a><b>x</b></a>') !== 'x') throw new Error('double got ' + JSON.stringify(m.stripTags('<a><b>x</b></a>')));\n"
            "if (m.stripTags('<>') !== '') throw new Error('empty-tag got ' + JSON.stringify(m.stripTags('<>')));\n"),
        "_fixed": {"utils.js":
            "function stripTags(s) { return s.replace(/<[^>]*>/g, ''); }\n"
            "module.exports = { stripTags };\n"},
    })
    tasks.append({
        "id": "g3t-empty-array",
        "category": "gen3-trap",
        "prompt": "utils.js exports head(items) which must return the FIRST element, or undefined "
                  "for an empty array (head([]) === undefined, NOT null). It currently returns null "
                  "for empty arrays via a falsy guard. Fix it so head([]) is undefined while "
                  "head([1,2]) is still 1.",
        "setup": lambda d: write(d, {"utils.js":
            "function head(items) { if (!items.length) return null; return items[0]; }  // BUG: null instead of undefined\n"
            "module.exports = { head };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.head([]) !== undefined) throw new Error('empty got ' + m.head([]));\n"
            "if (m.head([1, 2]) !== 1) throw new Error('first got ' + m.head([1, 2]));\n"
            "if (m.head([0]) !== 0) throw new Error('falsy elem got ' + m.head([0]));\n"
            "if (m.head([false]) !== false) throw new Error('false elem got ' + m.head([false]));\n"
            "if (m.head([null]) !== null) throw new Error('null elem got ' + m.head([null]));\n"
            "if (m.head([undefined]) !== undefined) throw new Error('undef elem got ' + m.head([undefined]));\n"),
        "_fixed": {"utils.js":
            "function head(items) { return items[0]; }\n"
            "module.exports = { head };\n"},
    })

    # ══════════════════════════════════════════════════════════════════════
    # gen3-chain — 5 mini-repos x 3 sequential tasks (cumulative state)
    # Each task's setup = base + previous tasks' FIXED code + this task's bug.
    # The hidden test checks BOTH previous behavior (no regression) and the new feature.
    # ══════════════════════════════════════════════════════════════════════
    def build_chain(cid, base_files, steps):
        out = []
        fixed_so_far = {}
        for i, st in enumerate(steps):
            fixed_so_far.update(st["fixed"])
            buggy_files = dict(base_files)
            # buggy = base + previous fixes + CURRENT bug patch (buggy must keep prior fixes)
            for rel, content in fixed_so_far.items():
                buggy_files[rel] = content
            buggy_files.update(st["bug"])
            tid = st["id"]
            out.append({
                "id": tid,
                "category": "gen3-chain",
                "prompt": st["prompt"],
                "setup": lambda d, _b=buggy_files: (write(d, _b), ensure_deps(d)),
                "test": lambda d, _t=st["test"]: _t(d),
                "_fixed": dict(base_files, **fixed_so_far),
            })
        return out

    # ── chain 1: orders (express CRUD) ──
    ch1_base = {
        "server.js": _expr(
            "const orders = [];\n"
            "app.get('/api/orders', (req, res) => res.json(orders));\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch1", ch1_base, [
        {
            "id": "g3c1-1",
            "prompt": "Add POST /api/orders to the existing Express app: body {name} must create an "
                      "order (201) and return it; a missing/empty name must return 400 with an error. "
                      "GET /api/orders must keep working. The current POST has no validation.",
            "bug": {"server.js": _expr(
                "const orders = [];\n"
                "app.get('/api/orders', (req, res) => res.json(orders));\n"
                "app.post('/api/orders', (req, res) => { orders.push({ id: orders.length + 1, name: req.body.name }); res.status(201).json({ ok: true }); });  // BUG: no validation\n"),
                },
            "fixed": {"server.js": _expr(
                "const orders = [];\n"
                "app.get('/api/orders', (req, res) => res.json(orders));\n"
                "app.post('/api/orders', (req, res) => {\n"
                "  const name = req.body && req.body.name;\n"
                "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
                "  orders.push({ id: orders.length + 1, name });\n"
                "  res.status(201).json({ ok: true });\n"
                "});\n")},
            "test": lambda d: api(d, (
                "    let r = await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'first' }) });\n"
                "    if (r.status !== 201) throw new Error('create status ' + r.status);\n"
                "    r = await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({}) });\n"
                "    if (r.status !== 400) throw new Error('empty must 400, got ' + r.status);\n"
                "    let list = await (await fetch(base + '/api/orders')).json();\n"
                "    if (list.length !== 1 || list[0].name !== 'first') throw new Error('list ' + JSON.stringify(list));\n"
            )),
        },
        {
            "id": "g3c1-2",
            "prompt": "Add DELETE /api/orders/:id to the existing app: removes the order with that "
                      "numeric id and returns 200; unknown ids return 404. The current DELETE never "
                      "matches because it compares a NUMBER id with a STRING param. Fix the "
                      "comparison (parse the param). POST and GET must keep working.",
            "bug": {"server.js": _expr(
                "const orders = [];\n"
                "app.get('/api/orders', (req, res) => res.json(orders));\n"
                "app.post('/api/orders', (req, res) => {\n"
                "  const name = req.body && req.body.name;\n"
                "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
                "  orders.push({ id: orders.length + 1, name });\n"
                "  res.status(201).json({ ok: true });\n"
                "});\n"
                "app.delete('/api/orders/:id', (req, res) => {\n"
                "  const idx = orders.findIndex(o => o.id === req.params.id);  // BUG: number vs string\n"
                "  if (idx === -1) return res.status(404).json({ error: 'not found' });\n"
                "  orders.splice(idx, 1);\n"
                "  res.json({ ok: true });\n"
                "});\n")},
            "fixed": {"server.js": _expr(
                "const orders = [];\n"
                "app.get('/api/orders', (req, res) => res.json(orders));\n"
                "app.post('/api/orders', (req, res) => {\n"
                "  const name = req.body && req.body.name;\n"
                "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
                "  orders.push({ id: orders.length + 1, name });\n"
                "  res.status(201).json({ ok: true });\n"
                "});\n"
                "app.delete('/api/orders/:id', (req, res) => {\n"
                "  const idx = orders.findIndex(o => o.id === Number(req.params.id));\n"
                "  if (idx === -1) return res.status(404).json({ error: 'not found' });\n"
                "  orders.splice(idx, 1);\n"
                "  res.json({ ok: true });\n"
                "});\n")},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'a' }) });\n"
                "    await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'b' }) });\n"
                "    let r = await fetch(base + '/api/orders/1', { method: 'DELETE' });\n"
                "    if (r.status !== 200) throw new Error('delete status ' + r.status);\n"
                "    let list = await (await fetch(base + '/api/orders')).json();\n"
                "    if (list.length !== 1 || list[0].name !== 'b') throw new Error('after del ' + JSON.stringify(list));\n"
                "    r = await fetch(base + '/api/orders/999', { method: 'DELETE' });\n"
                "    if (r.status !== 404) throw new Error('unknown must 404, got ' + r.status);\n"
            )),
        },
        {
            "id": "g3c1-3",
            "prompt": "Add a ?status= filter to GET /api/orders: with no query it must return ALL "
                      "orders; with ?status=open only open ones. The current filter returns [] when "
                      "no status is given. Fix the filtering so a missing status means 'all'. POST "
                      "and DELETE must keep working.",
            "bug": {"server.js": _expr(
                "const orders = [];\n"
                "app.get('/api/orders', (req, res) => {\n"
                "  if (!req.query.status) return res.json(orders.filter(o => o.status === req.query.status));  // BUG: no query -> []\n"
                "  res.json(orders.filter(o => o.status === req.query.status));\n"
                "});\n"
                "app.post('/api/orders', (req, res) => {\n"
                "  const name = req.body && req.body.name;\n"
                "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
                "  orders.push({ id: orders.length + 1, name, status: 'open' });\n"
                "  res.status(201).json({ ok: true });\n"
                "});\n"
                "app.delete('/api/orders/:id', (req, res) => {\n"
                "  const idx = orders.findIndex(o => o.id === Number(req.params.id));\n"
                "  if (idx === -1) return res.status(404).json({ error: 'not found' });\n"
                "  orders.splice(idx, 1);\n"
                "  res.json({ ok: true });\n"
                "});\n")},
            "fixed": {"server.js": _expr(
                "const orders = [];\n"
                "app.get('/api/orders', (req, res) => {\n"
                "  const filtered = req.query.status ? orders.filter(o => o.status === req.query.status) : orders;\n"
                "  res.json(filtered);\n"
                "});\n"
                "app.post('/api/orders', (req, res) => {\n"
                "  const name = req.body && req.body.name;\n"
                "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
                "  orders.push({ id: orders.length + 1, name, status: 'open' });\n"
                "  res.status(201).json({ ok: true });\n"
                "});\n"
                "app.delete('/api/orders/:id', (req, res) => {\n"
                "  const idx = orders.findIndex(o => o.id === Number(req.params.id));\n"
                "  if (idx === -1) return res.status(404).json({ error: 'not found' });\n"
                "  orders.splice(idx, 1);\n"
                "  res.json({ ok: true });\n"
                "});\n")},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'a' }) });\n"
                "    await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'b' }) });\n"
                "    let all = await (await fetch(base + '/api/orders')).json();\n"
                "    if (all.length !== 2) throw new Error('all got ' + JSON.stringify(all));\n"
                "    let open = await (await fetch(base + '/api/orders?status=open')).json();\n"
                "    if (open.length !== 2) throw new Error('open got ' + JSON.stringify(open));\n"
                "    await fetch(base + '/api/orders/1', { method: 'DELETE' });\n"
                "    all = await (await fetch(base + '/api/orders')).json();\n"
                "    if (all.length !== 1) throw new Error('after del ' + JSON.stringify(all));\n"
            )),
        },
    ])

    # ── chain 2: counter service ──
    ch2_base = {
        "counter.js":
            "let count = 0;\n"
            "exports.inc = function () { count += 1; };\n"
            "exports.get = function () { return count; };\n",
        "server.js": _expr(
            "const counter = require('./counter');\n"
            "app.post('/api/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
            "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch2", ch2_base, [
        {
            "id": "g3c2-1",
            "prompt": "Add POST /api/reset which must reset the counter to 0. The current reset "
                      "creates a LOCAL variable instead of resetting the module state, so GET "
                      "/api/count still shows the old value. Fix counter.js reset() to reset the "
                      "shared module counter.",
            "bug": {"counter.js":
                "let count = 0;\n"
                "exports.inc = function () { count += 1; };\n"
                "exports.get = function () { return count; };\n"
                "exports.reset = function () { let count = 0; }  // BUG: local shadow, module count unchanged\n",
                "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"
                "app.post('/api/reset', (req, res) => { counter.reset(); res.json({ ok: true }); });\n")},
            "fixed": {"counter.js":
                "let count = 0;\n"
                "exports.inc = function () { count += 1; };\n"
                "exports.get = function () { return count; };\n"
                "exports.reset = function () { count = 0; };\n",
                "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"
                "app.post('/api/reset', (req, res) => { counter.reset(); res.json({ ok: true }); });\n")},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    let j = await (await fetch(base + '/api/count')).json();\n"
                "    if (j.count !== 2) throw new Error('pre-reset ' + JSON.stringify(j));\n"
                "    await fetch(base + '/api/reset', { method: 'POST' });\n"
                "    j = await (await fetch(base + '/api/count')).json();\n"
                "    if (j.count !== 0) throw new Error('post-reset ' + JSON.stringify(j));\n"
                "    await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    j = await (await fetch(base + '/api/count')).json();\n"
                "    if (j.count !== 1) throw new Error('after inc ' + JSON.stringify(j));\n"
            )),
        },
        {
            "id": "g3c2-2",
            "prompt": "Add a maximum of 10: POST /api/inc must return 400 with {error:'limit reached'} "
                      "when the counter is already at the limit. The current inc() has no limit. "
                      "Implement the limit in counter.js (inc returns a status) and have the route "
                      "return 400 accordingly. reset must keep working.",
            "bug": {"counter.js":
                "let count = 0;\n"
                "const LIMIT = 10;\n"
                "exports.inc = function () { count += 1; return { ok: true }; };  // BUG: no limit check\n"
                "exports.get = function () { return count; };\n"
                "exports.reset = function () { count = 0; };\n",
                "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"
                "app.post('/api/reset', (req, res) => { counter.reset(); res.json({ ok: true }); });\n")},
            "fixed": {"counter.js":
                "let count = 0;\n"
                "const LIMIT = 10;\n"
                "exports.inc = function () {\n"
                "  if (count >= LIMIT) return { ok: false, error: 'limit reached' };\n"
                "  count += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.get = function () { return count; };\n"
                "exports.reset = function () { count = 0; };\n",
                "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { const r = counter.inc(); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"
                "app.post('/api/reset', (req, res) => { counter.reset(); res.json({ ok: true }); });\n")},
            "test": lambda d: api(d, (
                "    for (let i = 0; i < 10; i++) await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    let r = await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    if (r.status !== 400) throw new Error('limit should 400, got ' + r.status);\n"
                "    let j = await (await fetch(base + '/api/count')).json();\n"
                "    if (j.count !== 10) throw new Error('count ' + JSON.stringify(j));\n"
                "    await fetch(base + '/api/reset', { method: 'POST' });\n"
                "    j = await (await fetch(base + '/api/count')).json();\n"
                "    if (j.count !== 0) throw new Error('post-reset ' + JSON.stringify(j));\n"
            )),
        },
        {
            "id": "g3c2-3",
            "prompt": "Add GET /api/stats returning { count, limit }. The current stats endpoint "
                      "returns the LIMIT as the count — it reads the wrong variable. Fix stats() in "
                      "counter.js to report the real count. inc/reset/limit must keep working.",
            "bug": {"counter.js":
                "let count = 0;\n"
                "const LIMIT = 10;\n"
                "exports.inc = function () {\n"
                "  if (count >= LIMIT) return { ok: false, error: 'limit reached' };\n"
                "  count += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.get = function () { return count; };\n"
                "exports.reset = function () { count = 0; };\n"
                "exports.stats = function () { return { count: LIMIT, limit: LIMIT }; };  // BUG: count = limit\n",
                "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { const r = counter.inc(); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"
                "app.post('/api/reset', (req, res) => { counter.reset(); res.json({ ok: true }); });\n"
                "app.get('/api/stats', (req, res) => res.json(counter.stats()));\n")},
            "fixed": {"counter.js":
                "let count = 0;\n"
                "const LIMIT = 10;\n"
                "exports.inc = function () {\n"
                "  if (count >= LIMIT) return { ok: false, error: 'limit reached' };\n"
                "  count += 1;\n"
                "  return { ok: true };\n"
                "};\n"
                "exports.get = function () { return count; };\n"
                "exports.reset = function () { count = 0; };\n"
                "exports.stats = function () { return { count, limit: LIMIT }; };\n",
                "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/inc', (req, res) => { const r = counter.inc(); if (!r.ok) return res.status(400).json({ error: r.error }); res.json({ ok: true }); });\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"
                "app.post('/api/reset', (req, res) => { counter.reset(); res.json({ ok: true }); });\n"
                "app.get('/api/stats', (req, res) => res.json(counter.stats()));\n")},
            "test": lambda d: api(d, (
                "    await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    await fetch(base + '/api/inc', { method: 'POST' });\n"
                "    let j = await (await fetch(base + '/api/stats')).json();\n"
                "    if (j.count !== 3 || j.limit !== 10) throw new Error('stats ' + JSON.stringify(j));\n"
                "    j = await (await fetch(base + '/api/count')).json();\n"
                "    if (j.count !== 3) throw new Error('count ' + JSON.stringify(j));\n"
            )),
        },
    ])

    # ── chain 3: utils library ──
    ch3_base = {
        "utils.js":
            "function formatName(name) { return name.trim(); }\n"
            "module.exports = { formatName };\n",
    }
    tasks += build_chain("ch3", ch3_base, [
        {
            "id": "g3c3-1",
            "prompt": "Add dedupe(list) to utils.js: removes duplicates case-insensitively, keeping "
                      "the FIRST occurrence (dedupe(['A','a','B']) === ['A','B']). The current "
                      "dedupe is case-sensitive. Use formatName for the case-insensitive comparison.",
            "bug": {"utils.js":
                "function formatName(name) { return name.trim(); }\n"
                "function dedupe(list) { return list.filter((v, i, a) => a.findIndex(x => x === v) === i); }  // BUG: case-sensitive\n"
                "module.exports = { formatName, dedupe };\n"},
            "fixed": {"utils.js":
                "function formatName(name) { return name.trim(); }\n"
                "function dedupe(list) { return list.filter((v, i, a) => a.findIndex(x => formatName(String(x)).toLowerCase() === formatName(String(v)).toLowerCase()) === i); }\n"
                "module.exports = { formatName, dedupe };\n"},
            "test": lambda d: rt(d, "utils",
                "const r = m.dedupe(['A', 'a', 'B']);\n"
                "if (r.length !== 2 || r[0] !== 'A' || r[1] !== 'B') throw new Error('got ' + JSON.stringify(r));\n"
                "if (m.dedupe(['x']).length !== 1) throw new Error('single');\n"),
        },
        {
            "id": "g3c3-2",
            "prompt": "Add sortByName(list) to utils.js: sorts strings case-insensitively by their "
                      "formatName (sortByName(['b','A']) === ['A','b']). The current sort is "
                      "case-sensitive. dedupe must keep working.",
            "bug": {"utils.js":
                "function formatName(name) { return name.trim(); }\n"
                "function dedupe(list) { return list.filter((v, i, a) => a.findIndex(x => formatName(String(x)).toLowerCase() === formatName(String(v)).toLowerCase()) === i); }\n"
                "function sortByName(list) { return [...list].sort(); }  // BUG: case-sensitive\n"
                "module.exports = { formatName, dedupe, sortByName };\n"},
            "fixed": {"utils.js":
                "function formatName(name) { return name.trim(); }\n"
                "function dedupe(list) { return list.filter((v, i, a) => a.findIndex(x => formatName(String(x)).toLowerCase() === formatName(String(v)).toLowerCase()) === i); }\n"
                "function sortByName(list) { return [...list].sort((x, y) => formatName(String(x)).toLowerCase().localeCompare(formatName(String(y)).toLowerCase())); }\n"
                "module.exports = { formatName, dedupe, sortByName };\n"},
            "test": lambda d: rt(d, "utils",
                "if (m.sortByName(['b', 'a', 'C']).join(',') !== 'a,b,C') throw new Error('got ' + m.sortByName(['b', 'a', 'C']).join(','));\n"
                "if (m.dedupe(['A', 'a']).length !== 1) throw new Error('dedupe regressed');\n"),
        },
        {
            "id": "g3c3-3",
            "prompt": "Add groupByFirstLetter(list) to utils.js: returns an object mapping the "
                      "lowercased first letter of each name (via formatName) to the list of matching "
                      "names (groupByFirstLetter(['apple','Banana']) === {a:['apple'], b:['Banana']}). "
                      "The current grouping is case-sensitive (keys 'a' and 'B' separate). sortByName "
                      "and dedupe must keep working.",
            "bug": {"utils.js":
                "function formatName(name) { return name.trim(); }\n"
                "function dedupe(list) { return list.filter((v, i, a) => a.findIndex(x => formatName(String(x)).toLowerCase() === formatName(String(v)).toLowerCase()) === i); }\n"
                "function sortByName(list) { return [...list].sort((x, y) => formatName(String(x)).toLowerCase().localeCompare(formatName(String(y)).toLowerCase())); }\n"
                "function groupByFirstLetter(list) { const out = {}; for (const n of list) { const k = formatName(String(n))[0]; (out[k] = out[k] || []).push(n); } return out; }  // BUG: key not lowercased\n"
                "module.exports = { formatName, dedupe, sortByName, groupByFirstLetter };\n"},
            "fixed": {"utils.js":
                "function formatName(name) { return name.trim(); }\n"
                "function dedupe(list) { return list.filter((v, i, a) => a.findIndex(x => formatName(String(x)).toLowerCase() === formatName(String(v)).toLowerCase()) === i); }\n"
                "function sortByName(list) { return [...list].sort((x, y) => formatName(String(x)).toLowerCase().localeCompare(formatName(String(y)).toLowerCase())); }\n"
                "function groupByFirstLetter(list) { const out = {}; for (const n of list) { const k = formatName(String(n))[0].toLowerCase(); (out[k] = out[k] || []).push(n); } return out; }\n"
                "module.exports = { formatName, dedupe, sortByName, groupByFirstLetter };\n"},
            "test": lambda d: rt(d, "utils",
                "const g = m.groupByFirstLetter(['apple', 'Banana']);\n"
                "if (JSON.stringify(g) !== '{\"a\":[\"apple\"],\"b\":[\"Banana\"]}') throw new Error('got ' + JSON.stringify(g));\n"
                "if (m.sortByName(['b', 'A']).join(',') !== 'A,b') throw new Error('sort regressed');\n"),
        },
    ])

    # ── chain 4: cache with TTL ──
    ch4_base = {
        "cache.js":
            "const store = new Map();\n"
            "exports.get = function (key) { return store.has(key) ? store.get(key) : null; };\n"
            "exports.set = function (key, value) { store.set(key, value); };\n",
        "server.js": _expr(
            "const cache = require('./cache');\n"
            "app.get('/api/cached', (req, res) => res.json({ value: cache.get('k') }));\n"
            "app.post('/api/cached', (req, res) => { cache.set('k', req.body.value); res.json({ ok: true }); });\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch4", ch4_base, [
        {
            "id": "g3c4-1",
            "prompt": "Add TTL support: cache.get(key, ttlMs) must return null when the entry is "
                      "older than ttlMs. The current get() ignores ttl entirely and returns stale "
                      "entries. Store the timestamp on set() and check it in get(). The express "
                      "routes must keep working.",
            "bug": {"cache.js":
                "const store = new Map();\n"
                "exports.get = function (key, ttlMs) { return store.has(key) ? store.get(key) : null; };  // BUG: ignores ttl\n"
                "exports.set = function (key, value) { store.set(key, value); };\n"},
            "fixed": {"cache.js":
                "const store = new Map();\n"
                "exports.get = function (key, ttlMs) {\n"
                "  if (!store.has(key)) return null;\n"
                "  const entry = store.get(key);\n"
                "  if (ttlMs && Date.now() - entry.at > ttlMs) { store.delete(key); return null; }\n"
                "  return entry.value;\n"
                "};\n"
                "exports.set = function (key, value) { store.set(key, { value, at: Date.now() }); };\n"},
            "test": lambda d: rat(d, "cache",
                "    m.set('k', 42);\n"
                "    if (m.get('k') !== 42) throw new Error('no-ttl get');\n"
                "    if (m.get('k', 10000) !== 42) throw new Error('fresh ttl');\n"
                "    await new Promise(r => setTimeout(r, 60));\n"
                "    if (m.get('k', 20) !== null) throw new Error('expired should be null, got ' + JSON.stringify(m.get('k', 20)));\n"),
        },
        {
            "id": "g3c4-2",
            "prompt": "Add cache.invalidateAll() which must remove ALL entries. The current "
                      "implementation creates a NEW Map but never reassigns the module-level store, "
                      "so old entries survive. Fix invalidateAll to clear the REAL store. TTL get "
                      "must keep working.",
            "bug": {"cache.js":
                "const store = new Map();\n"
                "exports.get = function (key, ttlMs) {\n"
                "  if (!store.has(key)) return null;\n"
                "  const entry = store.get(key);\n"
                "  if (ttlMs && Date.now() - entry.at > ttlMs) { store.delete(key); return null; }\n"
                "  return entry.value;\n"
                "};\n"
                "exports.set = function (key, value) { store.set(key, { value, at: Date.now() }); };\n"
                "exports.invalidateAll = function () { const store = new Map(); };  // BUG: shadows, never clears\n"},
            "fixed": {"cache.js":
                "const store = new Map();\n"
                "exports.get = function (key, ttlMs) {\n"
                "  if (!store.has(key)) return null;\n"
                "  const entry = store.get(key);\n"
                "  if (ttlMs && Date.now() - entry.at > ttlMs) { store.delete(key); return null; }\n"
                "  return entry.value;\n"
                "};\n"
                "exports.set = function (key, value) { store.set(key, { value, at: Date.now() }); };\n"
                "exports.invalidateAll = function () { store.clear(); };\n"},
            "test": lambda d: rt(d, "cache",
                "m.set('a', 1); m.set('b', 2);\n"
                "m.invalidateAll();\n"
                "if (m.get('a') !== null) throw new Error('a survived');\n"
                "if (m.get('b') !== null) throw new Error('b survived');\n"
                "m.set('c', 3);\n"
                "if (m.get('c') !== 3) throw new Error('set after invalidate');\n"),
        },
        {
            "id": "g3c4-3",
            "prompt": "The express app has POST /api/cached writing under key 'k' and GET reading "
                      "'k'. After a POST, GET must return the NEW value. The current POST writes to "
                      "the wrong key ('k2' typo), so GET keeps returning the old value. Fix the key "
                      "in server.js. cache.js must keep working.",
            "bug": {"server.js": _expr(
                "const cache = require('./cache');\n"
                "app.get('/api/cached', (req, res) => res.json({ value: cache.get('k') }));\n"
                "app.post('/api/cached', (req, res) => { cache.set('k2', req.body.value); res.json({ ok: true }); });  // BUG: typo'd key\n")},
            "fixed": {"server.js": _expr(
                "const cache = require('./cache');\n"
                "app.get('/api/cached', (req, res) => res.json({ value: cache.get('k') }));\n"
                "app.post('/api/cached', (req, res) => { cache.set('k', req.body.value); res.json({ ok: true }); });\n")},
            "test": lambda d: api(d, (
                "    let j = await (await fetch(base + '/api/cached')).json();\n"
                "    await fetch(base + '/api/cached', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ value: 'fresh' }) });\n"
                "    j = await (await fetch(base + '/api/cached')).json();\n"
                "    if (j.value !== 'fresh') throw new Error('stale: ' + JSON.stringify(j));\n"
            )),
        },
    ])

    # ── chain 5: auth + rate limit ──
    ch5_base = {
        "guard.js":
            "exports.isAdmin = function (user) { return !!(user && user.role === 'admin'); };\n",
        "server.js": _expr(
            "const { isAdmin } = require('./guard');\n"
            "app.get('/api/admin', (req, res) => {\n"
            "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
            "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
            "  res.json({ data: 'classified' });\n"
            "});\n"),
        "package.json": _pkg(),
    }
    tasks += build_chain("ch5", ch5_base, [
        {
            "id": "g3c5-1",
            "prompt": "Add a token check to the admin route: requests WITHOUT an 'x-token' header "
                      "must get 401, and the existing admin check (x-user: admin -> 200, else 403) "
                      "must still work. The current middleware reads the WRONG header "
                      "('x-auth-token'), so requests that send 'x-token' still pass through to the "
                      "admin check. Fix it to check the correct header.",
            "bug": {"server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  if (!req.headers['x-auth-token']) return res.status(401).json({ error: 'no token' });  // BUG: wrong header name\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json({ data: 'classified' });\n"
                "});\n")},
            "fixed": {"server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token) return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json({ data: 'classified' });\n"
                "});\n")},
            "test": lambda d: api(d, (
                "    let r = await fetch(base + '/api/admin');\n"
                "    if (r.status !== 401) throw new Error('no token should 401, got ' + r.status);\n"
                "    r = await fetch(base + '/api/admin', { headers: { 'x-token': 't', 'x-user': 'admin' } });\n"
                "    if (r.status !== 200) throw new Error('admin with x-token should 200, got ' + r.status);\n"
                "    r = await fetch(base + '/api/admin', { headers: { 'x-token': 't' } });\n"
                "    if (r.status !== 403) throw new Error('anon should 403, got ' + r.status);\n"
                "    r = await fetch(base + '/api/admin', { headers: { 'x-auth-token': 't', 'x-user': 'admin' } });\n"
                "    if (r.status !== 401) throw new Error('wrong header should 401, got ' + r.status);\n"
            )),
        },
        {
            "id": "g3c5-2",
            "prompt": "Add GET /api/admin/users returning [{name:'alice'}] — accessible ONLY to "
                      "admins WITH a valid token (403 for non-admins even with a token, 401 without "
                      "token). The current implementation checks the token but FORGETS the admin "
                      "check, so anyone with a token sees the users. Fix it to require both.",
            "bug": {"server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "const users = [{ name: 'alice' }];\n"
                "app.get('/api/admin/users', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token || token.trim() === '') return res.status(401).json({ error: 'no token' });\n"
                "  res.json(users);  // BUG: missing admin check\n"
                "});\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token || token.trim() === '') return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json({ data: 'classified' });\n"
                "});\n")},
            "fixed": {"server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "const users = [{ name: 'alice' }];\n"
                "app.get('/api/admin/users', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token || token.trim() === '') return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json(users);\n"
                "});\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token || token.trim() === '') return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json({ data: 'classified' });\n"
                "});\n")},
            "test": lambda d: api(d, (
                "    let r = await fetch(base + '/api/admin/users', { headers: { 'x-token': 't' } });\n"
                "    if (r.status !== 403) throw new Error('token-only should 403, got ' + r.status);\n"
                "    r = await fetch(base + '/api/admin/users', { headers: { 'x-token': 't', 'x-user': 'admin' } });\n"
                "    if (r.status !== 200) throw new Error('admin should 200, got ' + r.status);\n"
                "    r = await fetch(base + '/api/admin/users');\n"
                "    if (r.status !== 401) throw new Error('no token should 401, got ' + r.status);\n"
            )),
        },
        {
            "id": "g3c5-3",
            "prompt": "Add rate limiting: each IP may call POST /api/inc at most 5 times per minute "
                      "— the 6th call must return 429. The current limiter uses a GLOBAL counter "
                      "shared by ALL IPs, so different clients block each other. Fix it to key the "
                      "counter by IP (req.ip).",
            "bug": {"server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "const users = [{ name: 'alice' }];\n"
                "const hits = {};\n"
                "let globalHits = 0;  // BUG: global, not per-IP\n"
                "app.set('trust proxy', true);\n"
                "app.post('/api/inc', (req, res) => {\n"
                "  globalHits += 1;\n"
                "  if (globalHits > 5) return res.status(429).json({ error: 'too many requests' });\n"
                "  res.json({ ok: true });\n"
                "});\n"
                "app.get('/api/admin/users', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token) return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json(users);\n"
                "});\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token) return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json({ data: 'classified' });\n"
                "});\n"),
                },
            "fixed": {"server.js": _expr(
                "const { isAdmin } = require('./guard');\n"
                "const users = [{ name: 'alice' }];\n"
                "const hits = {};\n"
                "app.set('trust proxy', true);\n"
                "app.post('/api/inc', (req, res) => {\n"
                "  const ip = req.ip || 'unknown';\n"
                "  hits[ip] = (hits[ip] || 0) + 1;\n"
                "  if (hits[ip] > 5) return res.status(429).json({ error: 'too many requests' });\n"
                "  res.json({ ok: true });\n"
                "});\n"
                "app.get('/api/admin/users', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token) return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json(users);\n"
                "});\n"
                "app.get('/api/admin', (req, res) => {\n"
                "  const token = req.headers['x-token'];\n"
                "  if (!token) return res.status(401).json({ error: 'no token' });\n"
                "  const user = req.headers['x-user'] ? { role: req.headers['x-user'] } : null;\n"
                "  if (!isAdmin(user)) return res.status(403).json({ error: 'forbidden' });\n"
                "  res.json({ data: 'classified' });\n"
                "});\n")},
            "test": lambda d: api(d, (
                "    // client A: 5 allowed, 6th blocked (per-IP budget)\n"
                "    let ok = 0, limited = 0;\n"
                "    for (let i = 0; i < 6; i++) {\n"
                "      const r = await fetch(base + '/api/inc', { method: 'POST', headers: { 'x-forwarded-for': '10.0.0.1' } });\n"
                "      if (r.status === 200) ok++; else if (r.status === 429) limited++;\n"
                "    }\n"
                "    if (ok !== 5 || limited !== 1) throw new Error('A: ok=' + ok + ' limited=' + limited);\n"
                "    // client B: a DIFFERENT ip must NOT be blocked by A's budget\n"
                "    const r = await fetch(base + '/api/inc', { method: 'POST', headers: { 'x-forwarded-for': '10.0.0.2' } });\n"
                "    if (r.status !== 200) throw new Error('B should have fresh budget, got ' + r.status);\n"
                "    // client C: a THIRD ip also starts fresh\n"
                "    const r2 = await fetch(base + '/api/inc', { method: 'POST', headers: { 'x-forwarded-for': '10.0.0.3' } });\n"
                "    if (r2.status !== 200) throw new Error('C should have fresh budget, got ' + r2.status);\n"
            )),
        },
    ])

    # ══════════════════════════════════════════════════════════════════════
    # gen3-log — debug-from-log: source looks fine, debug.log has the failure.
    # The agent must READ the log, trace the root cause, and fix the source.
    # ══════════════════════════════════════════════════════════════════════
    def log_task(tid, prompt, log_content, buggy_files, fixed_files, test_lambda):
        files = dict(buggy_files)
        files["debug.log"] = log_content
        return {
            "id": tid,
            "category": "gen3-log",
            "prompt": prompt + "\n\nA captured debug.log is included in the project — READ IT FIRST; "
                      "it contains the exact failure from a real run.",
            "setup": lambda d, _f=files: (write(d, _f), ensure_deps(d)),
            "test": test_lambda,
            "_fixed": fixed_files,
        }

    tasks.append(log_task(
        "g3l-undefined-prop",
        "utils.js exports totalPrice(cart) which should sum item prices. A runtime crash was "
        "captured in debug.log: 'TypeError: Cannot read properties of undefined (reading "
        "'price')'. Some items in the cart are missing the price field. Fix totalPrice to skip "
        "items without a price instead of crashing.",
        "[15:04:22] ERROR  TypeError: Cannot read properties of undefined (reading 'price')\n"
        "[15:04:22]          at totalPrice (utils.js:3:38)\n"
        "[15:04:22]          at <anonymous> (index.js:12)\n",
        {"utils.js":
            "function totalPrice(cart) { return cart.reduce((sum, item) => sum + item.price, 0); }  // BUG: crashes on missing price\n"
            "module.exports = { totalPrice };\n"},
        {"utils.js":
            "function totalPrice(cart) { return cart.reduce((sum, item) => sum + (item && typeof item.price === 'number' ? item.price : 0), 0); }\n"
            "module.exports = { totalPrice };\n"},
        lambda d: rt(d, "utils",
            "if (m.totalPrice([{ price: 10 }, { price: 20 }, { name: 'no price' }]) !== 30) throw new Error('got ' + m.totalPrice([{ price: 10 }, { price: 20 }, { name: 'no price' }]));\n"
            "if (m.totalPrice([]) !== 0) throw new Error('empty');\n"),
    ))
    tasks.append(log_task(
        "g3l-nan-result",
        "utils.js exports computeTotal(amounts) which sums currency strings like '$3.50'. The log "
        "shows the result is NaN. The code uses parseFloat directly on the '$' strings, which "
        "yields NaN. Fix it to strip non-numeric characters before parsing.",
        "[15:10:01] WARN   computeTotal returned NaN for input [\"$3.50\", \"$1.25\"]\n"
        "[15:10:01]        expected 4.75, got NaN\n",
        {"utils.js":
            "function computeTotal(amounts) { return amounts.reduce((a, x) => a + parseFloat(x), 0); }  // BUG: parseFloat('$3.50') = NaN\n"
            "module.exports = { computeTotal };\n"},
        {"utils.js":
            "function computeTotal(amounts) { return amounts.reduce((a, x) => a + parseFloat(x.replace(/[^0-9.]/g, '')), 0); }\n"
            "module.exports = { computeTotal };\n"},
        lambda d: rt(d, "utils",
            "const r = m.computeTotal(['$3.50', '$1.25']);\n"
            "if (!isFinite(r) || Math.abs(r - 4.75) > 0.001) throw new Error('got ' + r);\n"),
    ))
    tasks.append(log_task(
        "g3l-off-by-one",
        "utils.js exports pageItems(items, page, size) for 1-based pagination. The log shows page 1 "
        "skips the first item — an off-by-one. Fix the slice indices.",
        "[15:12:44] INFO   page 1 size 2 -> expected [a,b], got [b,c]\n",
        {"utils.js":
            "function pageItems(items, page, size) { const start = (page - 1) * size; return items.slice(start + 1, start + size + 1); }  // BUG: off-by-one\n"
            "module.exports = { pageItems };\n"},
        {"utils.js":
            "function pageItems(items, page, size) { const start = (page - 1) * size; return items.slice(start, start + size); }\n"
            "module.exports = { pageItems };\n"},
        lambda d: rt(d, "utils",
            "const items = ['a', 'b', 'c', 'd', 'e'];\n"
            "if (m.pageItems(items, 1, 2).join(',') !== 'a,b') throw new Error('p1 got ' + m.pageItems(items, 1, 2).join(','));\n"
            "if (m.pageItems(items, 2, 2).join(',') !== 'c,d') throw new Error('p2 got ' + m.pageItems(items, 2, 2).join(','));\n"),
    ))
    tasks.append(log_task(
        "g3l-mutates-input",
        "utils.js exports sortCopy(arr) which must return a sorted COPY. The log shows the ORIGINAL "
        "array was mutated — a caller noticed arr changed after sortCopy. Fix it to not sort in "
        "place.",
        "[15:15:03] WARN   input array [3,1,2] was MUTATED by sortCopy (now [1,2,3])\n",
        {"utils.js":
            "function sortCopy(arr) { return arr.sort((a, b) => a - b); }  // BUG: sorts in place\n"
            "module.exports = { sortCopy };\n"},
        {"utils.js":
            "function sortCopy(arr) { return [...arr].sort((a, b) => a - b); }\n"
            "module.exports = { sortCopy };\n"},
        lambda d: rt(d, "utils",
            "const orig = [3, 1, 2];\n"
            "const s = m.sortCopy(orig);\n"
            "if (s.join(',') !== '1,2,3') throw new Error('sorted ' + s.join(','));\n"
            "if (orig.join(',') !== '3,1,2') throw new Error('ORIGINAL MUTATED');\n"),
    ))
    tasks.append(log_task(
        "g3l-async-await",
        "utils.js exports loadUser(id) which must return the user's NAME (the .name field, e.g. "
        "'alice'). The log shows it resolves to undefined — loadUser reads '.name' on the Promise "
        "returned by the internal async fetch instead of awaiting it first. Fix loadUser so it "
        "awaits fetchUser before accessing .name, and returns the name string (NOT the whole user "
        "object and NOT a Promise — callers await it and expect 'alice').",
        "[15:18:20] ERROR  loadUser(1) resolved to undefined (accessed .name on a Promise)\n"
        "[15:18:20]        at loadUser (utils.js:4)\n",
        {"utils.js":
            "async function fetchUser(id) { return new Promise(res => setTimeout(() => res({ id, name: 'alice' }), 10)); }\n"
            "function loadUser(id) { const u = fetchUser(id); return u.name; }  // BUG: u is a Promise, .name undefined\n"
            "module.exports = { loadUser };\n"},
        {"utils.js":
            "async function fetchUser(id) { return new Promise(res => setTimeout(() => res({ id, name: 'alice' }), 10)); }\n"
            "async function loadUser(id) { const u = await fetchUser(id); return u.name; }\n"
            "module.exports = { loadUser };\n"},
        lambda d: rat(d, "utils",
            "    const name = await m.loadUser(1);\n"
            "    if (name !== 'alice') throw new Error('got ' + JSON.stringify(name));\n"),
    ))
    tasks.append(log_task(
        "g3l-swallowed-error",
        "utils.js exports runTask(fn) which must return fn()'s result and PROPAGATE errors. The log "
        "shows an error was swallowed — runTask caught everything and returned null. Fix it to "
        "only handle the expected case or rethrow.",
        "[15:21:44] ERROR  task 'sync' failed silently: Error: boom\n"
        "[15:21:44]        runTask returned null — error swallowed by broad catch\n",
        {"utils.js":
            "async function runTask(fn) { try { return await fn(); } catch (e) { return null; } }  // BUG: swallows errors\n"
            "module.exports = { runTask };\n"},
        {"utils.js":
            "async function runTask(fn) { return await fn(); }\n"
            "module.exports = { runTask };\n"},
        lambda d: rat(d, "utils",
            "    if ((await m.runTask(async () => 42)) !== 42) throw new Error('success path');\n"
            "    let rejected = false;\n"
            "    try { await m.runTask(async () => { throw new Error('boom'); }); } catch (e) { rejected = true; }\n"
            "    if (!rejected) throw new Error('error was swallowed');\n"),
    ))
    tasks.append(log_task(
        "g3l-parse-json",
        "utils.js exports safeParse(s) which must return the parsed JSON or null for invalid input. "
        "The log shows an UNHANDLED exception escaped — safeParse threw instead of returning null. "
        "Add try/catch around JSON.parse.",
        "[15:24:12] ERROR  unhandled SyntaxError: Unexpected token '<'\n"
        "[15:24:12]        at JSON.parse (<anonymous>)\n"
        "[15:24:12]        at safeParse (utils.js:2)\n",
        {"utils.js":
            "function safeParse(s) { return JSON.parse(s); }  // BUG: throws on invalid JSON\n"
            "module.exports = { safeParse };\n"},
        {"utils.js":
            "function safeParse(s) { try { return JSON.parse(s); } catch (e) { return null; } }\n"
            "module.exports = { safeParse };\n"},
        lambda d: rt(d, "utils",
            "if (JSON.stringify(m.safeParse('{\"a\":1}')) !== '{\"a\":1}') throw new Error('valid');\n"
            "if (m.safeParse('not json') !== null) throw new Error('invalid should be null');\n"
            "if (m.safeParse('') !== null) throw new Error('empty should be null');\n"),
    ))
    tasks.append(log_task(
        "g3l-wrong-export",
        "main.js requires ./utils and calls utils.getUser, but the log shows it crashes with "
        "'getUser is not a function'. The export in utils.js has a typo — it exports getUserData "
        "instead. Fix the export NAME so the callers work.",
        "[15:27:33] ERROR  TypeError: utils.getUser is not a function\n"
        "[15:27:33]        at main.js:6\n",
        {"utils.js":
            "const data = { id: 1, name: 'alice' };\n"
            "module.exports = { getUserData: () => data };  // BUG: wrong export name\n",
            "main.js":
            "const utils = require('./utils');\n"
            "exports.getUser = () => utils.getUser();\n"},
        {"utils.js":
            "const data = { id: 1, name: 'alice' };\n"
            "module.exports = { getUser: () => data };\n"},
        lambda d: rt(d, "main",
            "const u = m.getUser();\n"
            "if (!u || u.name !== 'alice') throw new Error('got ' + JSON.stringify(u));\n"),
    ))
    tasks.append(log_task(
        "g3l-stale-state",
        "utils.js exports nextId() which must return 1, 2, 3 on successive calls. The log shows it "
        "ALWAYS returns 1 — the counter is reset every call. Fix the module state so it persists.",
        "[15:30:01] WARN   nextId() called 3 times but always returned 1 (state reset each call)\n",
        {"utils.js":
            "function nextId() { let count = 0; count += 1; return count; }  // BUG: count local\n"
            "module.exports = { nextId };\n"},
        {"utils.js":
            "let count = 0;\n"
            "function nextId() { count += 1; return count; }\n"
            "module.exports = { nextId };\n"},
        lambda d: rt(d, "utils",
            "if (m.nextId() !== 1) throw new Error('1st got ' + m.nextId());\n"
            "if (m.nextId() !== 2) throw new Error('2nd got ' + m.nextId());\n"
            "if (m.nextId() !== 3) throw new Error('3rd got ' + m.nextId());\n"),
    ))
    tasks.append(log_task(
        "g3l-array-index",
        "utils.js exports nthUser(users, n) which must return the n-th user (1-based) or null if out "
        "of range. The log shows a crash 'Cannot read properties of undefined (reading 0)' when "
        "users is empty. Add a bounds guard.",
        "[15:33:45] ERROR  TypeError: Cannot read properties of undefined (reading 'name')\n"
        "[15:33:45]        at nthUser (utils.js:2)  — empty array, n=1\n",
        {"utils.js":
            "function nthUser(users, n) { return users[n - 1].name; }  // BUG: no bounds check\n"
            "module.exports = { nthUser };\n"},
        {"utils.js":
            "function nthUser(users, n) { const u = users[n - 1]; return u ? u.name : null; }\n"
            "module.exports = { nthUser };\n"},
        lambda d: rt(d, "utils",
            "if (m.nthUser([{ name: 'a' }, { name: 'b' }], 1) !== 'a') throw new Error('first');\n"
            "if (m.nthUser([{ name: 'a' }], 5) !== null) throw new Error('oor should be null');\n"
            "if (m.nthUser([], 1) !== null) throw new Error('empty should be null');\n"),
    ))

    # ══════════════════════════════════════════════════════════════════════
    # gen3-long — long-horizon: bigger projects, more files, deeper dirs.
    # Stresses context growth, compaction, and working memory over many steps.
    # ══════════════════════════════════════════════════════════════════════
    tasks.append({
        "id": "g3lng-rename-everywhere",
        "category": "gen3-long",
        "prompt": "A small project: utils.js exports compute(), and FIVE other files call "
                  "utils.compute() or import it. The project owner renamed the function to "
                  "calculate() everywhere. Your job: complete the rename — utils.js must export "
                  "calculate(), ALL five callers must call calculate(), and the old name compute "
                  "must be GONE from every file. Behavior must not change (calculate(x) === "
                  "x * 2 + 1).",
        "setup": lambda d: write(d, {
            "utils.js":
                "function compute(x) { return x * 2 + 1; }\n"
                "module.exports = { compute };\n",
            "a.js": "const { compute } = require('./utils');\nmodule.exports = { run: () => compute(1) };\n",
            "b.js": "const utils = require('./utils');\nmodule.exports = { run: () => utils.compute(2) };\n",
            "c.js": "const { compute: c } = require('./utils');\nmodule.exports = { run: () => c(3) };\n",
            "d.js": "const u = require('./utils');\nmodule.exports = { run: () => u.compute(4) };\n",
            "e.js": "const { compute } = require('./utils');\nmodule.exports = { run: () => compute(5) };\n",
            "main.js":
                "module.exports = {\n"
                "  a: require('./a').run(),\n"
                "  b: require('./b').run(),\n"
                "  c: require('./c').run(),\n"
                "  d: require('./d').run(),\n"
                "  e: require('./e').run(),\n"
                "  direct: require('./utils').compute(10),\n"
                "};\n",
        }),
        "test": lambda d: ref(d, "main",
            "    const r = m;\n"
            "    if (r.a !== 3 || r.b !== 5 || r.c !== 7 || r.d !== 9 || r.e !== 11) throw new Error('callers broken ' + JSON.stringify(r));\n"
            "    if (r.direct !== 21) throw new Error('direct got ' + r.direct);\n",
            [("has_export", "utils.js", "calculate"),
             ("lacks_export", "utils.js", "compute"),
             ("lacks_text", "a.js", "compute"), ("lacks_text", "b.js", "compute"),
             ("lacks_text", "c.js", "compute"), ("lacks_text", "d.js", "compute"),
             ("lacks_text", "e.js", "compute"), ("lacks_text", "main.js", "compute")]),
        "_fixed": {
            "utils.js": "function calculate(x) { return x * 2 + 1; }\nmodule.exports = { calculate };\n",
            "a.js": "const { calculate } = require('./utils');\nmodule.exports = { run: () => calculate(1) };\n",
            "b.js": "const utils = require('./utils');\nmodule.exports = { run: () => utils.calculate(2) };\n",
            "c.js": "const { calculate: c } = require('./utils');\nmodule.exports = { run: () => c(3) };\n",
            "d.js": "const u = require('./utils');\nmodule.exports = { run: () => u.calculate(4) };\n",
            "e.js": "const { calculate } = require('./utils');\nmodule.exports = { run: () => calculate(5) };\n",
            "main.js":
                "module.exports = {\n"
                "  a: require('./a').run(),\n"
                "  b: require('./b').run(),\n"
                "  c: require('./c').run(),\n"
                "  d: require('./d').run(),\n"
                "  e: require('./e').run(),\n"
                "  direct: require('./utils').calculate(10),\n"
                "};\n",
        },
    })
    tasks.append({
        "id": "g3lng-deep-dirs",
        "category": "gen3-long",
        "prompt": "The bug is hidden deep in the project. src/lib/math/calc.js exports sum(nums) "
                  "which must return the sum — it currently returns nums.length (the count instead "
                  "of the sum). Every other module is correct. Find the buggy file by exploring the "
                  "tree and fix sum(). index.js must then work end to end.",
        "setup": lambda d: write(d, {
            "index.js":
                "const { viaA } = require('./src/service/a');\n"
                "module.exports = { result: viaA([1, 2, 3, 4]) };\n",
            "src/service/a.js": "const { viaB } = require('./b');\nexports.viaA = (n) => viaB(n);\n",
            "src/service/b.js": "const { viaC } = require('../core/c');\nexports.viaB = (n) => viaC(n);\n",
            "src/core/c.js": "const { viaD } = require('./d');\nexports.viaC = (n) => viaD(n);\n",
            "src/core/d.js": "const { viaE } = require('../lib/math/e');\nexports.viaD = (n) => viaE(n);\n",
            "src/lib/math/e.js": "const { sum } = require('./calc');\nexports.viaE = (n) => sum(n);\n",
            "src/lib/math/calc.js":
                "function sum(nums) { return nums.length; }  // BUG: count, not sum\n"
                "module.exports = { sum };\n",
        }),
        "test": lambda d: rt(d, "index",
            "if (m.result !== 10) throw new Error('got ' + m.result);\n"),
        "_fixed": {"src/lib/math/calc.js":
            "function sum(nums) { return nums.reduce((a, b) => a + b, 0); }\n"
            "module.exports = { sum };\n"},
    })
    tasks.append({
        "id": "g3lng-interface-impl",
        "category": "gen3-long",
        "prompt": "Four backend services all implement the same interface: format(items) returns "
                  "items.map(x => '[' + x + ']'). Three implementations are correct; ONE is subtly "
                  "wrong (it returns x + ']' without the opening bracket). Find the WRONG service "
                  "by exploring services/ and fix it. index.js routes through all four.",
        "setup": lambda d: write(d, {
            "index.js":
                "const a = require('./services/a');\n"
                "const b = require('./services/b');\n"
                "const c = require('./services/c');\n"
                "const dd = require('./services/d');\n"
                "module.exports = {\n"
                "  a: a.format(['x', 'y']),\n"
                "  b: b.format(['x', 'y']),\n"
                "  c: c.format(['x', 'y']),\n"
                "  d: dd.format(['x', 'y']),\n"
                "};\n",
            "services/a.js": "exports.format = (items) => items.map(x => '[' + x + ']');\n",
            "services/b.js": "exports.format = (items) => items.map(x => '[' + x + ']');\n",
            "services/c.js": "exports.format = (items) => items.map(x => '[' + x + ']');\n",
            "services/d.js": "exports.format = (items) => items.map(x => x + ']');  // BUG: missing '['\n",
        }),
        "test": lambda d: rt(d, "index",
            "const want = '[\"[' + 'x' + ']\",\"[' + 'y' + ']\"]';\n"
            "for (const k of ['a', 'b', 'c', 'd']) {\n"
            "  const got = JSON.stringify(m[k]);\n"
            "  if (got !== '[\"[x]\",\"[y]\"]') throw new Error(k + ' got ' + got);\n"
            "}\n"),
        "_fixed": {"services/d.js": "exports.format = (items) => items.map(x => '[' + x + ']');\n"},
    })
    tasks.append({
        "id": "g3lng-pipeline",
        "category": "gen3-long",
        "prompt": "A data pipeline processes words through 6 stages: a.js (split) -> b.js (trim) -> "
                  "c.js (lowercase) -> d.js (drop empties) -> e.js (dedupe) -> f.js (sort). One "
                  "stage in the middle is buggy: d.js drops every word SHORTER THAN 2 characters "
                  "(x.length > 1), which wrongly removes the single-character token '0'. Fix d.js "
                  "so only EMPTY strings are dropped. index.js runs the whole pipeline.",
        "setup": lambda d: write(d, {
            "index.js":
                "const a = require('./stages/a');\n"
                "module.exports = { out: a.run('  Hello  world  0 false  hello ') };\n",
            "stages/a.js": "const b = require('./b');\nexports.run = (s) => b.run(s.split(' '));\n",
            "stages/b.js": "const c = require('./c');\nexports.run = (arr) => c.run(arr.map(x => x.trim()));\n",
            "stages/c.js": "const d = require('./d');\nexports.run = (arr) => d.run(arr.map(x => x.toLowerCase()));\n",
            "stages/d.js":
                "const e = require('./e');\n"
                "exports.run = (arr) => e.run(arr.filter(x => x.length > 1));  // BUG: drops single-char tokens\n",
            "stages/e.js": "const f = require('./f');\nexports.run = (arr) => f.run([...new Set(arr)]);\n",
            "stages/f.js": "exports.run = (arr) => arr.sort();\n",
        }),
        "test": lambda d: rt(d, "index",
            "if (m.out.join(',') !== '0,false,hello,world') throw new Error('got ' + JSON.stringify(m.out));\n"),
        "_fixed": {"stages/d.js":
            "const e = require('./e');\n"
            "exports.run = (arr) => e.run(arr.filter(x => x !== ''));\n"},
    })
    tasks.append({
        "id": "g3lng-big-utils",
        "category": "gen3-long",
        "prompt": "utils.js contains TWELVE exported functions. Eleven are correct. ONE is buggy: "
                  "clamp(value, min, max) returns max when value is below min (it compares the wrong "
                  "way). Find clamp and fix it so clamp(5, 10, 20) === 10. Do NOT change any other "
                  "function — the hidden test checks all twelve.",
        "setup": lambda d: write(d, {"utils.js":
            "function add(a, b) { return a + b; }\n"
            "function sub(a, b) { return a - b; }\n"
            "function mul(a, b) { return a * b; }\n"
            "function div(a, b) { return a / b; }\n"
            "function clamp(value, min, max) { if (value > max) return min; if (value < min) return max; return value; }  // BUG: inverted branches\n"
            "function abs(a) { return Math.abs(a); }\n"
            "function pow(a, b) { return Math.pow(a, b); }\n"
            "function min2(a, b) { return Math.min(a, b); }\n"
            "function max2(a, b) { return Math.max(a, b); }\n"
            "function round2(a) { return Math.round((a + Number.EPSILON) * 100) / 100; }\n"
            "function floor2(a) { return Math.floor(a); }\n"
            "function ceil2(a) { return Math.ceil(a); }\n"
            "module.exports = { add, sub, mul, div, clamp, abs, pow, min2, max2, round2, floor2, ceil2 };\n"}),
        "test": lambda d: rt(d, "utils",
            "if (m.add(2, 3) !== 5) throw new Error('add');\n"
            "if (m.sub(5, 3) !== 2) throw new Error('sub');\n"
            "if (m.mul(4, 3) !== 12) throw new Error('mul');\n"
            "if (m.div(10, 2) !== 5) throw new Error('div');\n"
            "if (m.clamp(5, 10, 20) !== 10) throw new Error('clamp low got ' + m.clamp(5, 10, 20));\n"
            "if (m.clamp(25, 10, 20) !== 20) throw new Error('clamp high got ' + m.clamp(25, 10, 20));\n"
            "if (m.clamp(15, 10, 20) !== 15) throw new Error('clamp mid');\n"
            "if (m.abs(-5) !== 5) throw new Error('abs');\n"
            "if (m.pow(2, 8) !== 256) throw new Error('pow');\n"
            "if (m.min2(3, 7) !== 3) throw new Error('min2');\n"
            "if (m.max2(3, 7) !== 7) throw new Error('max2');\n"
            "if (m.round2(1.005) !== 1.01) throw new Error('round2');\n"
            "if (m.floor2(3.9) !== 3) throw new Error('floor2');\n"
            "if (m.ceil2(3.1) !== 4) throw new Error('ceil2');\n"),
        "_fixed": {"utils.js":
            "function add(a, b) { return a + b; }\n"
            "function sub(a, b) { return a - b; }\n"
            "function mul(a, b) { return a * b; }\n"
            "function div(a, b) { return a / b; }\n"
            "function clamp(value, min, max) { if (value < min) return min; if (value > max) return max; return value; }\n"
            "function abs(a) { return Math.abs(a); }\n"
            "function pow(a, b) { return Math.pow(a, b); }\n"
            "function min2(a, b) { return Math.min(a, b); }\n"
            "function max2(a, b) { return Math.max(a, b); }\n"
            "function round2(a) { return Math.round((a + Number.EPSILON) * 100) / 100; }\n"
            "function floor2(a) { return Math.floor(a); }\n"
            "function ceil2(a) { return Math.ceil(a); }\n"
            "module.exports = { add, sub, mul, div, clamp, abs, pow, min2, max2, round2, floor2, ceil2 };\n"},
    })
    tasks.append({
        "id": "g3lng-config-spread",
        "category": "gen3-long",
        "prompt": "An Express app reads its config from config/index.js which merges THREE partial "
                  "config files (base.json then env.json then local.json — later files override "
                  "earlier ones). The app expects { name: 'myapp', port: 9090, debug: false } — "
                  "local.json must override the port to 9090. But one partial has a TYPO ('potr' "
                  "instead of 'port'), so the override never lands and cfg.port stays 8080. Find the "
                  "typo across the config/ directory and fix it. config/index.js and server.js must "
                  "not need changes.",
        "setup": express_setup({
            "config/index.js":
                "const fs = require('fs');\n"
                "const path = require('path');\n"
                "const dir = __dirname;\n"
                "const parts = ['base.json', 'env.json', 'local.json'];\n"
                "const cfg = {};\n"
                "for (const p of parts) {\n"
                "  const raw = fs.readFileSync(path.join(dir, p), 'utf8');\n"
                "  Object.assign(cfg, JSON.parse(raw));\n"
                "}\n"
                "module.exports = cfg;\n",
            "config/base.json": '{\n  "name": "myapp",\n  "port": 8080\n}\n',
            "config/env.json": '{\n  "debug": false\n}\n',
            "config/local.json": '{\n  "potr": 9090\n}\n',  # BUG: typo'd key, override never lands
            "server.js": _expr(
                "const cfg = require('./config');\n"
                "app.get('/api/app', (req, res) => res.json({ name: cfg.name, port: cfg.port, debug: cfg.debug }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/app')).json();\n"
            "    if (j.name !== 'myapp' || j.port !== 9090 || j.debug !== false) throw new Error('got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"config/local.json": '{\n  "port": 9090\n}\n'},
    })
    tasks.append({
        "id": "g3lng-many-routes",
        "category": "gen3-long",
        "prompt": "An Express app has TWELVE routes spread across three files (routes/items.js, "
                  "routes/users.js, routes/misc.js) plus server.js mounting them. Eleven routes work; "
                  "ONE has a bug: GET /api/users returns the WRONG data (it returns the items list "
                  "instead of the users list — a copy-paste in routes/users.js). Find and fix it. "
                  "All twelve routes must work.",
        "setup": express_setup({
            "routes/items.js":
                "const express = require('express');\n"
                "const r = express.Router();\n"
                "const items = [{ id: 1, name: 'apple' }];\n"
                "r.get('/api/items', (req, res) => res.json(items));\n"
                "r.post('/api/items', (req, res) => { items.push({ id: items.length + 1, name: req.body.name }); res.status(201).json({ ok: true }); });\n"
                "r.get('/api/items/:id', (req, res) => res.json(items.find(i => i.id === Number(req.params.id)) || null));\n"
                "module.exports = r;\n",
            "routes/users.js":
                "const express = require('express');\n"
                "const r = express.Router();\n"
                "const items = [{ id: 1, name: 'apple' }];  // BUG: copy-paste — this is the items list\n"
                "const users = [{ id: 1, name: 'alice' }];\n"
                "r.get('/api/users', (req, res) => res.json(items));  // BUG: returns items\n"
                "r.post('/api/users', (req, res) => { users.push({ id: users.length + 1, name: req.body.name }); res.status(201).json({ ok: true }); });\n"
                "r.get('/api/users/:id', (req, res) => res.json(users.find(u => u.id === Number(req.params.id)) || null));\n"
                "module.exports = r;\n",
            "routes/misc.js":
                "const express = require('express');\n"
                "const r = express.Router();\n"
                "r.get('/api/health', (req, res) => res.json({ status: 'ok' }));\n"
                "r.get('/api/version', (req, res) => res.json({ version: '1.0' }));\n"
                "r.get('/api/time', (req, res) => res.json({ ts: Date.now() }));\n"
                "r.get('/api/status', (req, res) => res.json({ up: true }));\n"
                "r.get('/api/ping', (req, res) => res.json({ pong: true }));\n"
                "r.get('/api/info', (req, res) => res.json({ info: 'x' }));\n"
                "module.exports = r;\n",
            "server.js": _expr(
                "app.use(require('./routes/items'));\n"
                "app.use(require('./routes/users'));\n"
                "app.use(require('./routes/misc'));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/users')).json();\n"
            "    if (!Array.isArray(j) || j[0].name !== 'alice') throw new Error('users got ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(j) || j[0].name !== 'apple') throw new Error('items got ' + JSON.stringify(j));\n"
            "    for (const p of ['health', 'version', 'status', 'ping', 'info']) {\n"
            "      const r = await fetch(base + '/api/' + p);\n"
            "      if (r.status !== 200) throw new Error(p + ' -> ' + r.status);\n"
            "    }\n"
            "    let r = await fetch(base + '/api/time');\n"
            "    if (r.status !== 200) throw new Error('time');\n"
            "    r = await fetch(base + '/api/items/1');\n"
            "    if (r.status !== 200) throw new Error('item by id');\n"
            "    r = await fetch(base + '/api/users/1');\n"
            "    if (r.status !== 200) throw new Error('user by id');\n"
        )),
        "_fixed": {"routes/users.js":
            "const express = require('express');\n"
            "const r = express.Router();\n"
            "const users = [{ id: 1, name: 'alice' }];\n"
            "r.get('/api/users', (req, res) => res.json(users));\n"
            "r.post('/api/users', (req, res) => { users.push({ id: users.length + 1, name: req.body.name }); res.status(201).json({ ok: true }); });\n"
            "r.get('/api/users/:id', (req, res) => res.json(users.find(u => u.id === Number(req.params.id)) || null));\n"
            "module.exports = r;\n"},
    })
    tasks.append({
        "id": "g3lng-multi-state",
        "category": "gen3-long",
        "prompt": "A multi-module counter service: state/ folder has counter.js (the shared counter) "
                  "and THREE consumers (api/users.js, api/stats.js, api/health.js) that all call "
                  "counter.inc() and counter.get(). The bug: api/stats.js requires './counter' "
                  "WRONG — it requires a RELATIVE path that resolves to its OWN directory "
                  "(require('./counter') inside api/), creating a SECOND, independent counter. So "
                  "increments via /api/stats/inc never show up in /api/users/count. Fix the require "
                  "path in api/stats.js to point at the real shared counter.",
        "setup": express_setup({
            "state/counter.js":
                "let count = 0;\n"
                "exports.inc = function () { count += 1; };\n"
                "exports.get = function () { return count; };\n",
            "api/users.js":
                "const express = require('express');\n"
                "const counter = require('../state/counter');\n"
                "const r = express.Router();\n"
                "r.post('/api/users/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
                "r.get('/api/users/count', (req, res) => res.json({ count: counter.get() }));\n"
                "module.exports = r;\n",
            "api/stats.js":
                "const express = require('express');\n"
                "const counter = require('./counter');  // BUG: wrong path — resolves to api/counter.js (missing) or local shadow\n"
                "const r = express.Router();\n"
                "r.post('/api/stats/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
                "module.exports = r;\n",
            "api/health.js":
                "const express = require('express');\n"
                "const counter = require('../state/counter');\n"
                "const r = express.Router();\n"
                "r.get('/api/health/count', (req, res) => res.json({ count: counter.get() }));\n"
                "module.exports = r;\n",
            "server.js": _expr(
                "app.use(require('./api/users'));\n"
                "app.use(require('./api/stats'));\n"
                "app.use(require('./api/health'));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    await fetch(base + '/api/stats/inc', { method: 'POST' });\n"
            "    let j = await (await fetch(base + '/api/users/count')).json();\n"
            "    if (j.count !== 1) throw new Error('shared count got ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/health/count')).json();\n"
            "    if (j.count !== 1) throw new Error('health count got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"api/stats.js":
            "const express = require('express');\n"
            "const counter = require('../state/counter');\n"
            "const r = express.Router();\n"
            "r.post('/api/stats/inc', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
            "module.exports = r;\n"},
    })

    return tasks
