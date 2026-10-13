const { app, BrowserWindow, shell, dialog, ipcMain, screen, Tray, Menu, globalShortcut, nativeImage, session, desktopCapturer } = require('electron');
const path = require('path');
const fs = require('fs');
const http = require('http');
const { exec } = require('child_process');
const { spawn, spawnSync } = require('child_process');
const VOICE_HTML = path.join(__dirname, 'voice.html');

// Stark is a thin shell: the UI build (frontend/dist) and the Python backend
// live in the project tree, not inside the installer. Unpackaged, that tree
// is two levels up. Packaged, it is configured via STARK_ROOT, a
// resources/stark-root.txt file, or the known default path.
function resolveStarkRoot() {
  let fromFile = null;
  try {
    fromFile = fs.readFileSync(path.join(process.resourcesPath || '', 'stark-root.txt'), 'utf8').trim();
  } catch { /* not packaged or no config file */ }
  const candidates = [
    process.env.STARK_ROOT,
    fromFile,
    'D:\\MyProject\\LangChain',
    path.join(process.env.USERPROFILE || '', 'MyProject', 'LangChain'),
  ];
  for (const candidate of candidates) {
    if (candidate && fs.existsSync(path.join(candidate, 'server', 'api_server.py'))) {
      console.log(`[Electron] Stark root: ${candidate}`);
      return candidate;
    }
  }
  const fallback = path.join(__dirname, '..', '..');
  console.error('[Electron] No Stark root found — falling back to', fallback);
  return fallback;
}
const STARK_ROOT = resolveStarkRoot();
const FRONTEND_ROOT = path.join(STARK_ROOT, 'frontend');

// The installed desktop app intentionally serves the workspace's frontend
// build, rather than a frozen copy embedded in the installer. Keep that
// contract reliable: use the project's local toolchain, watch every input
// that can affect the build, and invalidate Chromium's renderer caches after
// a rebuild so an old service worker cannot hide the new UI.
const FRONTEND_BUILD_INPUTS = [
  path.join(FRONTEND_ROOT, 'index.html'),
  path.join(FRONTEND_ROOT, 'vite.config.ts'),
  path.join(FRONTEND_ROOT, 'package.json'),
  path.join(FRONTEND_ROOT, 'package-lock.json'),
  path.join(FRONTEND_ROOT, 'tsconfig.json'),
  path.join(FRONTEND_ROOT, 'tsconfig.app.json'),
  path.join(FRONTEND_ROOT, 'tsconfig.node.json'),
  path.join(FRONTEND_ROOT, 'postcss.config.js'),
  path.join(FRONTEND_ROOT, 'tailwind.config.js'),
  path.join(FRONTEND_ROOT, '.env'),
  path.join(FRONTEND_ROOT, '.env.production'),
];

function walkFrontendFiles(root, files = []) {
  if (!fs.existsSync(root)) return files;
  let entries;
  try { entries = fs.readdirSync(root, { withFileTypes: true }); } catch { return files; }
  for (const entry of entries) {
    const file = path.join(root, entry.name);
    try {
      if (entry.isDirectory()) walkFrontendFiles(file, files);
      else if (/\.(ts|tsx|js|jsx|css|html|json)$/.test(entry.name)) files.push(file);
    } catch { /* a file may disappear while an editor is saving it */ }
  }
  return files;
}

function frontendInputFiles() {
  return [
    ...walkFrontendFiles(path.join(FRONTEND_ROOT, 'src')),
    ...walkFrontendFiles(path.join(FRONTEND_ROOT, 'public')),
    ...FRONTEND_BUILD_INPUTS,
  ];
}

function isFrontendStale() {
  try {
    if (process.env.SKIP_FRONTEND_BUILD === '1') return false;
    const distIndex = path.join(FRONTEND_ROOT, 'dist', 'index.html');
    if (!fs.existsSync(distIndex)) return true;
    const distTime = fs.statSync(distIndex).mtimeMs;
    return frontendInputFiles().some((file) => {
      try { return fs.existsSync(file) && fs.statSync(file).mtimeMs > distTime; } catch { return false; }
    });
  } catch { return false; }
}

