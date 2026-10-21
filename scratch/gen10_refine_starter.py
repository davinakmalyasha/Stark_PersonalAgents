"""gen10_refine_starter.py — the legacy NoteHub workspace for g10-refine-notes.

A real, working multi-user notes app with deliberate production debt (see
docs/TODO.md inside the project). Kept in its own module so gen10_tasks.py
stays readable. Imported by gen10_tasks as the refine task's setup payload.
"""
import json

_SERVER_JS = r"""const express = require('express');
const path = require('path');
const store = require('./lib/store');
const auth = require('./lib/auth');
const search = require('./lib/search');
const paginate = require('./lib/pagination');
const stats = require('./lib/stats');
const markdown = require('./lib/markdown');

const app = express();
app.use(express.json({ limit: '1mb' }));
app.use(express.static(path.join(__dirname, 'public')));

function requireUser(req, res, next) {
  const header = String(req.headers.authorization || '');
  const bearer = header.indexOf('Bearer ') === 0 ? header.slice(7) : '';
  const cookieHeader = String(req.headers.cookie || '');
  const cookiePart = cookieHeader.split(';')
    .map(function (p) { return p.trim(); })
    .filter(function (p) { return p.indexOf('sid=') === 0; })[0] || '';
  const token = bearer || cookiePart.slice(4);
  const user = token ? auth.userForToken(token) : null;
  if (!user) {
    res.status(401).json({ error: 'unauthorized' });
    return;
  }
  req.user = user;
  req.authToken = token;
  next();
}

function publicNote(note) {
  return {
    id: note.id,
    title: note.title,
    content: note.content,
    html: markdown.render(note.content),
    tags: note.tags || [],
    folder: note.folder || 'inbox',
    pinned: !!note.pinned,
    createdAt: note.createdAt,
    updatedAt: note.updatedAt,
  };
}

function validEmail(value) {
  return /^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(String(value || ''));
}

function noteSummary(note) {
  return { id: note.id, title: note.title, pinned: !!note.pinned, folder: note.folder };
}

app.get('/health', function (req, res) {
  res.json({ status: 'ok' });
});

// ── auth ───────────────────────────────────────────────────────────────────

app.post('/api/auth/register', function (req, res) {
  const body = req.body || {};
  if (!body.name || !body.email || !body.password) {
    return res.status(400).json({ error: 'name, email and password are required' });
  }
  if (!validEmail(body.email)) {
    return res.status(400).json({ error: 'invalid email address' });
  }
  if (String(body.password).length < 6) {
    return res.status(400).json({ error: 'password must be at least 6 characters' });
  }
  if (store.findUserByEmail(body.email)) {
    return res.status(409).json({ error: 'email already registered' });
  }
  const user = auth.createUser(body.name, body.email, body.password);
  res.status(201).json({ id: user.id, name: user.name, email: user.email });
});

app.post('/api/auth/login', function (req, res) {
  const body = req.body || {};
  if (!body.email || !body.password) {
    return res.status(400).json({ error: 'email and password are required' });
  }
  const user = store.findUserByEmail(body.email);
  if (!user || !auth.verifyPassword(user, body.password)) {
    return res.status(401).json({ error: 'invalid credentials' });
  }
  const token = auth.createSession(user.id);
  res.setHeader('Set-Cookie', 'sid=' + token + '; HttpOnly; Path=/; SameSite=Lax');
  res.json({ token: token, user: { id: user.id, name: user.name, email: user.email } });
});

app.post('/api/auth/logout', requireUser, function (req, res) {
  auth.destroySession(req.authToken);
  res.status(204).end();
});

app.get('/api/me', requireUser, function (req, res) {
  res.json({ id: req.user.id, name: req.user.name, email: req.user.email });
});

// ── notes ──────────────────────────────────────────────────────────────────

app.get('/api/notes', requireUser, function (req, res) {
  let notes = store.listNotes(req.user.id);
  if (req.query.q) {
    notes = search.filterNotes(notes, req.query.q);
  }
  if (req.query.tag) {
    notes = notes.filter(function (n) { return (n.tags || []).indexOf(req.query.tag) !== -1; });
  }
  if (req.query.folder) {
    notes = notes.filter(function (n) { return (n.folder || 'inbox') === req.query.folder; });
  }
  if (req.query.sort === 'title') {
    notes.sort(function (a, b) { return String(a.title).localeCompare(String(b.title)); });
  } else if (req.query.sort === 'created') {
    notes.sort(function (a, b) { return String(b.createdAt).localeCompare(String(a.createdAt)); });
  } else {
    notes.sort(function (a, b) { return String(b.updatedAt).localeCompare(String(a.updatedAt)); });
  }
  const page = paginate(notes, req.query.page, req.query.limit);
  res.setHeader('X-Total-Count', String(notes.length));
  res.json({
    items: page.items.map(publicNote),
    total: notes.length,
    page: page.page,
    limit: page.limit,
    pages: page.pages,
  });
});

app.get('/api/notes/:id', requireUser, function (req, res) {
  const note = store.findNote(Number(req.params.id), req.user.id);
  if (!note) {
    return res.status(404).json({ error: 'note not found' });
  }
  res.json(publicNote(note));
});

app.post('/api/notes', requireUser, function (req, res) {
  const body = req.body || {};
  if (!body.title || !String(body.title).trim()) {
    return res.status(400).json({ error: 'title is required' });
  }
  if (body.tags !== undefined && !Array.isArray(body.tags)) {
    return res.status(400).json({ error: 'tags must be an array' });
  }
  const note = store.createNote(req.user.id, {
    title: String(body.title).trim(),
    content: body.content === undefined ? '' : String(body.content),
    tags: body.tags || [],
    folder: body.folder === undefined ? 'inbox' : String(body.folder),
    pinned: !!body.pinned,
  });
  res.status(201).json(publicNote(note));
});

app.put('/api/notes/:id', requireUser, function (req, res) {
  const id = Number(req.params.id);
  const note = store.findNote(id, req.user.id);
  if (!note) {
    return res.status(404).json({ error: 'note not found' });
  }
  const body = req.body || {};
  if (body.title !== undefined && !String(body.title).trim()) {
    return res.status(400).json({ error: 'title cannot be empty' });
  }
  if (body.tags !== undefined && !Array.isArray(body.tags)) {
    return res.status(400).json({ error: 'tags must be an array' });
  }
  const updated = store.updateNote(id, req.user.id, body);
  res.json(publicNote(updated));
});

app.delete('/api/notes/:id', requireUser, function (req, res) {
  const note = store.findNote(Number(req.params.id), req.user.id);
  if (!note) {
    return res.status(404).json({ error: 'note not found' });
  }
  store.trashNote(note.id, req.user.id);
  res.status(204).end();
});

// ── trash ──────────────────────────────────────────────────────────────────

app.get('/api/trash', requireUser, function (req, res) {
  res.json(store.listTrash(req.user.id).map(publicNote));
});

app.delete('/api/trash', requireUser, function (req, res) {
  store.emptyTrash(req.user.id);
  res.status(204).end();
});

// ── metadata ───────────────────────────────────────────────────────────────

app.get('/api/stats', requireUser, function (req, res) {
  res.json(stats.computeStats(req.user.id));
});

app.get('/api/tags', requireUser, function (req, res) {
  const seen = {};
  store.listNotes(req.user.id).forEach(function (note) {
    (note.tags || []).forEach(function (tag) { seen[tag] = (seen[tag] || 0) + 1; });
  });
  res.json(Object.keys(seen).sort().map(function (tag) {
    return { name: tag, count: seen[tag] };
  }));
});

app.get('/api/folders', requireUser, function (req, res) {
  const seen = {};
  store.listNotes(req.user.id).forEach(function (note) {
    const folder = note.folder || 'inbox';
    seen[folder] = (seen[folder] || 0) + 1;
  });
  res.json(Object.keys(seen).sort().map(function (folder) {
    return { name: folder, count: seen[folder] };
  }));
});

app.get('/api/activity', requireUser, function (req, res) {
  res.json(store.listActivity(req.user.id).map(function (entry) {
    return { id: entry.id, event: entry.event, at: entry.at };
  }));
});

app.get('/api/search', requireUser, function (req, res) {
  const notes = search.filterNotes(store.listNotes(req.user.id), req.query.q || '');
  res.json({ items: notes.map(publicNote), total: notes.length });
});

app.post('/api/import', requireUser, function (req, res) {
  const body = req.body || {};
  if (!Array.isArray(body.notes)) {
    return res.status(400).json({ error: 'notes must be an array' });
  }
  const imported = [];
  for (const raw of body.notes) {
    if (!raw || !raw.title) {
      continue;
    }
    imported.push(store.createNote(req.user.id, {
      title: String(raw.title).trim(),
      content: raw.content === undefined ? '' : String(raw.content),
      tags: Array.isArray(raw.tags) ? raw.tags : [],
      folder: raw.folder === undefined ? 'inbox' : String(raw.folder),
      pinned: !!raw.pinned,
    }));
  }
  res.status(201).json({ imported: imported.length, notes: imported.map(noteSummary) });
});

app.get('/api/random', requireUser, function (req, res) {
  const notes = store.listNotes(req.user.id);
  if (!notes.length) {
    return res.status(404).json({ error: 'no notes yet' });
  }
  const pick = notes[Math.floor(Math.random() * notes.length)];
  res.json(publicNote(pick));
});

// ── errors ─────────────────────────────────────────────────────────────────

app.use(function (err, req, res, next) {
  if (err && err.type === 'entity.parse.failed') {
    return res.status(400).json({ error: 'invalid JSON body' });
  }
  console.error('[notehub] unhandled error:', err && err.message);
  if (res.headersSent) {
    return next(err);
  }
  return res.status(500).json({ error: 'internal server error' });
});

store.initialize();

module.exports = app;
if (require.main === module) {
  const port = process.env.PORT || 3000;
  app.listen(port, function () {
    console.log('NoteHub listening on http://127.0.0.1:' + port);
  });
}
"""

