"""gen7_ops_tasks.py — 20-task "gen7-ops" suite: local Terminal-Bench-style tasks.

The agent must OPERATE a real shell — run the app, run tests, read crash logs —
to diagnose and fix broken projects (no Docker needed). Prompts are written in
pure user voice: short, vague, no file names, no hints. The hidden tests verify
the project is actually operable again (npm test passes, the server boots, the
report produces the right numbers, exit codes are correct, secrets stop leaking).

Helper keys used from H: write, ensure_deps, require_test (rt), require_async_test
(rat), api_test (api), py_test (py), ops_test (ops), file_test (ft).

Every task carries `_fixed` (preflight-verified correct version); the agent never
sees it.
"""


def _pkg(deps='"express": "^4.19.0"'):
    return '{\n  "name": "ops-app",\n  "main": "server.js",\n  "scripts": {\n    "start": "node server.js",\n    "test": "node test.js"\n  },\n  "dependencies": { ' + deps + ' }\n}\n'


def _wbytes(cwd, rel, data):
    """Write raw bytes (text fixtures with a real encoding like UTF-16LE)."""
    import os as _os
    full = _os.path.join(cwd, rel)
    _os.makedirs(_os.path.dirname(full), exist_ok=True)
    with open(full, "wb") as f:
        f.write(data)


def _expr(extra):
    return (
        "const express = require('express');\n"
        "const app = express();\n"
        "app.use(express.json());\n"
        + extra +
        "\nmodule.exports = app;\n"
    )


