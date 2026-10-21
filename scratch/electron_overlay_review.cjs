/**
 * Real-Electron overlay review harness — desktop only, no browser.
 *
 * Builds the overlay window EXACTLY like production openVoiceWindow()
 * (frameless, transparent, always-on-top, right-edge anchored, real
 * voice-preload.cjs), registers the SAME screen IPC surface as main.cjs
 * (desktopCapturer sources, select, content protection, display-media
 * handler, shared resizeAnim tween), boots the real backend, then drives
 * REAL user flows with DOM clicks and produces review PNGs composited over
 * the actual desktop:
 *
 *   review-electron-01-orb.png            compact voice orb
 *   review-electron-02-menu.png           hamburger menu (personal, 4 items)
 *   review-electron-03-picker.png         source picker, REAL sources
 *   review-electron-04-live.png           screen mode LIVE, REAL capture
 *   review-electron-05-popover.png        preview popover open, REAL preview
 *   review-electron-06-paused.png         paused state
 *   review-electron-07-off.png            off state + Share CTA
 *   review-electron-08-morph.png          mid-tween orb->panel frame
 *   review-electron-09-thinking.png       thinking shimmer (scripted status)
 *
 * Run:  frontend\\node_modules\\.bin\\electron scratch\\electron_overlay_review.cjs
 */
const { app, BrowserWindow, desktopCapturer, session, ipcMain, screen } = require('electron');
const { spawn, execSync } = require('child_process');
const path = require('path');
const fs = require('fs');

const ROOT = path.join(__dirname, '..');
const VOICE_HTML = path.join(ROOT, 'frontend', 'electron', 'voice.html');
const VOICE_PRELOAD = path.join(ROOT, 'frontend', 'electron', 'voice-preload.cjs');
const { createVoiceResizer } = require('../frontend/electron/resizeAnim.cjs');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const log = (...a) => console.log('[review]', ...a);

// Deterministic profile: no saved source/mic state from real usage or prior
// runs (a persisted screenSourceId would skip the first-time picker).
const PROFILE_DIR = path.join(__dirname, '_review_profile');
app.setPath('userData', PROFILE_DIR);
try {
  fs.rmSync(PROFILE_DIR, { recursive: true, force: true });
} catch (_e) {}

let win = null;
let apiProc = null;
let apiSpawned = false;

async function apiUp() {
  try {
    const r = await fetch('http://127.0.0.1:8123/api/voice/status');
    return r.ok;
  } catch (_e) {
    return false;
  }
}

async function ensureBackend() {
  if (await apiUp()) {
    log('backend already running, reusing');
    return;
  }
  log('booting backend…');
  const py = path.join(ROOT, 'venv', 'Scripts', 'python.exe');
  apiProc = spawn(py, ['server/api_server.py'], {
    cwd: ROOT,
    env: { ...process.env, START_TELEGRAM: '0', STARK_PORT: '8123' },
    stdio: 'ignore',
    windowsHide: true,
  });
  apiSpawned = true;
  for (let i = 0; i < 90; i++) {
    await sleep(2000);
    if (await apiUp()) {
      log('backend up');
      return;
    }
  }
  throw new Error('backend did not come up');
}

function killBackend() {
  if (!apiSpawned || !apiProc || !apiProc.pid) return;
  try {
    execSync(`taskkill /F /T /PID ${apiProc.pid}`, { stdio: 'ignore' });
  } catch (_e) {}
}

// ── Production-identical screen IPC ──
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
  const list = sources.map((source) => ({
    id: source.id,
    name: source.name,
    kind: source.id.startsWith('screen:') ? 'screen' : 'window',
    display_id: source.display_id || '',
    thumbnail: source.thumbnail && !source.thumbnail.isEmpty() ? source.thumbnail.toDataURL() : '',
  }));
  screenSourceCache = { at: now, raw: sources, list };
  return list;
}