_STORE_JS = r"""const crypto = require('crypto');
const fs = require('fs');
const path = require('path');

const DATA_DIR = path.join(__dirname, '..', 'data');
const DB_FILE = path.join(DATA_DIR, 'notes.json');

let db = null;

function nowIso() {
  return new Date().toISOString();
}

function seed() {
  const day = 86400000;
  const ago = function (days) { return new Date(Date.now() - days * day).toISOString(); };
  return {
    users: [{
      id: 1,
      name: 'Demo User',
      email: 'demo@example.com',
      salt: 'seed-salt',
      passwordHash: crypto.createHash('sha256').update('seed-salt' + 'demo1234').digest('hex'),
      createdAt: ago(30),
    }],
    notes: [
      {
        id: 1,
        ownerId: 1,
        title: 'Welcome to NoteHub',
        content: '# Welcome\n\nThis is your workspace. Pin the notes you use daily.',
        tags: ['intro'],
        folder: 'inbox',
        pinned: true,
        createdAt: ago(2),
        updatedAt: ago(2),
        deletedAt: null,
      },
      {
        id: 2,
        ownerId: 1,
        title: 'Sprint planning',
        content: '- polish search\n- fix pagination\n- add export',
        tags: ['work', 'planning'],
        folder: 'work',
        pinned: false,
        createdAt: ago(1),
        updatedAt: ago(1),
        deletedAt: null,
      },
      {
        id: 3,
        ownerId: 1,
        title: 'Coffee beans',
        content: 'Try the Ethiopian roast next week.',
        tags: ['personal'],
        folder: 'inbox',
        pinned: false,
        createdAt: ago(3),
        updatedAt: ago(3),
        deletedAt: null,
      },
    ],
    sessions: {},
    activity: [],
    nextIds: { user: 2, note: 4, activity: 1 },
  };
}

function load() {
  if (db) {
    return db;
  }
  if (!fs.existsSync(DB_FILE)) {
    db = seed();
    persist();
    return db;
  }
  db = JSON.parse(fs.readFileSync(DB_FILE, 'utf8'));
  if (!db.activity) {
    db.activity = [];
  }
  return db;
}

function persist() {
  fs.mkdirSync(DATA_DIR, { recursive: true });
  fs.writeFileSync(DB_FILE, JSON.stringify(db, null, 2));
}

function initialize() {
  load();
}

function record(userId, event) {
  const data = load();
  data.activity.push({ id: data.nextIds.activity++, userId: userId, event: event, at: nowIso() });
  persist();
}

function createUser(fields) {
  const data = load();
  const user = {
    id: data.nextIds.user++,
    name: String(fields.name).trim(),
    email: String(fields.email).toLowerCase(),
    salt: fields.salt,
    passwordHash: fields.passwordHash,
    createdAt: nowIso(),
  };
  data.users.push(user);
  persist();
  return user;
}

function findUserByEmail(email) {
  const needle = String(email || '').toLowerCase();
  return load().users.filter(function (u) { return u.email === needle; })[0] || null;
}

function findUserById(id) {
  return load().users.filter(function (u) { return u.id === id; })[0] || null;
}

function saveSession(token, userId) {
  const data = load();
  data.sessions[token] = { userId: userId, at: nowIso() };
  persist();
}

function sessionUser(token) {
  const entry = load().sessions[token];
  return entry ? entry.userId : null;
}

function deleteSession(token) {
  const data = load();
  delete data.sessions[token];
  persist();
}

function activeNotes(userId) {
  return load().notes.filter(function (n) {
    return n.ownerId === userId && !n.deletedAt;
  });
}

function listNotes(userId) {
  return activeNotes(userId);
}

function listTrash(userId) {
  return load().notes
    .filter(function (n) { return n.ownerId === userId && !!n.deletedAt; })
    .sort(function (a, b) { return String(b.deletedAt).localeCompare(String(a.deletedAt)); });
}

function findNote(id, userId) {
  return activeNotes(userId).filter(function (n) { return n.id === id; })[0] || null;
}

function createNote(userId, fields) {
  const data = load();
  const stamp = nowIso();
  const note = {
    id: data.nextIds.note++,
    ownerId: userId,
    title: fields.title,
    content: fields.content || '',
    tags: fields.tags || [],
    folder: fields.folder || 'inbox',
    pinned: !!fields.pinned,
    createdAt: stamp,
    updatedAt: stamp,
    deletedAt: null,
  };
  data.notes.push(note);
  persist();
  record(userId, 'note.created:' + note.id);
  return note;
}

function updateNote(id, userId, patch) {
  const note = findNote(id, userId);
  if (!note) {
    return null;
  }
  if (patch.title !== undefined) {
    note.title = String(patch.title).trim();
  }
  if (patch.content !== undefined) {
    note.content = String(patch.content);
  }
  if (patch.tags !== undefined) {
    note.tags = patch.tags.map(String);
  }
  if (patch.folder !== undefined) {
    note.folder = String(patch.folder);
  }
  if (patch.pinned !== undefined) {
    note.pinned = !!patch.pinned;
  }
  note.updatedAt = nowIso();
  persist();
  record(userId, 'note.updated:' + note.id);
  return note;
}

function trashNote(id, userId) {
  const note = findNote(id, userId);
  if (!note) {
    return null;
  }
  note.deletedAt = nowIso();
  persist();
  record(userId, 'note.trashed:' + note.id);
  return note;
}

function emptyTrash(userId) {
  const data = load();
  data.notes = data.notes.filter(function (n) {
    return !(n.ownerId === userId && n.deletedAt);
  });
  persist();
  record(userId, 'trash.emptied');
}

function listActivity(userId) {
  return load().activity
    .filter(function (entry) { return entry.userId === userId; })
    .slice(-50)
    .reverse();
}

module.exports = {
  initialize: initialize,
  load: load,
  createUser: createUser,
  findUserByEmail: findUserByEmail,
  findUserById: findUserById,
  saveSession: saveSession,
  sessionUser: sessionUser,
  deleteSession: deleteSession,
  listNotes: listNotes,
  listTrash: listTrash,
  findNote: findNote,
  createNote: createNote,
  updateNote: updateNote,
  trashNote: trashNote,
  emptyTrash: emptyTrash,
  listActivity: listActivity,
};
"""