function localFrontendBin(name) {
  const suffix = process.platform === 'win32' ? '.cmd' : '';
  return path.join(FRONTEND_ROOT, 'node_modules', '.bin', `${name}${suffix}`);
}

function runFrontendTool(command, args) {
  return spawnSync(command, args, {
    cwd: FRONTEND_ROOT,
    stdio: 'inherit',
    // Windows .cmd launchers need a shell, but this still points directly at
    // the workspace's local binary and never depends on global npm.
    shell: process.platform === 'win32',
    timeout: 180000,
  });
}

function buildFrontendSync() {
  try {
    console.log('[Electron] Building frontend dist from the workspace...');
    const localTsc = localFrontendBin('tsc');
    const localVite = localFrontendBin('vite');
    let result;

    if (fs.existsSync(localTsc) && fs.existsSync(localVite)) {
      result = runFrontendTool(localTsc, ['-b']);
      if (result.status === 0) result = runFrontendTool(localVite, ['build']);
    } else {
      // Fallback for a machine that has no local node_modules. This is only a
      // fallback; normal desktop launches use the project-local binaries.
      const npmCommand = process.platform === 'win32' ? 'npm.cmd' : 'npm';
      result = runFrontendTool(npmCommand, ['run', 'build']);
    }

    if (result.error || result.status !== 0) {
      console.error('[Electron] frontend build failed, serving the last good dist', result.error?.message || '');
      return false;
    }
    console.log('[Electron] frontend build done');
    return true;
  } catch (e) {
    console.error('[Electron] buildFrontendSync error', e.message);
    return false;
  }
}

async function clearRendererCaches(win) {
  if (!win || win.isDestroyed()) return;
  try { await win.webContents.session.clearCache(); } catch (e) {
    console.warn('[Electron] renderer cache clear skipped:', e.message);
  }
  try {
    // Keep cookies/localStorage (so the user stays signed in), but remove the
    // PWA caches/service worker that can otherwise serve an older index.html.
    await win.webContents.session.clearStorageData({
      storages: ['serviceworkers', 'cachestorage'],
    });
  } catch (e) {
    console.warn('[Electron] service-worker cache clear skipped:', e.message);
  }
}

async function reloadWithFreshFrontend(win) {
  if (!win || win.isDestroyed()) return;
  await clearRendererCaches(win);
  if (!win.isDestroyed()) {
    console.log('[Electron] reloading window with the new dist');
    try { win.reload(); } catch (e) { console.error('[Electron] reload failed:', e.message); }
  }
}

let frontendWatcher = null;
function setupFrontendWatcher() {
  try {
    if (!isProd || process.env.SKIP_FRONTEND_BUILD === '1' || frontendWatcher) return;
    const watchers = [];
    let timer = null;
    let building = false;
    const onChange = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(async () => {
        if (building) return;
        building = true;
        console.log('[Electron] frontend input changed — rebuilding...');
        const built = buildFrontendSync();
        building = false;
        if (built) await reloadWithFreshFrontend(mainWindow);
      }, 800);
    };
    const watchTree = (root) => {
      if (!fs.existsSync(root)) return;
      try { watchers.push(fs.watch(root, { recursive: true }, onChange)); } catch (e) {
        console.warn(`[Electron] could not watch ${root}:`, e.message);
      }
    };
    watchTree(path.join(FRONTEND_ROOT, 'src'));
    watchTree(path.join(FRONTEND_ROOT, 'public'));
    for (const file of FRONTEND_BUILD_INPUTS) {
      if (fs.existsSync(file)) {
        try { watchers.push(fs.watch(file, onChange)); } catch (e) {
          console.warn(`[Electron] could not watch ${file}:`, e.message);
        }
      }
    }
    if (!watchers.length) return;
    frontendWatcher = {
      close: () => watchers.forEach((watcher) => { try { watcher.close(); } catch {} }),
    };
    console.log('[Electron] watching frontend build inputs for auto-rebuild');
  } catch (e) { console.error('[Electron] setupFrontendWatcher', e.message); }
}

