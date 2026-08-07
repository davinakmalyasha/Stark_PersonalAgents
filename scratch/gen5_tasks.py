"""gen5_tasks.py — the CAPSTONE: one big real-world enterprise build.

Task `g5-enterprise-portal` asks the full multi-agent team to build an
enterprise customer portal from scratch (greenfield, ~100KB+ codebase scale):

  Tech stack (required): Node.js + Express + JSON Web Tokens + bcrypt +
                         Helmet security headers + input validation.

  The hidden test verifies that EVERY agent participated, not just that the
  app runs:
    - BSA       -> specs/SPEC-*.md exists (spec produced)
    - Dev       -> the API actually works end to end (auth, products, orders)
    - SDET      -> tests/ suite exists AND passes when run (node --test)
    - DevSecOps -> security headers present, login rate-limited, passwords
                   bcrypt-hashed (no plaintext in source)

This stresses the LONG-HORIZON capability: orchestrator planning, Dev
building 12+ files, SDET writing a real test suite, DevSecOps hardening.
"""
import os
import subprocess


def _pkg(deps):
    return '{\n  "name": "enterprise-portal",\n  "main": "server.js",\n  "scripts": { "start": "node server.js", "test": "node --test tests/" },\n  "dependencies": ' + deps + '\n}\n'