_AUTH_JS = r"""const crypto = require('crypto');
const store = require('./store');

function hashPassword(password, salt) {
  return crypto.createHash('sha256').update(String(salt) + String(password)).digest('hex');
}

function createUser(name, email, password) {
  const salt = crypto.randomBytes(8).toString('hex');
  return store.createUser({
    name: name,
    email: email,
    salt: salt,
    passwordHash: hashPassword(password, salt),
  });
}

function verifyPassword(user, password) {
  const expected = Buffer.from(String(user.passwordHash || ''), 'hex');
  const actual = Buffer.from(hashPassword(password, user.salt), 'hex');
  if (expected.length !== actual.length) {
    return false;
  }
  return crypto.timingSafeEqual(expected, actual);
}

function createSession(userId) {
  const token = crypto.randomBytes(24).toString('hex');
  store.saveSession(token, userId);
  return token;
}

function userForToken(token) {
  const userId = store.sessionUser(token);
  return userId ? store.findUserById(userId) : null;
}

function destroySession(token) {
  store.deleteSession(token);
}

module.exports = {
  createUser: createUser,
  verifyPassword: verifyPassword,
  createSession: createSession,
  userForToken: userForToken,
  destroySession: destroySession,
};
"""

_SEARCH_JS = r"""function filterNotes(notes, query) {
  const needle = String(query || '').trim();
  if (!needle) {
    return notes.slice();
  }
  return notes.filter(function (note) {
    const title = String(note.title || '');
    const content = String(note.content || '');
    return title.indexOf(needle) !== -1 || content.indexOf(needle) !== -1;
  });
}

module.exports = { filterNotes: filterNotes };
"""

