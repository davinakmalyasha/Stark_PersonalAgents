const { app, BrowserWindow, shell, dialog, ipcMain, screen } = require('electron');
const path = require('path');
const fs = require('fs');
const http = require('http');
const { spawn } = require('child_process');
const VOICE_HTML = path.join(__dirname, 'voice.html');

let mainWindow;
let voiceWindow;
let apiProcess;

const isDev = !app.isPackaged;

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

  // Grant microphone permission for voice mode
  mainWindow.webContents.session.setPermissionRequestHandler((_wc, permission, callback) => {
    const allowed = ['media', 'microphone'];
    callback(allowed.includes(permission));
  });

  mainWindow.on('closed', () => {
    mainWindow = null;
  });
}

// ── IPC: Voice floating window ──
ipcMain.handle('voice-toggle', () => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    voiceWindow.close();
    voiceWindow = null;
    return false;
  }
  const displays = screen.getAllDisplays();
  const primary = displays.find(d => d.bounds.x === 0 && d.bounds.y === 0) || displays[0];
  const wx = primary.bounds.x + primary.bounds.width - 300;
  const wy = primary.bounds.y + 80;
  voiceWindow = new BrowserWindow({
    width: 280,
    height: 68,
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
    },
  });
  voiceWindow.loadFile(VOICE_HTML);
  voiceWindow.on('closed', () => {
    voiceWindow = null;
    if (mainWindow && !mainWindow.isDestroyed()) {
      mainWindow.webContents.send('voice-window-closed');
    }
  });
  voiceWindow.showInactive();
  return true;
});

ipcMain.on('voice-text', (_, text) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    const js = 'document.getElementById("voice-text").textContent = ' + JSON.stringify(text || '');
    voiceWindow.webContents.executeJavaScript(js).catch(() => {});
  }
});

ipcMain.on('voice-status', (_, status) => {
  if (voiceWindow && !voiceWindow.isDestroyed()) {
    if (status === 'listening') {
      const js1 = 'document.getElementById("dot").className = "listening";';
      const js2 = 'document.getElementById("label").textContent = "Listening...";';
      voiceWindow.webContents.executeJavaScript(js1 + js2).catch(() => {});
    } else {
      const js1 = 'document.getElementById("dot").className = "idle";';
      const js2 = 'document.getElementById("label").textContent = "Voice";';
      voiceWindow.webContents.executeJavaScript(js1 + js2).catch(() => {});
    }
  }
});

// ── IPC: Native folder picker ──// ── IPC: Native folder picker ──
ipcMain.handle('select-folder', async () => {
  const result = await dialog.showOpenDialog(mainWindow, {
    properties: ['openDirectory'],
    title: 'Select Workspace Folder',
  });
  if (result.canceled || !result.filePaths.length) return null;
  return result.filePaths[0];
});

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
  const isRunning = await checkApiRunning();
  if (isRunning) {
    console.log('[Electron] API server is already running on port 8000. Skipping auto-start.');
  } else {
    console.log('[Electron] Starting API server...');
    startApiServer();
  }
  setTimeout(createWindow, 1500);
});

app.on('window-all-closed', () => {
  if (voiceWindow && !voiceWindow.isDestroyed()) voiceWindow.close();
  killApiProcess();
  if (process.platform !== 'darwin') app.quit();
});

app.on('activate', () => {
  if (BrowserWindow.getAllWindows().length === 0) createWindow();
});

app.on('before-quit', () => {
  killApiProcess();
});