# ── Reference implementation (the agent NEVER sees this; preflight uses it) ──
_REF = {
    "server.js":
        "const express = require('express');\n"
        "const helmet = require('helmet');\n"
        "const app = express();\n"
        "app.use(helmet());\n"
        "app.use(express.json());\n"
        "\n"
        "const { register, login } = require('./routes/auth');\n"
        "const { me } = require('./routes/me');\n"
        "const { listProducts, createProduct, getProduct } = require('./routes/products');\n"
        "const { createOrder, listOrders } = require('./routes/orders');\n"
        "const { authRequired, adminRequired } = require('./middleware/auth');\n"
        "const { validateProduct } = require('./middleware/validate');\n"
        "\n"
        "app.get('/health', (req, res) => res.json({ status: 'ok' }));\n"
        "app.post('/api/auth/register', register);\n"
        "app.post('/api/auth/login', login);\n"
        "app.get('/api/me', authRequired, me);\n"
        "app.get('/api/products', listProducts);\n"
        "app.get('/api/products/:id', getProduct);\n"
        "app.post('/api/products', authRequired, adminRequired, validateProduct, createProduct);\n"
        "app.post('/api/orders', authRequired, createOrder);\n"
        "app.get('/api/orders', authRequired, listOrders);\n"
        "\n"
        "module.exports = app;\n"
        "if (require.main === module) { app.listen(process.env.PORT || 3000, () => console.log('portal up')); }\n",
    "config.js":
        "module.exports = {\n"
        "  JWT_SECRET: process.env.JWT_SECRET || 'dev-secret-change-in-prod',\n"
        "  JWT_EXPIRES: '1h',\n"
        "  LOGIN_MAX_FAILS: 5,\n"
        "  LOGIN_WINDOW_MS: 60000,\n"
        "};\n",
    "middleware/auth.js":
        "const jwt = require('jsonwebtoken');\n"
        "const cfg = require('../config');\n"
        "const users = require('../services/users');\n"
        "\n"
        "function authRequired(req, res, next) {\n"
        "  const header = req.headers['authorization'] || '';\n"
        "  const token = header.startsWith('Bearer ') ? header.slice(7) : null;\n"
        "  if (!token) return res.status(401).json({ error: 'no token' });\n"
        "  try {\n"
        "    const payload = jwt.verify(token, cfg.JWT_SECRET);\n"
        "    const user = users.findByEmail(payload.email);\n"
        "    if (!user) return res.status(401).json({ error: 'no user' });\n"
        "    req.user = user;\n"
        "    next();\n"
        "  } catch (e) {\n"
        "    return res.status(401).json({ error: 'invalid token' });\n"
        "  }\n"
        "}\n"
        "\n"
        "function adminRequired(req, res, next) {\n"
        "  if (!req.user || req.user.role !== 'admin') return res.status(403).json({ error: 'admin only' });\n"
        "  next();\n"
        "}\n"
        "\n"
        "module.exports = { authRequired, adminRequired };\n",
    "middleware/validate.js":
        "function validateProduct(req, res, next) {\n"
        "  const { name, price } = req.body || {};\n"
        "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
        "  if (typeof price !== 'number' || !isFinite(price) || price <= 0) return res.status(400).json({ error: 'invalid price' });\n"
        "  next();\n"
        "}\n"
        "module.exports = { validateProduct };\n",
    "services/users.js":
        "const bcrypt = require('bcryptjs');\n"
        "const users = new Map();\n"
        "// Seed admin\n"
        "users.set('admin@example.com', { email: 'admin@example.com', password: bcrypt.hashSync('admin123', 10), name: 'Admin', role: 'admin' });\n"
        "\n"
        "exports.register = function (email, password, name) {\n"
        "  if (users.has(email)) return { error: 'email exists' };\n"
        "  const user = { email, password: bcrypt.hashSync(password, 10), name, role: 'user' };\n"
        "  users.set(email, user);\n"
        "  return { user };\n"
        "};\n"
        "exports.findByEmail = function (email) { return users.get(email) || null; };\n"
        "exports.verify = function (email, password) {\n"
        "  const u = users.get(email);\n"
        "  if (!u) return null;\n"
        "  return bcrypt.compareSync(password, u.password) ? u : null;\n"
        "};\n",
    "services/products.js":
        "const products = [\n"
        "  { id: 1, name: 'Laptop', price: 999.99 },\n"
        "  { id: 2, name: 'Mouse', price: 24.5 },\n"
        "];\n"
        "exports.list = function () { return products; };\n"
        "exports.get = function (id) { return products.find(p => p.id === id) || null; };\n"
        "exports.create = function (name, price) {\n"
        "  const p = { id: products.length + 1, name, price };\n"
        "  products.push(p);\n"
        "  return p;\n"
        "};\n",
    "services/orders.js":
        "const orders = [];\n"
        "exports.create = function (email, productId) {\n"
        "  const o = { id: orders.length + 1, email, productId, at: new Date().toISOString() };\n"
        "  orders.push(o);\n"
        "  return o;\n"
        "};\n"
        "exports.listFor = function (email) { return orders.filter(o => o.email === email); };\n",
    "routes/auth.js":
        "const jwt = require('jsonwebtoken');\n"
        "const cfg = require('../config');\n"
        "const users = require('../services/users');\n"
        "\n"
        "// Per-IP login attempt tracking\n"
        "const attempts = new Map();\n"
        "function checkRateLimit(ip) {\n"
        "  const rec = attempts.get(ip) || { fails: 0, windowStart: Date.now() };\n"
        "  if (Date.now() - rec.windowStart > cfg.LOGIN_WINDOW_MS) { rec.fails = 0; rec.windowStart = Date.now(); }\n"
        "  attempts.set(ip, rec);\n"
        "  return rec.fails >= cfg.LOGIN_MAX_FAILS;\n"
        "}\n"
        "\n"
        "exports.register = function (req, res) {\n"
        "  const { email, password, name } = req.body || {};\n"
        "  if (!email || !/^[^@\\s]+@[^@\\s]+\\.[^@\\s]+$/.test(email)) return res.status(400).json({ error: 'invalid email' });\n"
        "  if (typeof password !== 'string' || password.length < 8) return res.status(400).json({ error: 'password too short' });\n"
        "  if (typeof name !== 'string' || name.trim() === '') return res.status(400).json({ error: 'name required' });\n"
        "  const r = users.register(email, password, name);\n"
        "  if (r.error) return res.status(409).json({ error: r.error });\n"
        "  res.status(201).json({ email: r.user.email, name: r.user.name, role: r.user.role });\n"
        "};\n"
        "\n"
        "exports.login = function (req, res) {\n"
        "  const ip = req.ip || 'unknown';\n"
        "  if (checkRateLimit(ip)) return res.status(429).json({ error: 'too many attempts' });\n"
        "  const { email, password } = req.body || {};\n"
        "  const user = users.verify(email || '', password || '');\n"
        "  if (!user) {\n"
        "    const rec = attempts.get(ip);\n"
        "    rec.fails += 1;\n"
        "    return res.status(401).json({ error: 'invalid credentials' });\n"
        "  }\n"
        "  attempts.delete(ip);\n"
        "  const token = jwt.sign({ email: user.email, role: user.role }, cfg.JWT_SECRET, { expiresIn: cfg.JWT_EXPIRES });\n"
        "  res.json({ token, email: user.email, role: user.role });\n"
        "};\n",
    "routes/me.js":
        "exports.me = function (req, res) { res.json({ email: req.user.email, name: req.user.name, role: req.user.role }); };\n",
    "routes/products.js":
        "const products = require('../services/products');\n"
        "exports.listProducts = (req, res) => res.json(products.list());\n"
        "exports.getProduct = (req, res) => {\n"
        "  const p = products.get(Number(req.params.id));\n"
        "  if (!p) return res.status(404).json({ error: 'not found' });\n"
        "  res.json(p);\n"
        "};\n"
        "exports.createProduct = (req, res) => {\n"
        "  const p = products.create(req.body.name.trim(), req.body.price);\n"
        "  res.status(201).json(p);\n"
        "};\n",
    "routes/orders.js":
        "const orders = require('../services/orders');\n"
        "const products = require('../services/products');\n"
        "exports.createOrder = (req, res) => {\n"
        "  const productId = Number((req.body || {}).productId);\n"
        "  if (!products.get(productId)) return res.status(400).json({ error: 'unknown product' });\n"
        "  const o = orders.create(req.user.email, productId);\n"
        "  res.status(201).json(o);\n"
        "};\n"
        "exports.listOrders = (req, res) => res.json(orders.listFor(req.user.email));\n",
    "tests/portal.test.js":
        "const { test } = require('node:test');\n"
        "const assert = require('node:assert');\n"
        "const app = require('../server');\n"
        "let server, base;\n"
        "test.before(async () => { server = app.listen(0); base = 'http://127.0.0.1:' + server.address().port; });\n"
        "test.after(() => server.close());\n"
        "async function post(path, body, token) {\n"
        "  const h = { 'Content-Type': 'application/json' };\n"
        "  if (token) h['Authorization'] = 'Bearer ' + token;\n"
        "  return fetch(base + path, { method: 'POST', headers: h, body: JSON.stringify(body || {}) });\n"
        "}\n"
        "test('health', async () => { const r = await fetch(base + '/health'); assert.strictEqual(r.status, 200); });\n"
        "test('register+login+me', async () => {\n"
        "  const r1 = await post('/api/auth/register', { email: 'bob@example.com', password: 'password123', name: 'Bob' });\n"
        "  assert.strictEqual(r1.status, 201);\n"
        "  const r2 = await post('/api/auth/login', { email: 'bob@example.com', password: 'password123' });\n"
        "  assert.strictEqual(r2.status, 200);\n"
        "  const { token } = await r2.json();\n"
        "  const r3 = await fetch(base + '/api/me', { headers: { 'Authorization': 'Bearer ' + token } });\n"
        "  assert.strictEqual(r3.status, 200);\n"
        "  const me = await r3.json();\n"
        "  assert.strictEqual(me.email, 'bob@example.com');\n"
        "});\n"
        "test('auth required', async () => { const r = await fetch(base + '/api/me'); assert.strictEqual(r.status, 401); });\n"
        "test('rate limit login', async () => {\n"
        "  for (let i = 0; i < 5; i++) await post('/api/auth/login', { email: 'nobody@example.com', password: 'wrongpass' });\n"
        "  const r = await post('/api/auth/login', { email: 'nobody@example.com', password: 'wrongpass' });\n"
        "  assert.strictEqual(r.status, 429);\n"
        "});\n"
        "test('security headers', async () => {\n"
        "  const r = await fetch(base + '/health');\n"
        "  assert.ok(r.headers.get('x-content-type-options') === 'nosniff');\n"
        "});\n",
    "README.md":
        "# Enterprise Portal\n\nCustomer portal API with JWT auth, products, orders.\n\n"
        "Run: `npm start` · Tests: `npm test`\n",
    "specs/SPEC-enterprise-portal.md":
        "# SPEC — Enterprise Customer Portal\n\n"
        "## Architecture\n"
        "Express app; routes/ for HTTP, services/ for business logic, middleware/ for auth + validation.\n\n"
        "## API Contract\n"
        "- POST /api/auth/register {email, password, name} → 201 / 400 / 409\n"
        "- POST /api/auth/login {email, password} → {token} / 401 / 429 (5 fails/IP)\n"
        "- GET /api/me → {email, name, role} (Bearer, 401 without)\n"
        "- GET /api/products, GET /api/products/:id, POST /api/products (admin, 403 non-admin)\n"
        "- POST /api/orders {productId}, GET /api/orders (own orders only)\n\n"
        "## Security Plan\n"
        "- bcrypt password hashing (never plaintext)\n"
        "- Helmet headers on every response\n"
        "- JWT with role claims, 1h expiry, secret from config/env\n"
        "- Login rate limiting per IP (5 fails → 429)\n"
        "- Input validation on all write endpoints\n",
}