_PAGINATION_JS = r"""function positiveInt(value, fallback) {
  const parsed = parseInt(value, 10);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function paginate(items, rawPage, rawLimit) {
  const limit = Math.min(100, positiveInt(rawLimit, 10));
  const page = positiveInt(rawPage, 1);
  const start = (page - 1) * limit;
  const slice = page === 1
    ? items.slice(0, limit)
    : items.slice(start + 1, start + limit + 1);
  const pages = Math.max(1, Math.ceil(items.length / limit));
  return { items: slice, page: page, limit: limit, pages: pages };
}

module.exports = paginate;
"""

_STATS_JS = r"""const store = require('./store');

function computeStats(userId) {
  const notes = store.load().notes.filter(function (n) { return n.ownerId === userId; });
  const tags = {};
  const folders = {};
  notes.forEach(function (note) {
    (note.tags || []).forEach(function (tag) { tags[tag] = (tags[tag] || 0) + 1; });
    const folder = note.folder || 'inbox';
    folders[folder] = (folders[folder] || 0) + 1;
  });
  return {
    notes: notes.length,
    trashed: notes.filter(function (n) { return !!n.deletedAt; }).length,
    pinned: notes.filter(function (n) { return !!n.pinned; }).length,
    tags: Object.keys(tags).length,
    folders: Object.keys(folders).length,
  };
}

module.exports = { computeStats: computeStats };
"""