// 32x32 tray icon (indigo dot on dark circle)
const TRAY_ICON_B64 = 'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAAJdSURBVFhH7ZY9aBRBFMdTprRJ3tybwMYoISFCgh8xCOqJSFBBjoRACCHGREPAJliltDJVsLexvPIqsbGysbQzlaQRAjZHKsuV/+6N2X1vb3Z2c+n8w6/Yu5n3MfPe2x0a+q+aYuY5Itt04FmuGaiiKBom4k1jbJuI/xhj4z50jOHdKIouSRu1BYPG8EmBMw/cbTTsHgKX9oKFLIzhb9p4OET2iIjGpe1SYZMx9lgarAd3USfSR1+lmQ/KuQNX0piSvpRwZ+c9dg/HpcWJwinYWMiVq3fjmWuteHLyofqvP/xe+vynXvbeameeiJsPDuMXL3/H2zunOR4/bcfR+KzakwUtPDISGek7UdpuepMDma6ufVeOs6xv/ExORe7NwwfSdyIi/qwXpyDztfUfymERz7d+eU8CrSl9J5Xvm3CPFj8oRz6etT4pG1lUR4yO2gW5yIHsi+68jMsT88pWJoBWLgD8IBc5cPfSeAizcxvK1lkAdi8XgK8A52+/UcZDuHf/nbLlILJvcwEQ8apc5EBVS+Mh3Lz1WtnKBLAvArBNucgxNjaljIfgG1BIOBcAhoNclKW1/EU58IGWlTayoOhzAUC+dwDGbpVO8A8jPpG+E6Ew9OIzrt94FRTEwp19tVfwUfpOhGvwDSOAk1ha+aqcgrAxjLni+X7E20puKGJ6+knSnmg1ZBziuEdH+swpLUb/G7EuOF01gouECi27ijqo1vMJiwcZhJp8IUqHE3elsSogiUqZS/UGVFsaDqQTdOchQuugf8sLNDmxdqXP8KpCkeJ1int19J4vzulF6S/Uf/jEH8HliAAAAABJRU5ErkJggg==';

let mainWindow;
let voiceWindow;
let apiProcess;
let apiPid = null;
let tray;
let isQuitting = false;

// Prod mode: load minified dist/ even when unpackaged (dev launcher uses
// `npm run electron:dev` → vite URL + DevTools). Set ELECTRON_PROD=1 in
// start_desktop_prod.bat for the lightweight daily driver.
const isProd = process.env.ELECTRON_PROD === '1' || app.isPackaged;
const isDev = !isProd;

// Single-instance: a second launch focuses the running app and exits — never
// a second Electron + second API server warming duplicate models (2×RAM).
const gotLock = app.requestSingleInstanceLock();
if (!gotLock) {
  app.quit();
}

// Windows taskbar / notifications identity (must match electron-builder appId).
if (process.platform === 'win32') app.setAppUserModelId('com.stark.workspace');

function sendToMain(channel, payload) {
  if (mainWindow && !mainWindow.isDestroyed()) {
    mainWindow.webContents.send(channel, payload);
  }
}

function killPort(port) {
  // Kill whatever holds the port so the API server can always bind fresh.
  // Uses taskkill /F /T to kill the full tree (python.exe spawned via cmd /c
  // from start_api.bat leaves a detached handle that process.kill alone misses).
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve();
    exec(`netstat -ano | findstr ":${port}" | findstr "LISTENING"`, (err, stdout) => {
      if (!stdout) return resolve();
      const pids = new Set();
      for (const line of stdout.trim().split('\n')) {
        const parts = line.trim().split(/\s+/);
        const pid = parts[parts.length - 1];
        if (pid && /^\d+$/.test(pid) && Number(pid) !== process.pid) pids.add(pid);
      }
      if (!pids.size) return resolve();
      let pending = pids.size;
      for (const pid of pids) {
        exec(`taskkill /F /T /PID ${pid}`, () => {
          console.log(`[API] killed stale server PID ${pid}`);
          if (--pending === 0) setTimeout(resolve, 800);
        });
      }
    });
  });
}