def build_tasks(H):
    write = H["write"]
    ensure_deps = H["ensure_deps"]
    rt = H["require_test"]
    rat = H["require_async_test"]
    api = H["api_test"]

    tasks = []

    # ══════════════════════════════════════════════════════════════════════
    # gen5 — THE CAPSTONE: one big enterprise build, all agents required
    # ══════════════════════════════════════════════════════════════════════
    starter_deps = '{\n  "express": "^4.19.0",\n  "helmet": "^7.1.0",\n  "jsonwebtoken": "^9.0.2",\n  "bcryptjs": "^2.4.3"\n}'
    tasks.append({
        "id": "g5-enterprise-portal",
        "category": "gen5",
        "prompt": (
            "BUILD A COMPLETE ENTERPRISE CUSTOMER PORTAL WEB APP in this EMPTY project. This is a "
            "full greenfield build — design it, implement it, test it, and harden it.\n\n"
            "## TECH STACK (required)\n"
            "- Node.js + Express (deps available: express, helmet, jsonwebtoken, bcryptjs)\n"
            "- JWT-based auth with role claims (user/admin)\n"
            "- bcrypt password hashing (never store plaintext)\n"
            "- Helmet security headers on every response\n"
            "- Input validation on all write endpoints\n\n"
            "## REQUIREMENTS\n"
            "### Auth\n"
            "- POST /api/auth/register {email, password, name} → 201; validate email format, "
            "password >= 8 chars, name non-empty (400 on invalid); 409 if email exists\n"
            "- POST /api/auth/login {email, password} → {token, email, role}; 401 on bad "
            "credentials; RATE LIMIT: after 5 failed attempts from one IP, the 6th must be 429\n"
            "- GET /api/me (Bearer token) → {email, name, role}; 401 without valid token\n"
            "- Pre-seed an admin: admin@example.com / admin123 (role admin)\n"
            "### Products\n"
            "- GET /api/products → list; GET /api/products/:id → item or 404\n"
            "- POST /api/products (ADMIN only, validate name + price>0) → 201; 401 no token, "
            "403 non-admin, 400 bad input\n"
            "### Orders\n"
            "- POST /api/orders {productId} (any logged-in user) → 201; 400 unknown product\n"
            "- GET /api/orders → ONLY the caller's orders\n"
            "### Ops\n"
            "- GET /health → {status:'ok'}\n"
            "- server.js must module.exports = app and only listen when run directly\n"
            "- package.json with start + test scripts (node --test tests/)\n\n"
            "## PROCESS (use your full team)\n"
            "1. BSA: write a specs/SPEC-*.md design doc first (architecture, API contract, "
            "security plan)\n"
            "2. Dev: implement the full app\n"
            "3. SDET: write a real test suite in tests/ (node:test) covering auth, products, "
            "orders, validation, rate limiting, security headers — then RUN it and make it pass\n"
            "4. DevSecOps: security review — verify no plaintext passwords, secrets not "
            "hardcoded (read from config/env), headers present, rate limiting active\n"
            "5. README.md with run instructions\n\n"
            "A hidden acceptance suite will start the server, exercise every endpoint, check "
            "security headers, check the spec doc exists, and RUN your tests/ suite. All of it "
            "must pass."
        ),
        "setup": lambda d: (write(d, {"package.json": _pkg(starter_deps)}), ensure_deps(d)),
        "test": lambda d: _capstone_test(d, api, H),
        "_fixed": dict(_REF),
    })

    return tasks