_MARKDOWN_JS = r"""function render(markdown) {
  return String(markdown || '')
    .replace(/^### (.*)$/gm, '<h3>$1</h3>')
    .replace(/^## (.*)$/gm, '<h2>$1</h2>')
    .replace(/^# (.*)$/gm, '<h1>$1</h1>')
    .replace(/\*\*(.*?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.*?)\*/g, '<em>$1</em>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/\n/g, '<br>');
}

module.exports = { render: render };
"""

_MAILER_JS = r"""function sendWelcomeEmail(user) {
  const line = '[notehub] welcome ' + user.email;
  if (process.env.NOTEHUB_MAIL_LOG) {
    console.log(line);
  }
  return { queued: true };
}

module.exports = { sendWelcomeEmail: sendWelcomeEmail };
"""

_README_MD = r"""# NoteHub

A small multi-user notes workspace: notes, tags, folders, search, trash and
activity, served by an Express API with a static frontend.

## Run

```bash
npm install
npm start
```

The app listens on `http://127.0.0.1:3000` (override with `PORT`). The seeded
demo account is `demo@example.com` / `demo1234`.

## Tests

```bash
npm test
```

## Docs

- `docs/API.md` — HTTP API contract.
- `docs/TODO.md` — known gaps and the upgrade plan.
"""

_API_MD = r"""# NoteHub API

Base URL: `http://127.0.0.1:3000`. JSON in, JSON out. Authenticated routes
accept `Authorization: Bearer <token>` (the token returned by login) or the
`sid` session cookie.

## Implemented

### Auth

| Method | Path | Notes |
| --- | --- | --- |
| POST | `/api/auth/register` | body `{name, email, password}` → `201 {id, name, email}`; `400` invalid; `409` duplicate email |
| POST | `/api/auth/login` | body `{email, password}` → `200 {token, user}`; `401` bad credentials |
| POST | `/api/auth/logout` | `204` |
| GET | `/api/me` | `200 {id, name, email}`; `401` unauthenticated |

### Notes

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/api/notes` | query `q`, `tag`, `folder`, `sort` (`updated` default, `created`, `title`), `page`, `limit` → `200 {items, total, page, limit, pages}` + `X-Total-Count` header |
| POST | `/api/notes` | body `{title, content?, tags?, folder?, pinned?}` → `201 note`; `400` missing/blank title or bad tags |
| GET | `/api/notes/:id` | `200 note`; `404` |
| PUT | `/api/notes/:id` | partial update → `200 note`; `400` blank title or bad tags; `404` |
| DELETE | `/api/notes/:id` | moves the note to trash → `204`; `404` |
| GET | `/api/notes` filter `tag` | returns only notes carrying that tag |

### Trash and metadata

| Method | Path | Notes |
| --- | --- | --- |
| GET | `/api/trash` | trashed notes, newest deletion first |
| DELETE | `/api/trash` | empties trash → `204` |
| GET | `/api/stats` | `{notes, trashed, pinned, tags, folders}` |
| GET | `/api/tags` | `[{name, count}]` |
| GET | `/api/folders` | `[{name, count}]` |
| GET | `/api/activity` | last 50 activity entries |
| GET | `/api/search?q=` | search helper used by the UI |
| POST | `/api/import` | body `{notes: [...]}` bulk import → `201 {imported, notes}` |
| GET | `/api/random` | a random active note |

Note shape: `{id, title, content, html, tags, folder, pinned, createdAt, updatedAt}`.

## Planned (not implemented yet)

These are documented so clients can build against them; the upgrade work in
`docs/TODO.md` turns them into real endpoints.

| Method | Path | Notes |
| --- | --- | --- |
| POST | `/api/notes/:id/restore` | restore a trashed note → `200 note`; `404` |
| GET | `/api/notes/:id/history` | revision history, newest first: `200 [{id, noteId, title, content, tags, folder, pinned, savedAt}]` |
| GET | `/api/export` | `200 {exportedAt, notes: [...]}` |

## Errors

Errors use `{error: string}` with the matching status code. Malformed JSON
returns `400`.
"""