function registerScreenIpc() {
  ipcMain.handle('voice-screen-sources', async () => {
    try {
      return await listScreenSourcesCached();
    } catch (err) {
      console.error('[review] screen sources failed:', err.message);
      return [];
    }
  });
  ipcMain.handle('voice-screen-select', async (_event, sourceId) => {
    try {
      const list = await listScreenSourcesCached();
      const chosen = sourceId
        ? list.find((s) => s.id === sourceId)
        : (list.find((s) => s.kind === 'screen') || list[0]);
      pendingScreenSourceId = chosen ? chosen.id : null;
      return { ok: !!chosen, id: chosen ? chosen.id : '', name: chosen ? chosen.name : '' };
    } catch (err) {
      return { ok: false, id: '', name: '' };
    }
  });
  ipcMain.on('voice-screen-protect', (_event, on) => {
    if (win && !win.isDestroyed()) {
      try {
        win.setContentProtection(!!on);
      } catch (_e) {}
    }
  });
  const voiceResizer = createVoiceResizer();
  ipcMain.on('voice-resize', (_event, w, h, animate) => {
    voiceResizer.resize(win, w, h, animate);
  });
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
}

// ── Capture helpers ──
async function desktopGrab(outPath, size) {
  const sources = await desktopCapturer.getSources({
    types: ['screen'],
    thumbnailSize: { width: size.w, height: size.h },
  });
  const primary = sources[0];
  fs.writeFileSync(outPath, primary.thumbnail.toPNG());
}

async function overlayCapture(outPath) {
  const img = await win.capturePage();
  fs.writeFileSync(outPath, img.toPNG());
}

const PY = path.join(ROOT, 'venv', 'Scripts', 'python.exe');
function sh(cmd) {
  execSync(cmd, { cwd: ROOT, stdio: 'pipe' });
}

async function click(sel) {
  await win.webContents.executeJavaScript(
    `(function(){var el=document.querySelector(${JSON.stringify(sel)});if(!el)return 'missing';el.click();return 'ok';})()`
  );
}

async function state() {
  return win.webContents.executeJavaScript(
    `({mode:(function(){try{return document.body.className}catch(e){return ''}})(),` +
    `sharing:(function(){try{return document.querySelector('#screen-preview').classList.contains('live')}catch(e){return false}})()})`
  );
}

async function waitFor(fn, timeoutMs, label) {
  const t0 = Date.now();
  for (;;) {
    let v = false;
    try {
      v = await fn();
    } catch (_e) {}
    if (v) return true;
    if (Date.now() - t0 > timeoutMs) {
      log(`TIMEOUT waiting for ${label}`);
      return false;
    }
    await sleep(150);
  }
}