function startApiServer() {
  const voicePython = process.env.VOICE_PYTHON && fs.existsSync(process.env.VOICE_PYTHON) ? process.env.VOICE_PYTHON : null;
  const venvPython = path.join(STARK_ROOT, 'venv', 'Scripts', 'python.exe');
  const venvDotPython = path.join(STARK_ROOT, '.venv', 'Scripts', 'python.exe');
  const venv312Python = path.join(STARK_ROOT, 'venv312', 'Scripts', 'python.exe');
  
  let pythonCmd = process.platform === 'win32' ? 'python' : 'python3';
  if (voicePython) {
    pythonCmd = voicePython;
  } else if (fs.existsSync(venvPython) && fs.existsSync(path.join(STARK_ROOT, 'venv', 'pyvenv.cfg'))) {
    pythonCmd = venvPython;
  } else if (fs.existsSync(venvDotPython) && fs.existsSync(path.join(STARK_ROOT, '.venv', 'pyvenv.cfg'))) {
    pythonCmd = venvDotPython;
  } else if (fs.existsSync(venv312Python) && fs.existsSync(path.join(STARK_ROOT, 'venv312', 'pyvenv.cfg'))) {
    pythonCmd = venv312Python;
  }
  const apiPath = path.join(STARK_ROOT, 'server', 'api_server.py');
  const STARK_PORT = process.env.STARK_PORT || '8123';

  killStaleApiServers().then(() => {
    killPort(Number(STARK_PORT)).then(() => {
      // NOTE: never pass --reload here. Electron already self-heals the API
      // process on exit (restart in 3s below), so uvicorn's reloader adds a
      // second/third python process (it can resolve to a DIFFERENT interpreter
      // from PATH, e.g. C:\...\Python314\python.exe instead of venv) that
      // double-binds port 8000 and kills the app's health checks (ECONNREFUSED
      // + slow chat load). A single venv process with reload disabled is the
      // reliable configuration for the desktop app.
      const args = [apiPath];

      apiProcess = spawn(pythonCmd, args, {
        cwd: STARK_ROOT,
        env: { ...process.env, STARK_PORT },
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      apiPid = apiProcess.pid || null;
      setApiPid(apiPid);

    apiProcess.stdout.on('data', (data) => {
      console.log(`[API] ${data.toString().trim()}`);
    });

    apiProcess.stderr.on('data', (data) => {
      console.error(`[API err] ${data.toString().trim()}`);
    });

    apiProcess.on('error', (err) => {
      console.error('Failed to start API server:', err.message);
    });

    // Self-heal: restart the API server if it dies while the app is open.
    // The app is "open" while the tray lives — the main window may be
    // destroyed (lightweight close) with voice still running.
    apiProcess.on('exit', (code) => {
      console.error(`[API] server exited (${code})`);
      apiPid = null;
      setApiPid(null);
      if (!isQuitting) {
        console.log('[API] restarting in 3s...');
        setTimeout(() => startApiServer(), 3000);
      }
    });
    });
  });
}

// Kill EVERY python running api_server.py (not just the port holder) so a
// second Electron / leftover start_api.bat can't leave a duplicate model
// resident — the root cause of the 2×RAM/CPU + choppy TTS + red-log storm.
function killStaleApiServers() {
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve();
    exec('wmic process where "name=\'python.exe\'" get processid,commandline', (err, stdout) => {
      if (err || !stdout) return resolve();
      const pids = new Set();
      for (const line of stdout.split('\n')) {
        if (line.toLowerCase().includes('api_server.py')) {
          const parts = line.trim().split(/\s+/);
          const pid = parts[parts.length - 1];
          if (pid && /^\d+$/.test(pid) && Number(pid) !== process.pid) pids.add(pid);
        }
      }
      if (!pids.size) return resolve();
      let pending = pids.size;
      for (const pid of pids) {
        exec(`taskkill /F /T /PID ${pid}`, () => {
          console.log(`[API] killed stale api_server PID ${pid}`);
          if (--pending === 0) setTimeout(resolve, 500);
        });
      }
    });
  });
}

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1440,
    height: 900,
    minWidth: 1100,
    minHeight: 700,
    title: 'Stark — Agent Workspace',
    icon: path.join(__dirname, 'icon.png'),
    backgroundColor: '#0c0c0e',
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'preload.cjs'),
    },
    autoHideMenuBar: true,
    titleBarStyle: 'hidden',
    titleBarOverlay: process.platform === 'win32' ? {
      color: '#0c0c0e',
      symbolColor: '#a1a1aa',
      height: 38
    } : true,
  });

  mainWindow.maximize();

  // Lightweight-by-design: closing the window DESTROYS the whole renderer
  // (+ GPU process RSS) instead of hiding it. Tray + API server + voice
  // overlay survive; tray-open re-creates the window (cold React mount
  // ~1-2s, accepted tradeoff). No 'close' preventDefault on purpose.
  mainWindow.on('closed', () => {
    mainWindow = null;
  });

  // ── Forward renderer console to main process stdout ──
  mainWindow.webContents.on('console-message', (_event, _level, message) => {
    console.log(`[Renderer] ${message}`);
  });

  // ── Log page load failures ──
  mainWindow.webContents.on('did-fail-load', (_event, errorCode, errorDesc, validatedURL) => {
    console.error(`[Electron] Failed to load ${validatedURL}: ${errorCode} — ${errorDesc}`);
  });

  mainWindow.webContents.on('did-finish-load', () => {
    console.log(`[Electron] Page loaded: ${mainWindow.webContents.getURL()}`);
  });

  if (isDev) {
    mainWindow.loadURL('http://localhost:9000');
    mainWindow.webContents.openDevTools({ mode: 'detach' });
  } else {
    // Prod: same-origin via the local API server, which serves dist/ + /api
    // on one origin (see api_server.py spa_fallback). This is why prod works
    // at all: dist/index.html uses absolute /assets + relative /api paths,
    // both of which are broken over file:// (blank black window).
    clearRendererCaches(mainWindow).finally(() => waitForApiThenLoad(mainWindow));
  }

  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: 'deny' };
  });
}