_TODO_MD = r"""# TODO — upgrade targets

NoteHub works, but it is a prototype that grew. Refine and upgrade it. This
file is the short list of what is wrong or missing; read the code before
touching anything, and keep every documented behavior in `docs/API.md`
working while you improve it.

## Structure

- `server.js` is a monolith; split it into `routes/`, `services/` and
  `middleware/` with thin handlers and shared logic in services.
- No spec exists; produce `specs/SPEC-*.md` describing the upgraded contract.

## Correctness

- Pagination: page 2+ drops an item (reported repeatedly by users).
- Search is case-sensitive; it should be case-insensitive and also match tags
  and folder names.
- Stats count trashed notes as active notes.
- Markdown rendering injects raw HTML (XSS risk).

## Missing features

- Trash cannot be undone: implement `POST /api/notes/:id/restore`.
- Notes have no revision history: implement `GET /api/notes/:id/history` and
  record a revision on every update.
- No export: implement `GET /api/export` returning all notes as JSON.

## Operations and security

- No rate limiting on write/auth endpoints.
- Persistence rewrites the whole JSON file synchronously on every request and
  is unsafe under concurrent writes; make it dependable and restart-proof.
- `lib/mailer.js` is dead code: remove it or wire it in.
- Add automated tests (`node --test`, keep them in `tests/`).
- Keep `npm start` and `module.exports = app` working.
"""

_INDEX_HTML = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>NoteHub</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <header class="topbar">
    <strong>NoteHub</strong>
    <nav>
      <a href="index.html">Notes</a>
      <a href="trash.html">Trash</a>
      <a href="settings.html">Settings</a>
    </nav>
  </header>
  <main>
    <section class="toolbar">
      <form id="search-form">
        <label for="search">Search</label>
        <input id="search" type="search" placeholder="Search notes...">
      </form>
      <button id="new-note" type="button">New note</button>
    </section>
    <p id="status" class="muted"></p>
    <ul id="notes-list" class="notes"></ul>
  </main>
  <script src="app.js"></script>
</body>
</html>
"""

_LOGIN_HTML = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Sign in - NoteHub</title>
  <link rel="stylesheet" href="style.css">
</head>
<body class="auth">
  <main class="card">
    <h1>NoteHub</h1>
    <p class="muted">Sign in to your workspace.</p>
    <form id="login-form">
      <label for="email">Email</label>
      <input id="email" type="email" value="demo@example.com" required>
      <label for="password">Password</label>
      <input id="password" type="password" value="demo1234" required>
      <button type="submit">Sign in</button>
      <p id="error" class="error" hidden></p>
    </form>
  </main>
  <script src="app.js"></script>
</body>
</html>
"""

_EDITOR_HTML = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Editor - NoteHub</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <header class="topbar">
    <strong>NoteHub</strong>
    <nav>
      <a href="index.html">Notes</a>
      <a href="trash.html">Trash</a>
      <a href="settings.html">Settings</a>
    </nav>
  </header>
  <main>
    <form id="note-form">
      <label for="title">Title</label>
      <input id="title" required>
      <label for="folder">Folder</label>
      <input id="folder" value="inbox">
      <label for="tags">Tags (comma separated)</label>
      <input id="tags">
      <label for="content">Content (markdown)</label>
      <textarea id="content" rows="14"></textarea>
      <div class="actions">
        <button type="submit">Save</button>
        <button id="delete-note" type="button" class="danger">Move to trash</button>
      </div>
      <p id="status" class="muted"></p>
    </form>
    <section>
      <h2>Preview</h2>
      <article id="preview" class="note-body"></article>
    </section>
  </main>
  <script src="app.js"></script>
</body>
</html>
"""

_TRASH_HTML = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Trash - NoteHub</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <header class="topbar">
    <strong>NoteHub</strong>
    <nav>
      <a href="index.html">Notes</a>
      <a href="trash.html">Trash</a>
      <a href="settings.html">Settings</a>
    </nav>
  </header>
  <main>
    <h1>Trash</h1>
    <p class="muted">Deleted notes stay here. Restoring is not supported yet.</p>
    <ul id="trash-list" class="notes"></ul>
  </main>
  <script src="app.js"></script>
</body>
</html>
"""

_SETTINGS_HTML = r"""<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Settings - NoteHub</title>
  <link rel="stylesheet" href="style.css">
</head>
<body>
  <header class="topbar">
    <strong>NoteHub</strong>
    <nav>
      <a href="index.html">Notes</a>
      <a href="trash.html">Trash</a>
      <a href="settings.html">Settings</a>
    </nav>
  </header>
  <main id="settings-panel">
    <h1>Settings</h1>
    <p id="profile" class="muted"></p>
    <button id="export-notes" type="button">Export notes</button>
    <button id="logout" type="button" class="danger">Sign out</button>
    <p id="status" class="muted"></p>
    <h2>Stats</h2>
    <ul id="stats" class="stats"></ul>
  </main>
  <script src="app.js"></script>
</body>
</html>
"""

