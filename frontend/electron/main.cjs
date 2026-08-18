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
  return new Promise((resolve) => {
    if (process.platform !== 'win32') return resolve();
    exec(`netstat -ano | findstr ":${port}" | findstr "LISTENING"`, (err, stdout) => {
      if (!stdout) return resolve();
      for (const line of stdout.trim().split('\n')) {
        const parts = line.trim().split(/\s+/);
        const pid = parts[parts.length - 1];
        if (pid && /^\d+$/.test(pid) && Number(pid) !== process.pid) {
          try { process.kill(Number(pid)); console.log(`[API] killed stale server PID ${pid}`); } catch (_e) { /* gone */ }
        }
      }
      resolve();
    });
  });
}

function startApiServer() {
  const venv312Python = path.join(__dirname, '..', '..', 'venv312', 'Scripts', 'python.exe');
  const venvPython = path.join(__dirname, '..', '..', 'venv', 'Scripts', 'python.exe');
  
  let pythonCmd = process.platform === 'win32' ? 'python' : 'python3';
  if (fs.existsSync(venv312Python) && fs.existsSync(path.join(__dirname, '..', '..', 'venv312', 'pyvenv.cfg'))) {
    pythonCmd = venv312Python;
  } else if (fs.existsSync(venvPython) && fs.existsSync(path.join(__dirname, '..', '..', 'venv', 'pyvenv.cfg'))) {
    pythonCmd = venvPython;
  }
  const apiPath = path.join(__dirname, '..', '..', 'api_server.py');

  killPort(8000).then(() => {
    const args = [apiPath];
    if (isDev) {
      args.push('--reload');
    }

    apiProcess = spawn(pythonCmd, args, {
      cwd: path.join(__dirname, '..', '..'),
      env: { ...process.env },
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
    const req = http.request({
      hostname: '127.0.0.1',
      port: 8000,
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
    if (process.platform === 'win32') {
      try {
        spawn('taskkill', ['/pid', apiProcess.pid, '/f', '/t']);
        console.log(`[Electron] API server process tree (PID ${apiProcess.pid}) killed.`);
      } catch (err) {
        console.error('Failed to kill API server process tree:', err.message);
        apiProcess.kill();
      }
    } else {
      apiProcess.kill();
    }
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

  const isRunning = await checkApiRunning();
  if (isRunning) {
    console.log('[Electron] API server is already running on port 8000. Skipping auto-start.');
  } else {
    console.log('[Electron] Starting API server...');
    startApiServer();
  }
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