// ── Voice floating overlay ──
function openVoiceWindow() {
  if (voiceWindow && !voiceWindow.isDestroyed()) return;
  const displays = screen.getAllDisplays();
  const primary = displays.find(d => d.bounds.x === 0 && d.bounds.y === 0) || displays[0];
  const wx = primary.bounds.x + primary.bounds.width - 96;
  const wy = primary.bounds.y + 96;
  voiceWindow = new BrowserWindow({
    width: 84,
    height: 84,
    x: wx,
    y: wy,
    frame: false,
    transparent: true,
    alwaysOnTop: true,
    skipTaskbar: true,
    resizable: false,
    focusable: false,
    show: false,
    backgroundColor: '#00000000',
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      preload: path.join(__dirname, 'voice-preload.cjs'),
    },
  });
  voiceWindow.setAlwaysOnTop(true, 'screen-saver');
  voiceWindow.loadFile(VOICE_HTML);
  voiceWindow.on('closed', () => {
    voiceWindow = null;
    sendToMain('voice-session-event', { type: 'closed' });
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.webContents.send('voice-window-closed');
    }
  });
  voiceWindow.showInactive();
  sendToMain('voice-session-event', { type: 'opened' });
}

function toggleVoice() {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    voiceWindow.close();
  } else {
    openVoiceWindow();
  }
}

// ── IPC: Voice overlay ──
ipcMain.handle('voice-toggle', () => {
  toggleVoice();
  return !!(voiceWindow && !voiceWindow.isDestroyed());
});