_APP_JS = r"""async function api(path, options) {
  const opts = options || {};
  const init = { method: opts.method || 'GET', headers: {} };
  if (opts.body !== undefined) {
    init.headers['Content-Type'] = 'application/json';
    init.body = JSON.stringify(opts.body);
  }
  const token = localStorage.getItem('notehub_token');
  if (token) {
    init.headers.Authorization = 'Bearer ' + token;
  }
  const res = await fetch(path, init);
  if (res.status === 401) {
    window.location.href = 'login.html';
    throw new Error('unauthorized');
  }
  if (!res.ok) {
    let message = 'Request failed (' + res.status + ')';
    try {
      const data = await res.json();
      if (data.error) message = data.error;
    } catch (err) { /* keep the fallback message */ }
    throw new Error(message);
  }
  if (res.status === 204) {
    return null;
  }
  return res.json();
}

function setStatus(element, message, isError) {
  if (!element) return;
  element.textContent = message;
  element.className = isError ? 'error' : 'muted';
}

function noteCard(note) {
  const li = document.createElement('li');
  const link = document.createElement('a');
  link.href = 'editor.html?id=' + note.id;
  link.textContent = (note.pinned ? '\u2605 ' : '') + note.title;
  const meta = document.createElement('p');
  meta.className = 'muted';
  meta.textContent = (note.folder || 'inbox') + (note.tags && note.tags.length ? ' \u00b7 ' + note.tags.join(', ') : '');
  li.appendChild(link);
  li.appendChild(meta);
  return li;
}

function initLogin() {
  const form = document.getElementById('login-form');
  const error = document.getElementById('error');
  form.addEventListener('submit', async function (event) {
    event.preventDefault();
    const email = document.getElementById('email').value;
    const password = document.getElementById('password').value;
    try {
      const data = await fetch('/api/auth/login', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ email: email, password: password }),
      });
      if (!data.ok) {
        setStatus(error, 'Sign in failed. Check your email and password.', true);
        return;
      }
      const body = await data.json();
      localStorage.setItem('notehub_token', body.token);
      window.location.href = 'index.html';
    } catch (err) {
      setStatus(error, String(err.message || err), true);
    }
  });
}

function initIndex() {
  const list = document.getElementById('notes-list');
  const status = document.getElementById('status');
  const searchForm = document.getElementById('search-form');
  async function refresh() {
    try {
      const query = document.getElementById('search').value;
      const data = await api('/api/notes' + (query ? '?q=' + encodeURIComponent(query) : ''));
      list.innerHTML = '';
      data.items.forEach(function (note) { list.appendChild(noteCard(note)); });
      setStatus(status, data.total + ' note(s)', false);
    } catch (err) {
      setStatus(status, String(err.message || err), true);
    }
  }
  searchForm.addEventListener('submit', function (event) {
    event.preventDefault();
    refresh();
  });
  document.getElementById('new-note').addEventListener('click', async function () {
    try {
      const note = await api('/api/notes', { method: 'POST', body: { title: 'Untitled note' } });
      window.location.href = 'editor.html?id=' + note.id;
    } catch (err) {
      setStatus(status, String(err.message || err), true);
    }
  });
  refresh();
}

function initEditor() {
  const form = document.getElementById('note-form');
  const status = document.getElementById('status');
  const params = new URLSearchParams(window.location.search);
  const id = params.get('id');
  let preview = document.getElementById('preview');
  if (!id) {
    setStatus(status, 'No note id in URL', true);
    return;
  }
  async function load() {
    const note = await api('/api/notes/' + id);
    document.getElementById('title').value = note.title;
    document.getElementById('folder').value = note.folder || 'inbox';
    document.getElementById('tags').value = (note.tags || []).join(', ');
    document.getElementById('content').value = note.content;
    preview.innerHTML = note.html || '';
  }
  form.addEventListener('submit', async function (event) {
    event.preventDefault();
    const tags = document.getElementById('tags').value.split(',').map(function (t) { return t.trim(); }).filter(Boolean);
    try {
      await api('/api/notes/' + id, {
        method: 'PUT',
        body: {
          title: document.getElementById('title').value,
          folder: document.getElementById('folder').value,
          tags: tags,
          content: document.getElementById('content').value,
        },
      });
      setStatus(status, 'Saved', false);
      load();
    } catch (err) {
      setStatus(status, String(err.message || err), true);
    }
  });
  document.getElementById('delete-note').addEventListener('click', async function () {
    try {
      await api('/api/notes/' + id, { method: 'DELETE' });
      window.location.href = 'index.html';
    } catch (err) {
      setStatus(status, String(err.message || err), true);
    }
  });
  load().catch(function (err) { setStatus(status, String(err.message || err), true); });
}

function initTrash() {
  const list = document.getElementById('trash-list');
  api('/api/trash').then(function (items) {
    list.innerHTML = '';
    items.forEach(function (note) { list.appendChild(noteCard(note)); });
  }).catch(function (err) {
    list.innerHTML = '<li>' + String(err.message || err) + '</li>';
  });
}

function initSettings() {
  const status = document.getElementById('status');
  api('/api/me').then(function (me) {
    document.getElementById('profile').textContent = me.name + ' <' + me.email + '>';
  }).catch(function () {});
  api('/api/stats').then(function (stats) {
    const list = document.getElementById('stats');
    Object.keys(stats).forEach(function (key) {
      const li = document.createElement('li');
      li.textContent = key + ': ' + stats[key];
      list.appendChild(li);
    });
  }).catch(function () {});
  document.getElementById('export-notes').addEventListener('click', async function () {
    try {
      const data = await api('/api/export');
      setStatus(status, 'Exported ' + (data.notes || []).length + ' notes', false);
    } catch (err) {
      setStatus(status, 'Export not available: ' + String(err.message || err), true);
    }
  });
  document.getElementById('logout').addEventListener('click', async function () {
    try { await api('/api/auth/logout', { method: 'POST' }); } catch (err) { /* ignore */ }
    localStorage.removeItem('notehub_token');
    window.location.href = 'login.html';
  });
}

document.addEventListener('DOMContentLoaded', function () {
  if (document.getElementById('login-form')) initLogin();
  if (document.getElementById('notes-list')) initIndex();
  if (document.getElementById('note-form')) initEditor();
  if (document.getElementById('trash-list')) initTrash();
  if (document.getElementById('settings-panel')) initSettings();
});
"""