def build_tasks(H):
    write = H["write"]
    ensure_deps = H["ensure_deps"]
    rt = H["require_test"]
    api = H["api_test"]
    py = H["py_test"]
    ops = H["ops_test"]

    def express_setup(files):
        return lambda d: (write(d, files), ensure_deps(d))

    tasks = []

    # ── 1. boot crash: syntax error in a router file ──
    tasks.append({
        "id": "g7o-boot-crash",
        "category": "gen7-ops",
        "prompt": "The app won't start at all after my last change — it dies immediately. I can't "
                  "see why.",
        "setup": express_setup({
            "routes.js":
                "const express = require('express');\n"
                "const router = express.Router();\n"
                "router.get('/api/health', (req, res) => {\n"
                "  res.json({ ok: true });\n"
                "// BUG: unclosed — missing the closing '});' and module.exports\n",
            "server.js": _expr(
                "app.use(require('./routes'));\n"
                "app.get('/api/ping', (req, res) => res.json({ pong: true }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/health')).json();\n"
            "    if (j.ok !== true) throw new Error('health got ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/ping')).json();\n"
            "    if (j.pong !== true) throw new Error('ping got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {
            "routes.js":
                "const express = require('express');\n"
                "const router = express.Router();\n"
                "router.get('/api/health', (req, res) => {\n"
                "  res.json({ ok: true });\n"
                "});\n"
                "module.exports = router;\n",
        },
    })

    # ── 2. missing dependency: code requires lodash, package.json lacks it ──
    tasks.append({
        "id": "g7o-missing-dep",
        "category": "gen7-ops",
        "prompt": "It worked on my machine yesterday, but now the app crashes on startup saying "
                  "something isn't installed. Please get it running.",
        "setup": express_setup({
            "server.js": _expr(
                "const _ = require('lodash');\n"
                "app.get('/api/name', (req, res) => res.json({ name: _.upperFirst((req.query.name || '').toLowerCase()) }));\n"
                "app.get('/api/health', (req, res) => res.json({ ok: true }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/name?name=bob')).json();\n"
            "    if (j.name !== 'Bob') throw new Error('name got ' + JSON.stringify(j));\n"
            "    j = await (await fetch(base + '/api/health')).json();\n"
            "    if (j.ok !== true) throw new Error('health');\n"
        )),
        "_fixed": {"package.json": _pkg('"express": "^4.19.0", "lodash": "^4.17.21"')},
    })

    # ── 3. broken npm test script (typo in the script) ──
    tasks.append({
        "id": "g7o-broken-npm-test",
        "category": "gen7-ops",
        "prompt": "I can't run the tests anymore — the command fails right away with an error I "
                  "don't understand.",
        "setup": lambda d: write(d, {
            "package.json":
                '{\n  "name": "ops-app",\n  "scripts": { "test": "node --tes ./test/test.js" }\n}\n',
            "utils.js": "function compute(x) { return x * 2; }\nmodule.exports = { compute };\n",
            "test/test.js":
                "const { compute } = require('../utils');\n"
                "if (compute(3) !== 6) { console.error('FAIL'); process.exit(1); }\n"
                "console.log('PASS');\n",
        }),
        "test": lambda d: ops(d, "npm test", must_contain="PASS"),
        "_fixed": {"package.json":
            '{\n  "name": "ops-app",\n  "scripts": { "test": "node ./test/test.js" }\n}\n'},
    })

    # ── 4. health check reports a hardcoded port instead of the configured one ──
    tasks.append({
        "id": "g7o-wrong-port-config",
        "category": "gen7-ops",
        "prompt": "The health check reports the wrong port — it says 3000 but we configured 8080.",
        "setup": express_setup({
            "config.json": '{\n  "port": 8080\n}\n',
            "server.js": _expr(
                "const config = require('./config.json');\n"
                "const actualPort = 3000;  // BUG: hardcoded, ignores config\n"
                "app.get('/api/health', (req, res) => res.json({ port: actualPort }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/health')).json();\n"
            "    if (j.port !== 8080) throw new Error('port got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const config = require('./config.json');\n"
            "const actualPort = config.port;\n"
            "app.get('/api/health', (req, res) => res.json({ port: actualPort }));\n")},
    })

    # ── 5. debug flag reads the env instead of the config file ──
    tasks.append({
        "id": "g7o-env-flag",
        "category": "gen7-ops",
        "prompt": "Debug mode won't turn on. I put it in the config file like the docs say, but it "
                  "stays off.",
        "setup": express_setup({
            "config.json": '{\n  "debug": true\n}\n',
            "server.js": _expr(
                "const config = require('./config.json');\n"
                "const debug = process.env.DEBUG_MODE || false;  // BUG: env, not config\n"
                "app.get('/api/debug', (req, res) => res.json({ debug: !!debug }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/debug')).json();\n"
            "    if (j.debug !== true) throw new Error('debug got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const config = require('./config.json');\n"
            "const debug = config.debug;\n"
            "app.get('/api/debug', (req, res) => res.json({ debug: !!debug }));\n")},
    })

    # ── 6. package.json main points at a file that doesn't exist ──
    tasks.append({
        "id": "g7o-wrong-entry",
        "category": "gen7-ops",
        "prompt": "npm start fails with module not found even though the app file exists. It used "
                  "to start fine.",
        "setup": lambda d: (write(d, {
            "package.json":
                '{\n  "name": "ops-app",\n  "main": "start.js",\n  "dependencies": { "express": "^4.19.0" }\n}\n',
            "server.js":
                "const express = require('express');\n"
                "const app = express();\n"
                "app.get('/api/health', (req, res) => res.json({ ok: true }));\n"
                "app.listen(3000, () => console.log('listening on 3000'));\n",
        }), ensure_deps(d)),
        "test": lambda d: ops(d, "node .", must_contain="listening on 3000", timeout=20),
        "_fixed": {"package.json":
            '{\n  "name": "ops-app",\n  "main": "server.js",\n  "dependencies": { "express": "^4.19.0" }\n}\n'},
    })

    # ── 7. utils.js has a syntax error; the test script dies instantly ──
    tasks.append({
        "id": "g7o-syntax-utils",
        "category": "gen7-ops",
        "prompt": "The test run dies instantly with a parse error. Nothing in the output makes "
                  "sense to me.",
        "setup": lambda d: write(d, {
            "utils.js":
                "function compute(x) { return x * 2; \n"  # BUG: unclosed brace
                "module.exports = { compute };\n",
            "test.js":
                "const { compute } = require('./utils');\n"
                "if (compute(3) !== 6) { console.error('FAIL'); process.exit(1); }\n"
                "console.log('PASS');\n",
        }),
        "test": lambda d: ops(d, "node test.js", must_contain="PASS"),
        "_fixed": {"utils.js":
            "function compute(x) { return x * 2; }\n"
            "module.exports = { compute };\n"},
    })

    # ── 8. crash-log diagnosis: helper references an undefined variable ──
    tasks.append({
        "id": "g7o-log-diagnose",
        "category": "gen7-ops",
        "prompt": "The pipeline crashes when it runs — I attached the log from the last run. "
                  "Please make it work.",
        "setup": lambda d: write(d, {
            "settings.js": "module.exports = { multiplier: 3 };\n",
            "helper.js":
                "function compute(x) { return x * multiplier; }  // BUG: multiplier never imported\n"
                "module.exports = { compute };\n",
            "index.js":
                "const { compute } = require('./helper');\n"
                "module.exports = { result: compute(2) };\n",
            "debug.log":
                "[09:12:44] ERROR ReferenceError: multiplier is not defined\n"
                "[09:12:44]        at compute (helper.js:1:32)\n"
                "[09:12:44]        at Object.<anonymous> (index.js:2)\n",
        }),
        "test": lambda d: rt(d, "index",
            "if (m.result !== 6) throw new Error('got ' + m.result);\n"),
        "_fixed": {"helper.js":
            "const { multiplier } = require('./settings');\n"
            "function compute(x) { return x * multiplier; }\n"
            "module.exports = { compute };\n"},
    })

    # ── 9. config.json has a trailing comma (invalid JSON, boot crash) ──
    tasks.append({
        "id": "g7o-bad-json-config",
        "category": "gen7-ops",
        "prompt": "The app won't start after the last change. Something in the config seems off "
                  "but I can't spot it.",
        "setup": express_setup({
            "config.json": '{\n  "name": "myapp",\n  "port": 8080,\n}\n',  # invalid: trailing comma
            "server.js": _expr(
                "const config = require('./config.json');\n"
                "app.get('/api/app', (req, res) => res.json({ name: config.name, port: config.port }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/app')).json();\n"
            "    if (j.name !== 'myapp' || j.port !== 8080) throw new Error('got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"config.json": '{\n  "name": "myapp",\n  "port": 8080\n}\n'},
    })

    # ── 10. report skips rows with empty fields (undercounts) ──
    tasks.append({
        "id": "g7o-csv-empty",
        "category": "gen7-ops",
        "prompt": "The report undercounts — rows where a field is empty are missing from the "
                  "totals. Those rows should still count with zero for the empty field.",
        "setup": lambda d: write(d, {
            "data.csv":
                "name,amount\n"
                "a,10\n"
                "b,\n"
                "c,30\n",
            "process.py":
                "import csv\n"
                "def compute_total(path='data.csv'):\n"
                "    total = 0\n"
                "    n = 0\n"
                "    with open(path, newline='') as f:\n"
                "        for row in csv.DictReader(f):\n"
                "            if not row['amount']:\n"
                "                continue  # BUG: skips rows with an empty field\n"
                "            total += float(row['amount'])\n"
                "            n += 1\n"
                "    return total, n\n",
        }),
        "test": lambda d: py(d, "process",
            "s, n = m.compute_total()\n"
            "assert s == 40, s\n"
            "assert n == 3, n\n"),
        "_fixed": {"process.py":
            "import csv\n"
            "def compute_total(path='data.csv'):\n"
            "    total = 0\n"
            "    n = 0\n"
            "    with open(path, newline='') as f:\n"
            "        for row in csv.DictReader(f):\n"
            "            total += float(row['amount']) if row['amount'] else 0.0\n"
            "            n += 1\n"
            "    return total, n\n"},
    })

    # ── 11. wrong router mounted last shadows the real one → empty list ──
    tasks.append({
        "id": "g7o-dup-mount",
        "category": "gen7-ops",
        "prompt": "Since the last update the items page shows an empty list even though the data "
                  "is definitely there.",
        "setup": express_setup({
            "routes/a.js":
                "const express = require('express');\n"
                "const r = express.Router();\n"
                "const items = [];\n"
                "r.get('/api/items', (req, res) => res.json(items));\n"
                "module.exports = r;\n",
            "routes/b.js":
                "const express = require('express');\n"
                "const r = express.Router();\n"
                "const items = [{ id: 1, name: 'apple' }, { id: 2, name: 'banana' }];\n"
                "r.get('/api/items', (req, res) => res.json(items));\n"
                "module.exports = r;\n",
            "server.js": _expr(
                "app.use(require('./routes/a'));  // BUG: empty router mounted first wins\n"
                "app.use(require('./routes/b'));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/items')).json();\n"
            "    if (!Array.isArray(j) || j.length !== 2) throw new Error('got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "app.use(require('./routes/b'));\n")},
    })

    # ── 12. feature flag reads the wrong env var name ──
    tasks.append({
        "id": "g7o-flag-name",
        "category": "gen7-ops",
        "prompt": "The feature flag does nothing — I set it in the config but the feature stays "
                  "off.",
        "setup": express_setup({
            "config.json": '{\n  "feature_enabled": true\n}\n',
            "server.js": _expr(
                "const config = require('./config.json');\n"
                "const enabled = process.env.FEATURE_X === '1';  // BUG: wrong flag source\n"
                "app.get('/api/feature', (req, res) => res.json({ enabled }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/feature')).json();\n"
            "    if (j.enabled !== true) throw new Error('got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const config = require('./config.json');\n"
            "const enabled = config.feature_enabled;\n"
            "app.get('/api/feature', (req, res) => res.json({ enabled }));\n")},
    })

    # ── 13. daily report uses local time; the day flips wrong at midnight ──
    tasks.append({
        "id": "g7o-tz-report",
        "category": "gen7-ops",
        "prompt": "The daily report assigns sales to the wrong day right around midnight.",
        "setup": lambda d: write(d, {
            "report.py":
                "from datetime import datetime\n"
                "def sale_day(ts_ms):\n"
                "    return datetime.fromtimestamp(ts_ms / 1000).strftime('%Y-%m-%d')  # BUG: local tz\n",
        }),
        "test": lambda d: py(d, "report",
            "assert m.sale_day(1768262399000) == '2026-01-12', 'expected 2026-01-12, got ' + str(m.sale_day(1768262399000))\n"
            "assert m.sale_day(1768262400000) == '2026-01-13', 'expected 2026-01-13, got ' + str(m.sale_day(1768262400000))\n"),
        "_fixed": {"report.py":
            "from datetime import datetime\n"
            "def sale_day(ts_ms):\n"
            "    return datetime.utcfromtimestamp(ts_ms / 1000).strftime('%Y-%m-%d')\n"},
    })

    # ── 14. invoice totals have floating-point noise ──
    tasks.append({
        "id": "g7o-float-money",
        "category": "gen7-ops",
        "prompt": "Invoices show long ugly decimals like 0.30000000000000004 in the totals.",
        "setup": lambda d: write(d, {
            "billing.js":
                "function total(prices) { return prices.reduce((a, b) => a + b, 0); }  // BUG: no rounding\n"
                "module.exports = { total };\n",
        }),
        "test": lambda d: rt(d, "billing",
            "if (m.total([0.1, 0.2]) !== 0.3) throw new Error('got ' + m.total([0.1, 0.2]));\n"
            "if (m.total([0.7, 0.1]) !== 0.8) throw new Error('got ' + m.total([0.7, 0.1]));\n"
            "if (m.total([1.5, 2.25]) !== 3.75) throw new Error('plain');\n"),
        "_fixed": {"billing.js":
            "function total(prices) { return Math.round((prices.reduce((a, b) => a + b, 0) + Number.EPSILON) * 100) / 100; }\n"
            "module.exports = { total };\n"},
    })

    # ── 15. validation script exits 0 on failure → CI falsely green ──
    tasks.append({
        "id": "g7o-exit-zero",
        "category": "gen7-ops",
        "prompt": "Our CI says the validation passed even when it actually failed — the check "
                  "script must not report success on failure.",
        "setup": lambda d: write(d, {
            "check.js":
                "const fs = require('fs');\n"
                "const file = process.argv[2];\n"
                "const data = JSON.parse(fs.readFileSync(file, 'utf8'));\n"
                "if (!data.name || data.name.trim() === '') { console.log('INVALID'); return; }  // BUG: exits 0\n"
                "console.log('OK');\n",
            "good.json": '{\n  "name": "app"\n}\n',
            "bad.json": '{\n  "name": ""\n}\n',
        }),
        "test": lambda d: (lambda ops: (lambda a, b: (a[0] and b[0], a[1] + " | " + b[1]))(
            ops(d, "node check.js good.json", expect_rc=0),
            ops(d, "node check.js bad.json", expect_rc=1)))(ops),
        "_fixed": {"check.js":
            "const fs = require('fs');\n"
            "const file = process.argv[2];\n"
            "const data = JSON.parse(fs.readFileSync(file, 'utf8'));\n"
            "if (!data.name || data.name.trim() === '') { console.log('INVALID'); process.exit(1); }\n"
            "console.log('OK');\n"},
    })

    # ── 16. logger writes a typo'd JSON key; the parser chokes on every line ──
    tasks.append({
        "id": "g7o-json-log-key",
        "category": "gen7-ops",
        "prompt": "The log parser breaks on every single line — the records are unreadable.",
        "setup": lambda d: write(d, {
            "logger.js":
                "const fs = require('fs');\n"
                "const rec = { ts: Date.now(), lvl: 'info', mssg: 'hi' };  // BUG: typo'd key\n"
                "fs.writeFileSync('_out.jsonl', JSON.stringify(rec) + '\\n');\n"
                "console.log('logged');\n",
            "check_log.py":
                "import json, sys\n"
                "rec = json.load(open('_out.jsonl', encoding='utf-8'))\n"
                "if 'msg' not in rec:\n"
                "    print('FAIL: no msg key in', list(rec.keys()))\n"
                "    sys.exit(1)\n"
                "if rec['msg'] != 'hi':\n"
                "    print('FAIL: wrong msg')\n"
                "    sys.exit(1)\n"
                "print('PASS')\n",
        }),
        "test": lambda d: ops(d, "node logger.js && python check_log.py", must_contain="PASS"),
        "_fixed": {"logger.js":
            "const fs = require('fs');\n"
            "const rec = { ts: Date.now(), lvl: 'info', msg: 'hi' };\n"
            "fs.writeFileSync('_out.jsonl', JSON.stringify(rec) + '\\n');\n"
            "console.log('logged');\n"},
    })

    # ── 17. visitor counter increments twice per request ──
    tasks.append({
        "id": "g7o-dup-increment",
        "category": "gen7-ops",
        "prompt": "The visitor counter always shows double the real visits.",
        "setup": express_setup({
            "counter.js":
                "let count = 0;\n"
                "exports.inc = function () { count += 1; };\n"
                "exports.get = function () { return count; };\n",
            "server.js": _expr(
                "const counter = require('./counter');\n"
                "app.post('/api/visit', (req, res) => { counter.inc(); counter.inc(); res.json({ ok: true }); });  // BUG: double increment\n"
                "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    await fetch(base + '/api/visit', { method: 'POST' });\n"
            "    await fetch(base + '/api/visit', { method: 'POST' });\n"
            "    let j = await (await fetch(base + '/api/count')).json();\n"
            "    if (j.count !== 2) throw new Error('got ' + JSON.stringify(j));\n"
        )),
        "_fixed": {"server.js": _expr(
            "const counter = require('./counter');\n"
            "app.post('/api/visit', (req, res) => { counter.inc(); res.json({ ok: true }); });\n"
            "app.get('/api/count', (req, res) => res.json({ count: counter.get() }));\n")},
    })

    # ── 18. secret leaks into a public response ──
    tasks.append({
        "id": "g7o-secret-leak",
        "category": "gen7-ops",
        "prompt": "Our API key is showing up in a public response. Remove it — we can't leak that.",
        "setup": express_setup({
            "config.json": '{\n  "apiKey": "sk-12345-secret",\n  "name": "app"\n}\n',
            "server.js": _expr(
                "const config = require('./config.json');\n"
                "app.get('/api/info', (req, res) => res.json({ name: config.name, apiKey: config.apiKey }));  // BUG: leaks secret\n"),
            "package.json": _pkg(),
        }),
        "test": lambda d: api(d, (
            "    let j = await (await fetch(base + '/api/info')).json();\n"
            "    const body = JSON.stringify(j);\n"
            "    if (body.includes('sk-12345')) throw new Error('SECRET LEAKED: ' + body);\n"
            "    if (j.name !== 'app') throw new Error('name got ' + body);\n"
        )),
        "_fixed": {"server.js": _expr(
            "const config = require('./config.json');\n"
            "app.get('/api/info', (req, res) => res.json({ name: config.name }));\n")},
    })

    # ── 19. seed data file saved as UTF-16; the reader shows garbage ──
    tasks.append({
        "id": "g7o-wrong-encoding",
        "category": "gen7-ops",
        "prompt": "The import reads garbage characters instead of the names in the file.",
        "setup": lambda d: (write(d, {
            "reader.py":
                "def read_names(path='seed.csv'):\n"
                "    names = []\n"
                "    for line in open(path, encoding='utf-8'):\n"
                "        parts = line.strip().split(',')\n"
                "        if len(parts) >= 2:\n"
                "            names.append(parts[1])\n"
                "    return names\n",
        }), _wbytes(d, "seed.csv", b"\xff\xfe" + "1,a\n2,b\n".encode("utf-16-le"))),  # real UTF-16LE with BOM
        "test": lambda d: py(d, "reader",
            "names = m.read_names()\n"
            "assert len(names) == 2, names\n"
            "assert 'a' in names and 'b' in names, names\n"),
        "_fixed": {"reader.py":
            "def read_names(path='seed.csv'):\n"
            "    names = []\n"
            "    raw = open(path, 'rb').read()\n"
            "    if raw.startswith(b'\\xff\\xfe') or raw.startswith(b'\\xfe\\xff'):\n"
            "        text = raw.decode('utf-16')\n"
            "    else:\n"
            "        text = raw.decode('utf-8-sig')\n"
            "    for line in text.splitlines():\n"
            "        parts = line.strip().split(',')\n"
            "        if len(parts) >= 2:\n"
            "            names.append(parts[1])\n"
            "    return names\n"},
    })

    # ── 20. cache reads the data file once at load; edits don't show up ──
    tasks.append({
        "id": "g7o-stale-cache",
        "category": "gen7-ops",
        "prompt": "Edits to the data file don't show up until I restart — it keeps serving the old "
                  "values.",
        "setup": lambda d: write(d, {
            "data.json": '{\n  "value": 1\n}\n',
            "cache.js":
                "const data = require('./data.json');  // BUG: read once at load\n"
                "exports.get = function (k) { return data[k]; };\n",
        }),
        "test": lambda d: rt(d, "cache",
            "const fs = require('fs');\n"
            "if (m.get('value') !== 1) throw new Error('initial got ' + m.get('value'));\n"
            "fs.writeFileSync('./data.json', JSON.stringify({ value: 2 }));\n"
            "if (m.get('value') !== 2) throw new Error('stale: ' + m.get('value'));\n"),
        "_fixed": {"cache.js":
            "exports.get = function (k) {\n"
            "  delete require.cache[require.resolve('./data.json')];\n"
            "  return require('./data.json')[k];\n"
            "};\n"},
    })

    return tasks