// Overlay → main window session events (listening / thinking / speaking / transcript...)
ipcMain.on('voice-session-event', (_event, payload) => {
  sendToMain('voice-session-event', payload);
});

// Main window → overlay: tell it which chat is currently active
ipcMain.on('voice-set-chat', (_event, payload) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    voiceWindow.webContents.send('voice-set-chat', payload);
  }
});

// Overlay → resize itself (compact circle ↔ expanded process feed ↔ screen
// panel). `animate` runs an eased morph for the orb ↔ screen-panel transition;
// VOICE_NO_RESIZE_ANIM=1 forces the instant path (safe fallback). The resizer
// is a shared module so smoke tests exercise the same implementation.
const { createVoiceResizer } = require('./resizeAnim.cjs');
const voiceResizer = createVoiceResizer();

ipcMain.on('voice-resize', (_event, w, h, animate) => {
  voiceResizer.resize(voiceWindow, w, h, animate);
});

// Overlay close button
ipcMain.on('voice-close', () => {
  if (voiceWindow && !voiceWindow.isDestroyed()) voiceWindow.close();
});

// ── IPC: Screen sharing ("look at screen" mode) ──
// The overlay lists sources (desktopCapturer, main-process only), then calls
// getDisplayMedia; the request handler below resolves the pending selection —
// no OS picker, no default-screen surprise when a source was chosen.
// A short-lived cache avoids re-enumerating (thumbnail generation is
// expensive) both for the picker and for the label lookup after selection.
let pendingScreenSourceId = null;
let screenSourceCache = { at: 0, raw: [], list: [] };
const SCREEN_SOURCE_TTL_MS = 10000;

async function listScreenSourcesCached() {
  const now = Date.now();
  if (screenSourceCache.list.length && now - screenSourceCache.at < SCREEN_SOURCE_TTL_MS) {
    return screenSourceCache.list;
  }
  const sources = await desktopCapturer.getSources({
    types: ['screen', 'window'],
    thumbnailSize: { width: 320, height: 200 },
    fetchWindowIcons: false,
  });
  // Never offer our own windows as share targets: the overlay/main window
  // would otherwise appear as ("meaningless, mirror-prone") picker entries.
  // getMediaSourceId() matches the desktopCapturer id of the same window.
  const ownIds = new Set();
  for (const w of [voiceWindow, mainWindow]) {
    try {
      if (w && !w.isDestroyed()) ownIds.add(w.getMediaSourceId());
    } catch (_e) {}
  }
  const kept = sources.filter((source) => {
    if (ownIds.has(source.id)) return false;
    if (!source.name || !source.name.trim()) return false;
    return true;
  });
  const list = kept.map((source) => ({
    id: source.id,
    name: source.name,
    kind: source.id.startsWith('screen:') ? 'screen' : 'window',
    display_id: source.display_id || '',
    thumbnail: source.thumbnail && !source.thumbnail.isEmpty() ? source.thumbnail.toDataURL() : '',
  }));
  // Keep the raw DesktopCapturerSource objects too — the getDisplayMedia
  // request handler needs them for the callback.
  screenSourceCache = { at: now, raw: kept, list };
  return list;
}

ipcMain.handle('voice-screen-sources', async () => {
  try {
    return await listScreenSourcesCached();
  } catch (err) {
    console.error('[Electron] screen sources failed:', err.message);
    return [];
  }
});

ipcMain.handle('voice-screen-select', async (_event, sourceId) => {
  try {
    const list = await listScreenSourcesCached();
    // No explicit pick → the primary screen (first 'screen' entry), so the
    // caller never needs a second enumeration just to learn the label.
    const chosen = sourceId
      ? list.find((s) => s.id === sourceId)
      : (list.find((s) => s.kind === 'screen') || list[0]);
    pendingScreenSourceId = chosen ? chosen.id : null;
    return { ok: !!chosen, id: chosen ? chosen.id : '', name: chosen ? chosen.name : '' };
  } catch (err) {
    console.error('[Electron] screen select failed:', err.message);
    return { ok: false, id: '', name: '' };
  }
});