app.whenReady().then(async () => {
  const results = [];
  const check = (name, ok) => {
    log(`${ok ? 'PASS' : 'FAIL'} ${name}`);
    results.push(ok);
  };
  try {
    await ensureBackend();
    registerScreenIpc();

    const display = screen.getPrimaryDisplay();
    const sf = display.scaleFactor || 1;
    const phys = { w: Math.round(display.size.width * sf), h: Math.round(display.size.height * sf) };
    log(`display ${display.size.width}x${display.size.height} @${sf} -> grab ${phys.w}x${phys.h}`);

    // Backdrop BEFORE the overlay exists.
    const backdrop = path.join(ROOT, 'review-electron-backdrop.png');
    await desktopGrab(backdrop, phys);

    // Production-shaped overlay window: 84x84 orb, anchored like main.cjs.
    const bounds0 = display.bounds;
    win = new BrowserWindow({
      width: 84,
      height: 84,
      x: bounds0.x + bounds0.width - 96,
      y: bounds0.y + 96,
      frame: false,
      transparent: true,
      alwaysOnTop: true,
      skipTaskbar: true,
      resizable: false,
      focusable: false,
      show: false,
      backgroundColor: '#00000000',
      webPreferences: { nodeIntegration: false, contextIsolation: true, preload: VOICE_PRELOAD },
    });
    win.setAlwaysOnTop(true, 'screen-saver');
    await win.loadFile(VOICE_HTML);
    await sleep(600);

    const overlayPos = () => {
      const b = win.getBounds();
      return { x: Math.round(b.x * sf), y: Math.round(b.y * sf), w: Math.round(b.width * sf), h: Math.round(b.height * sf) };
    };
    const shotComposite = (tag) => {
      const tmp = path.join(ROOT, `_review_tmp_${tag}.png`);
      const out = path.join(ROOT, `review-electron-${tag}.png`);
      return win.capturePage().then((img) => {
        fs.writeFileSync(tmp, img.toPNG());
        const p = overlayPos();
        sh(`"${PY}" scratch/composite_shot.py composite "${tmp}" "${backdrop}" ${p.x} ${p.y} "${out}"`);
        try { fs.unlinkSync(tmp); } catch (_e) {}
        log(`shot ${out}`);
        return out;
      });
    };
    const shotDesktopCrop = (tag, pad = 24) => {
      const full = path.join(ROOT, `_review_tmp_full_${tag}.png`);
      const out = path.join(ROOT, `review-electron-${tag}.png`);
      return desktopGrab(full, phys).then(() => {
        const p = overlayPos();
        const { execSync: ex } = require('child_process');
        ex(`"${PY}" -c "from PIL import Image; im=Image.open(r'${full}'); ` +
          `im.crop((${Math.max(0, p.x - pad)}, ${Math.max(0, p.y - pad)}, ${p.x + p.w + pad}, ${p.y + p.h + pad})).save(r'${out}')"`,
          { cwd: ROOT, stdio: 'pipe' });
        try { fs.unlinkSync(full); } catch (_e) {}
        log(`shot ${out}`);
        return out;
      });
    };

    // Bind to the personal workspace (real production pathway).
    win.webContents.send('voice-set-chat', {
      wsId: '_personal', wsPath: '', chatId: 'review', model: 'Automatic Fallback', temperature: 0.7,
    });
    await sleep(300);
    win.showInactive();
    await sleep(500);

    // 01 orb (composited: window hidden from nothing yet, protection off).
    await shotComposite('01-orb');
    check('orb window is 84x84', win.getBounds().width === 84 && win.getBounds().height === 84);

    // 02 menu (real resize path).
    await click('#menu-btn');
    await sleep(1400);
    await shotComposite('02-menu');
    const mb = win.getBounds();
    check('menu window ~286 wide', Math.abs(mb.width - 286) <= 2);

    // 03 picker: menu -> Look at screen (first time, no saved source).
    await click('#menu-screen');
    await waitFor(async () => {
      const n = await win.webContents.executeJavaScript(
        `document.querySelectorAll('.screen-picker-item').length`);
      return n > 0;
    }, 8000, 'picker items');
    await sleep(400);
    await shotComposite('03-picker');
    const items = await win.webContents.executeJavaScript(
      `[...document.querySelectorAll('.screen-picker-item span')].map(e=>e.textContent).join(' | ')`);
    log(`picker items: ${items}`);
    check('picker lists real sources', items.length > 0);
    const scrSize = win.getBounds();
    check('screen window is 300x232', scrSize.width === 300 && scrSize.height === 232);

    // 04 live: pick the first source (real capture). A returning profile may
    // skip the picker and share directly — handle both branches.
    const picked = await win.webContents.executeJavaScript(
      `(function(){var items=document.querySelectorAll('.screen-picker-item');` +
      `if(!items.length)return 'direct';items[0].click();return 'picked';})()`);
    log(`share path: ${picked}`);
    await waitFor(async () => {
      const live = await win.webContents.executeJavaScript(
        `document.querySelector('#screen-preview').classList.contains('live')`);
      return live;
    }, 12000, 'live preview');
    await sleep(800);
    await shotComposite('04-live');
    const vw = await win.webContents.executeJavaScript(
      `({w: document.querySelector('#screen-video').videoWidth, h: document.querySelector('#screen-video').videoHeight})`);
    log(`capture stream: ${vw.w}x${vw.h}`);
    check('real capture stream flowing', vw.w > 0 && vw.h > 0);
    const liveUi = await win.webContents.executeJavaScript(
      `({badge: document.querySelector('#screen-badge').textContent,` +
      ` chip: document.querySelector('#screen-chip-name').textContent,` +
      ` chipsVisible: getComputedStyle(document.querySelector('#screen-chips')).display !== 'none'})`);
    log(`live ui: badge=${liveUi.badge} chip=${liveUi.chip} chipsVisible=${liveUi.chipsVisible}`);
    check('LIVE badge + source chip shown', liveUi.badge === 'LIVE' && liveUi.chipsVisible === true);

    // No-mirror assertion: with protection ON the overlay must be absent
    // from a real desktop grab of its own rect.
    const fullLive = path.join(ROOT, '_review_tmp_fulllive.png');
    await desktopGrab(fullLive, phys);
    const p = overlayPos();
    sh(`"${PY}" scratch/composite_shot.py regiondiff "${fullLive}" "${backdrop}" ${p.x} ${p.y} ${p.w} ${p.h} > "${path.join(ROOT, '_review_tmp_diff.txt')}"`);
    const diffTxt = fs.readFileSync(path.join(ROOT, '_review_tmp_diff.txt'), 'utf8');
    const m = diffTxt.match(/mean=([\d.]+)/);
    const mean = m ? parseFloat(m[1]) : 999;
    log(`mirror check regiondiff mean=${mean}`);
    check('no mirror (overlay absent from own capture)', mean < 12);

    // 05 popover open (real preview).
    await click('#screen-preview-btn');
    await sleep(500);
    const popOpen = await win.webContents.executeJavaScript(
      `document.querySelector('#screen-preview-pop').classList.contains('open')`);
    check('preview popover opens', popOpen === true);
    await shotComposite('05-popover');

    // 06 paused via chip x.
    await win.webContents.executeJavaScript(`document.querySelector('#screen-pop-close').click()`);
    await sleep(200);
    await click('#screen-chip-x');
    await sleep(500);
    const badgeOff = await win.webContents.executeJavaScript(
      `document.querySelector('#screen-badge').textContent`);
    const chipPaused = await win.webContents.executeJavaScript(
      `document.querySelector('#screen-chip').classList.contains('paused')`);
    log(`paused badge=${badgeOff} chipPaused=${chipPaused}`);
    check('chip-x pauses (badge OFF, chip grey)', badgeOff === 'OFF' && chipPaused === true);
    await shotComposite('06-paused');

    // 07 off: menu -> Pause sharing is already off; use menu to show off state.
    // (chip-x already stopped sharing; capture the off state as-is.)
    await shotComposite('07-off');

    // 08 morph mid-flight: exit to orb then re-enter, shooting mid-tween.
    await click('#screen-exit-btn');
    await sleep(700);
    const orbBack = win.getBounds();
    check('exit returns to 84x84 orb', orbBack.width === 84 && orbBack.height === 84);
    await click('#menu-btn');
    await sleep(1300);
    await click('#menu-screen');
    await sleep(110); // mid-tween (~260ms)
    await shotComposite('08-morph');
    await sleep(600);

    // 09 thinking shimmer (scripted status; rendering is real Electron).
    await win.webContents.executeJavaScript(
      `document.getElementById('voice-text').textContent = 'Quarterly revenue grew 12 percent on services strength…';` +
      `document.getElementById('screen-caption-role').textContent = 'THINKING';` +
      `document.body.classList.remove('state-idle','state-listening');` +
      `document.body.classList.add('state-thinking');`);
    await sleep(700);
    const thinkingOn = await win.webContents.executeJavaScript(
      `document.body.classList.contains('state-thinking')`);
    check('thinking state applied', thinkingOn === true);
    await shotComposite('09-thinking');

    const ok = results.every(Boolean);
    log(`RESULT: ${ok ? 'PASS' : 'FAIL'}`);
    try { win.close(); } catch (_e) {}
    killBackend();
    app.exit(ok ? 0 : 1);
  } catch (e) {
    console.error('[review] FATAL', e && e.stack ? e.stack.stack || e.stack : e);
    try { if (win && !win.isDestroyed()) win.close(); } catch (_e) {}
    killBackend();
    app.exit(1);
  }
});