def _capstone_test(d, api, H):
    """Capstone hidden test: (1) API behavior, (2) security headers, (3) agent artifacts:
    specs/ (BSA), tests/ runnable+green (SDET), bcrypt/helmet in source (DevSecOps)."""
    ok, detail = api(d, (
        "    let r = await fetch(base + '/health');\n"
        "    if (r.status !== 200) throw new Error('health ' + r.status);\n"
        "    let h = r.headers.get('x-content-type-options');\n"
        "    if (h !== 'nosniff') throw new Error('missing security header xcto: ' + h);\n"
        "    // register\n"
        "    r = await fetch(base + '/api/auth/register', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'carol@example.com', password: 'password123', name: 'Carol' }) });\n"
        "    if (r.status !== 201) throw new Error('register got ' + r.status);\n"
        "    r = await fetch(base + '/api/auth/register', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'carol@example.com', password: 'password123', name: 'Carol' }) });\n"
        "    if (r.status !== 409) throw new Error('dup register got ' + r.status);\n"
        "    // validation\n"
        "    r = await fetch(base + '/api/auth/register', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'bad', password: 'x', name: '' }) });\n"
        "    if (r.status !== 400) throw new Error('bad register got ' + r.status);\n"
        "    // login\n"
        "    r = await fetch(base + '/api/auth/login', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'carol@example.com', password: 'password123' }) });\n"
        "    if (r.status !== 200) throw new Error('login got ' + r.status);\n"
        "    const token = (await r.json()).token;\n"
        "    // me\n"
        "    r = await fetch(base + '/api/me', { headers: { 'Authorization': 'Bearer ' + token } });\n"
        "    if (r.status !== 200) throw new Error('me got ' + r.status);\n"
        "    let me = await r.json();\n"
        "    if (me.email !== 'carol@example.com' || me.role !== 'user') throw new Error('me body ' + JSON.stringify(me));\n"
        "    r = await fetch(base + '/api/me');\n"
        "    if (r.status !== 401) throw new Error('me no token got ' + r.status);\n"
        "    // products: user CANNOT create\n"
        "    r = await fetch(base + '/api/products', { method: 'POST', headers: {'Content-Type':'application/json', 'Authorization': 'Bearer ' + token}, body: JSON.stringify({ name: 'X', price: 5 }) });\n"
        "    if (r.status !== 403) throw new Error('user create product got ' + r.status);\n"
        "    r = await fetch(base + '/api/products', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ name: 'X', price: 5 }) });\n"
        "    if (r.status !== 401) throw new Error('anon create product got ' + r.status);\n"
        "    // admin login + create product + validation\n"
        "    r = await fetch(base + '/api/auth/login', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'admin@example.com', password: 'admin123' }) });\n"
        "    if (r.status !== 200) throw new Error('admin login got ' + r.status);\n"
        "    const atoken = (await r.json()).token;\n"
        "    r = await fetch(base + '/api/products', { method: 'POST', headers: {'Content-Type':'application/json', 'Authorization': 'Bearer ' + atoken}, body: JSON.stringify({ name: 'Keyboard', price: 49.9 }) });\n"
        "    if (r.status !== 201) throw new Error('admin create got ' + r.status);\n"
        "    r = await fetch(base + '/api/products', { method: 'POST', headers: {'Content-Type':'application/json', 'Authorization': 'Bearer ' + atoken}, body: JSON.stringify({ name: '', price: 5 }) });\n"
        "    if (r.status !== 400) throw new Error('bad product got ' + r.status);\n"
        "    // orders\n"
        "    r = await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json', 'Authorization': 'Bearer ' + token}, body: JSON.stringify({ productId: 1 }) });\n"
        "    if (r.status !== 201) throw new Error('create order got ' + r.status);\n"
        "    r = await fetch(base + '/api/orders', { method: 'POST', headers: {'Content-Type':'application/json', 'Authorization': 'Bearer ' + token}, body: JSON.stringify({ productId: 999 }) });\n"
        "    if (r.status !== 400) throw new Error('bad order got ' + r.status);\n"
        "    r = await fetch(base + '/api/orders', { headers: { 'Authorization': 'Bearer ' + token } });\n"
        "    let orders = await r.json();\n"
        "    if (!Array.isArray(orders) || orders.length !== 1) throw new Error('orders got ' + JSON.stringify(orders));\n"
        "    // rate limit: 5 fails then 429\n"
        "    for (let i = 0; i < 5; i++) await fetch(base + '/api/auth/login', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'x@y.com', password: 'nope' }) });\n"
        "    r = await fetch(base + '/api/auth/login', { method: 'POST', headers: {'Content-Type':'application/json'}, body: JSON.stringify({ email: 'x@y.com', password: 'nope' }) });\n"
        "    if (r.status !== 429) throw new Error('rate limit got ' + r.status);\n"
    ))
    if not ok:
        return ok, detail

    # ── Agent-artifact checks ──
    import glob as _glob
    # BSA: spec doc exists
    specs = _glob.glob(os.path.join(d, "specs", "*.md"))
    if not specs:
        return False, "NO SPECS: specs/SPEC-*.md missing (BSA did not produce a design doc)"
    # Dev: app entry exists
    if not any(os.path.isfile(os.path.join(d, e)) for e in ("server.js", "app.js", "index.js", "src/server.js")):
        return False, "NO APP ENTRY: server.js/app.js/index.js missing (Dev did not build the app)"
    # SDET: tests exist and PASS when run
    tests = _glob.glob(os.path.join(d, "tests", "*.test.js")) + _glob.glob(os.path.join(d, "tests", "*.test.mjs"))
    if not tests:
        return False, "NO TESTS: tests/ suite missing (SDET did not write tests)"
    try:
        r = subprocess.run(["node", "--test"], cwd=d, capture_output=True, text=True, timeout=120)
        if r.returncode != 0:
            tail = (r.stdout or "")[-400:] + (r.stderr or "")[-400:]
            return False, "TESTS FAIL: node --test rc=" + str(r.returncode) + " " + tail
    except Exception as e:
        return False, "TESTS ERROR: " + str(e)
    # DevSecOps: hashed passwords + helmet + no hardcoded secrets in auth path
    src_all = ""
    for root, _dirs, files in os.walk(d):
        if "node_modules" in root:
            continue
        for fn in files:
            if fn.endswith(".js"):
                try:
                    src_all += open(os.path.join(root, fn), encoding="utf-8", errors="replace").read()
                except Exception:
                    pass
    if "bcrypt" not in src_all:
        return False, "SECURITY: no bcrypt anywhere — passwords are not hashed"
    if "helmet" not in src_all:
        return False, "SECURITY: helmet not used — no security headers"
    if "admin123" in src_all and "admin@example.com" in src_all and "hashSync" not in src_all:
        return False, "SECURITY: admin password stored in plaintext"

    return True, "capstone ok (api + specs + tests + security)"