// While sharing, exclude the overlay itself from capture (WDA_EXCLUDEFROMCAPTURE)
// so the user never sees the preview mirrored inside the shared frame.
ipcMain.on('voice-screen-protect', (_event, on) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    try {
      voiceWindow.setContentProtection(!!on);
    } catch (_e) {}
  }
});

// Legacy channels (kept for compatibility with older renderer code)
ipcMain.on('voice-text', (_, text) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    const js = 'document.getElementById("voice-text") && (document.getElementById("voice-text").textContent = ' + JSON.stringify(text || '') + ');';
    voiceWindow.webContents.executeJavaScript(js).catch(() => {});
  }
});

ipcMain.on('voice-status', (_, status) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    const js = status === 'listening'
      ? 'document.getElementById("dot") && (document.getElementById("dot").className = "listening");'
      : 'document.getElementById("dot") && (document.getElementById("dot").className = "idle");';
    voiceWindow.webContents.executeJavaScript(js).catch(() => {});
  }
});

// ── IPC: Native folder picker ──
ipcMain.handle('select-folder', async () => {
  // The main window may be destroyed (lightweight close) — recreate first
  // so the dialog has a parent.
  if (!mainWindow || mainWindow.isDestroyed()) showMainWindow();
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ['openDirectory'],
    title: 'Select Workspace Folder',
  });
  if (result.canceled || !result.filePaths.length) return null;
  return result.filePaths[0];
});

// ── Tray ──
function setApiPid(pid) {
  apiPid = pid || null;
  try {
    if (tray && !tray.isDestroyed()) {
      tray.setToolTip(apiPid
        ? `Stark — Voice Assistant (Ctrl+G) — API PID ${apiPid}`
        : 'Stark — Voice Assistant (Ctrl+G)');
    }
  } catch (_e) {}
}
function createTray() {
  const icon = nativeImage.createFromDataURL(TRAY_ICON_B64);
  tray = new Tray(icon);
  tray.setToolTip('Stark — Voice Assistant (Ctrl+G)');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Open Stark', click: () => showMainWindow() },
    { label: 'Toggle Voice Mode', click: () => toggleVoice() },
    { type: 'separator' },
    { label: 'Quit Stark', click: () => { isQuitting = true; app.quit(); } },
  ]));
  tray.on('double-click', () => showMainWindow());
}

function showMainWindow() {
  if (!mainWindow || mainWindow.isDestroyed()) {
    createWindow();
    return;
  }
  mainWindow.show();
  mainWindow.focus();
}

function checkApiRunning() {
  return new Promise((resolve) => {
    const port = Number(process.env.STARK_PORT || '8123');
    const req = http.request({
      hostname: '127.0.0.1',
      port,
      path: '/api/health',
      method: 'GET',
      timeout: 1000
    }, (res) => {
      resolve(res.statusCode === 200);
    });
    req.on('error', () => {
      resolve(false);
    });
    req.on('timeout', () => {
      req.destroy();
      resolve(false);
    });
    req.end();
  });
}

// Prod boot: cold torch/ONNX imports make the first server start slow, so
// poll /api/health (1s cadence, 90s bound) before loading the UI. Without
// this the window paints before /api (and the SPA shell) can answer.
function waitForApiThenLoad(win, tries = 90) {
  checkApiRunning().then((ok) => {
    if (win.isDestroyed()) return;
    if (ok || tries <= 0) {
      const port = process.env.STARK_PORT || '8123';
      if (!ok) console.error('[Electron] API not healthy after 90s — loading UI anyway (expect backend errors).');
      let buildStamp = Date.now();
      try { buildStamp = Math.floor(fs.statSync(path.join(FRONTEND_ROOT, 'dist', 'index.html')).mtimeMs); } catch {}
      // The changing query also bypasses any renderer navigation cache left by
      // an older build, while the cache clear above removes the PWA worker.
      win.loadURL(`http://127.0.0.1:${port}/?stark_build=${buildStamp}`);
      return;
    }
    setTimeout(() => waitForApiThenLoad(win, tries - 1), 1000);
  });
}

