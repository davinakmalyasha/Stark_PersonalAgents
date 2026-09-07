const { app, BrowserWindow, shell, dialog, ipcMain, screen, Tray, Menu, globalShortcut, nativeImage, session } = require('electron');
const path = require('path');
const fs = require('fs');
const http = require('http');
const { exec } = require('child_process');
const { spawn } = require('child_process');
const VOICE_HTML = path.join(__dirname, 'voice.html');

// 32x32 tray icon (indigo dot on dark circle)
const TRAY_ICON_B64 = 'iVBORw0KGgoAAAANSUhEUgAAACAAAAAgCAYAAABzenr0AAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAAJdSURBVFhH7ZY9aBRBFMdTprRJ3tybwMYoISFCgh8xCOqJSFBBjoRACCHGREPAJliltDJVsLexvPIqsbGysbQzlaQRAjZHKsuV/+6N2X1vb3Z2c+n8w6/Yu5n3MfPe2x0a+q+aYuY5Itt04FmuGaiiKBom4k1jbJuI/xhj4z50jOHdKIouSRu1BYPG8EmBMw/cbTTsHgKX9oKFLIzhb9p4OET2iIjGpe1SYZMx9lgarAd3USfSR1+lmQ/KuQNX0piSvpRwZ+c9dg/HpcWJwinYWMiVq3fjmWuteHLyofqvP/xe+vynXvbeameeiJsPDuMXL3/H2zunOR4/bcfR+KzakwUtPDISGek7UdpuepMDma6ufVeOs6xv/ExORe7NwwfSdyIi/qwXpyDztfUfymERz7d+eU8CrSl9J5Xvm3CPFj8oRz6etT4pG1lUR4yO2gW5yIHsi+68jMsT88pWJoBWLgD8IBc5cPfSeAizcxvK1lkAdi8XgK8A52+/UcZDuHf/nbLlILJvcwEQ8apc5EBVS+Mh3Lz1WtnKBLAvArBNucgxNjaljIfgG1BIOBcAhoNclKW1/EU58IGWlTayoOhzAUC+dwDGbpVO8A8jPpG+E6Ew9OIzrt94FRTEwp19tVfwUfpOhGvwDSOAk1ha+aqcgrAxjLni+X7E20puKGJ6+knSnmg1ZBziuEdH+swpLUb/G7EuOF01gouECi27ijqo1vMJiwcZhJp8IUqHE3elsSogiUqZS/UGVFsaDqQTdOchQuugf8sLNDmxdqXP8KpCkeJ1int19J4vzulF6S/Uf/jEH8HliAAAAABJRU5ErkJggg==';

let mainWindow;
let voiceWindow;
let apiProcess;
let tray;
let isQuitting = false;

const isDev = !app.isPackaged;

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
  const venvPython = path.join(__dirname, '..', '..', 'venv', 'Scripts', 'python.exe');
  const venvDotPython = path.join(__dirname, '..', '..', '.venv', 'Scripts', 'python.exe');
  const venv312Python = path.join(__dirname, '..', '..', 'venv312', 'Scripts', 'python.exe');
  
  let pythonCmd = process.platform === 'win32' ? 'python' : 'python3';
  if (voicePython) {
    pythonCmd = voicePython;
  } else if (fs.existsSync(venvPython) && fs.existsSync(path.join(__dirname, '..', '..', 'venv', 'pyvenv.cfg'))) {
    pythonCmd = venvPython;
  } else if (fs.existsSync(venvDotPython) && fs.existsSync(path.join(__dirname, '..', '..', '.venv', 'pyvenv.cfg'))) {
    pythonCmd = venvDotPython;
  } else if (fs.existsSync(venv312Python) && fs.existsSync(path.join(__dirname, '..', '..', 'venv312', 'pyvenv.cfg'))) {
    pythonCmd = venv312Python;
  }
  const apiPath = path.join(__dirname, '..', '..', 'server', 'api_server.py');
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
        cwd: path.join(__dirname, '..', '..'),
        env: { ...process.env, STARK_PORT },
        stdio: ['ignore', 'pipe', 'pipe'],
      });

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
    apiProcess.on('exit', (code) => {
      console.error(`[API] server exited (${code})`);
      if (mainWindow && !mainWindow.isDestroyed() && !isQuitting) {
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
    title: 'Antigravity 2.0 — Agent Workspace',
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

  // Closing the window hides it to the tray so voice mode keeps running.
  mainWindow.on('close', (e) => {
    if (!isQuitting) {
      e.preventDefault();
      mainWindow.hide();
    }
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
    mainWindow.loadFile(path.join(__dirname, '..', 'dist', 'index.html'));
  }

  mainWindow.webContents.setWindowOpenHandler(({ url }) => {
    shell.openExternal(url);
    return { action: 'deny' };
  });

  mainWindow.on('closed', () => {
    mainWindow = null;
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

// Overlay → resize itself (compact circle ↔ expanded process feed)
ipcMain.on('voice-resize', (_event, w, h) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    const bounds = voiceWindow.getBounds();
    const nextWidth = Math.round(w);
    // Resize around the current right edge so the mic stays in place while
    // the controls extend beside it. Dragging still determines the edge.
    const x = bounds.x - (nextWidth - bounds.width);
    voiceWindow.setBounds({
      x,
      y: bounds.y,
      width: nextWidth,
      height: Math.round(h),
    });
  }
});

// Overlay close button
ipcMain.on('voice-close', () => {
  if (voiceWindow && !voiceWindow.isDestroyed()) voiceWindow.close();
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
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ['openDirectory'],
    title: 'Select Workspace Folder',
  });
  if (result.canceled || !result.filePaths.length) return null;
  return result.filePaths[0];
});

// ── Tray ──
function createTray() {
  const icon = nativeImage.createFromDataURL(TRAY_ICON_B64);
  tray = new Tray(icon);
  tray.setToolTip('VinCode — Voice Assistant (Ctrl+G)');
  tray.setContextMenu(Menu.buildFromTemplate([
    { label: 'Open VinCode', click: () => showMainWindow() },
    { label: 'Toggle Voice Mode', click: () => toggleVoice() },
    { type: 'separator' },
    { label: 'Quit VinCode', click: () => { isQuitting = true; app.quit(); } },
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

  // Global shortcut so Ctrl+G works even when the app window is closed
  globalShortcut.register('CommandOrControl+G', () => toggleVoice());

  createTray();

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
  setTimeout(createWindow, 1500);
});

// Stay alive in the tray so voice mode keeps working when the window is closed.
app.on('window-all-closed', () => {
  // no-op on purpose
});

app.on('activate', () => {
  showMainWindow();
});

app.on('before-quit', () => {
  isQuitting = true;
  killApiProcess();
});

app.on('will-quit', () => {
  globalShortcut.unregisterAll();
});