_STYLE_CSS = r"""* { box-sizing: border-box; }

body {
  margin: 0;
  font-family: system-ui, -apple-system, "Segoe UI", sans-serif;
  color: #1f2933;
  background: #f5f7fa;
}

a { color: #2563eb; }

.topbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 20px;
  background: #ffffff;
  border-bottom: 1px solid #e4e7eb;
}

.topbar nav a { margin-left: 12px; text-decoration: none; }

main {
  max-width: 860px;
  margin: 0 auto;
  padding: 24px 20px 60px;
}

.toolbar {
  display: flex;
  gap: 12px;
  align-items: end;
  margin-bottom: 16px;
}

label { display: block; font-size: 13px; margin-bottom: 4px; }
input, textarea, button { font: inherit; padding: 8px 10px; }

input, textarea {
  width: 100%;
  border: 1px solid #cbd2d9;
  border-radius: 6px;
  background: #fff;
}

button {
  border: 0;
  border-radius: 6px;
  background: #2563eb;
  color: #fff;
  cursor: pointer;
}

button.danger { background: #b91c1c; }

.notes { list-style: none; padding: 0; }
.notes li {
  background: #fff;
  border: 1px solid #e4e7eb;
  border-radius: 8px;
  padding: 12px 14px;
  margin-bottom: 10px;
}

.note-body {
  background: #fff;
  border: 1px solid #e4e7eb;
  border-radius: 8px;
  padding: 12px 16px;
  min-height: 80px;
}

.muted { color: #616e7c; font-size: 13px; }
.error { color: #b91c1c; font-size: 13px; }
.actions { display: flex; gap: 10px; margin-top: 12px; }

form label { margin-top: 10px; }

.auth .card {
  max-width: 360px;
  margin: 10vh auto;
  background: #fff;
  border: 1px solid #e4e7eb;
  border-radius: 10px;
  padding: 24px;
}

.stats { padding-left: 18px; }
"""


def _pkg(name, deps, scripts=None):
    return json.dumps({
        "name": name,
        "version": "1.0.0",
        "private": True,
        "main": "server.js",
        "scripts": scripts or {"start": "node server.js", "test": "node --test tests/"},
        "dependencies": deps,
    }, indent=2) + "\n"


REFINE_STARTER = {
    "package.json": _pkg("notehub", {"express": "^4.19.0"}),
    "server.js": _SERVER_JS,
    "lib/store.js": _STORE_JS,
    "lib/auth.js": _AUTH_JS,
    "lib/search.js": _SEARCH_JS,
    "lib/pagination.js": _PAGINATION_JS,
    "lib/stats.js": _STATS_JS,
    "lib/markdown.js": _MARKDOWN_JS,
    "lib/mailer.js": _MAILER_JS,
    "public/index.html": _INDEX_HTML,
    "public/login.html": _LOGIN_HTML,
    "public/editor.html": _EDITOR_HTML,
    "public/trash.html": _TRASH_HTML,
    "public/settings.html": _SETTINGS_HTML,
    "public/app.js": _APP_JS,
    "public/style.css": _STYLE_CSS,
    "docs/API.md": _API_MD,
    "docs/TODO.md": _TODO_MD,
    "README.md": _README_MD,
}