function killApiProcess() {
  if (apiProcess && !apiProcess.killed) {
    const pid = apiProcess.pid;
    if (process.platform === 'win32') {
      try {
        // Sync kill so the Python CFFI stream has time to stop before app exits (fixes zombie 3.5GB + FFI popup)
        const { spawnSync } = require('child_process');
        spawnSync('taskkill', ['/pid', pid, '/f', '/t'], { stdio: 'ignore' });
        console.log(`[Electron] API server process tree (PID ${pid}) killed.`);
      } catch (err) {
        console.error('Failed to kill API server process tree:', err.message);
        try { apiProcess.kill(); } catch(_e){}
      }
    } else {
      try { apiProcess.kill('SIGTERM'); } catch(_e){}
      // Give PortAudio 0.4s to drain like audio_engine.stop()
      const start = Date.now();
      while (Date.now() - start < 600) { require('child_process').spawnSync('sleep', ['0.1']); }
    }
    apiProcess = null;
  }
}

app.whenReady().then(async () => {
  // Mic permission for every window (overlay included)
  session.defaultSession.setPermissionRequestHandler((_wc, permission, callback) => {
    const allowed = ['media', 'microphone'];
    callback(allowed.includes(permission));
  });

  // getDisplayMedia for the overlay's "look at screen" mode: resolve to the
  // source the picker confirmed (voice-screen-select) or the primary screen.
  session.defaultSession.setDisplayMediaRequestHandler((_request, callback) => {
    const resolveWith = (raw) => {
      const source = raw.find((s) => s.id === pendingScreenSourceId) || raw[0];
      if (!source) return callback({});
      callback({ video: source });
    };
    if (screenSourceCache.raw.length && Date.now() - screenSourceCache.at < SCREEN_SOURCE_TTL_MS) {
      return resolveWith(screenSourceCache.raw);
    }
    desktopCapturer.getSources({ types: ['screen', 'window'] })
      .then(resolveWith)
      .catch(() => callback({}));
  }, { useSystemPicker: false });

  // Global shortcut so Ctrl+G works even when the app window is closed
  globalShortcut.register('CommandOrControl+G', () => toggleVoice());

  createTray();

  // Production desktop launches always rebuild from the workspace's local
  // toolchain before starting the API. This makes the installed EXE pick up
  // every frontend edit even when timestamps are unreliable or an editor
  // rewrites a file in place. SKIP_FRONTEND_BUILD=1 remains an escape hatch.
  if (isProd && process.env.SKIP_FRONTEND_BUILD !== '1') {
    buildFrontendSync();
  } else if (isFrontendStale()) {
    buildFrontendSync();
  }

  // Always ensure a fresh server — a stale one on :8123 may be old code
  // (e.g. after `start_api.bat` left a detached handle, or a previous app
  // run's child survived the window close). Kill it and start fresh.
  const isRunning = await checkApiRunning();
  if (isRunning) {
    console.log('[Electron] Stale API server on :8123 — killing and restarting fresh...');
    await killPort(Number(process.env.STARK_PORT || '8123'));
  }
  console.log('[Electron] Starting API server...');
  startApiServer();
  setTimeout(() => {
    createWindow();
    // After the window exists, watch src so edits live-reload without restart.
    setTimeout(setupFrontendWatcher, 2000);
  }, 1500);
});

// Stay alive in the tray so voice mode keeps working when the window is closed.
app.on('window-all-closed', () => {
  // no-op on purpose
});

app.on('activate', () => {
  showMainWindow();
});

// Second launch (single-instance lock): focus the running app, no dupes.
app.on('second-instance', () => {
  showMainWindow();
});

app.on('before-quit', () => {
  isQuitting = true;
  killApiProcess();
});

app.on('will-quit', () => {
  globalShortcut.unregisterAll();
});
