/**
 * Electron window-morph smoke test.
 *
 * Loads the REAL voice.html + voice-preload, then drives the production
 * resize IPC and samples window bounds over time to prove:
 *   1. animate=true tweens size (intermediate frames between 84 and 380)
 *   2. the right edge is anchored (x + width stays constant)
 *   3. animate=false is instant
 *   4. shrink morph works and settles exactly
 *
 * Run:  frontend\\node_modules\\.bin\\electron scratch\\electron_resize_smoke.cjs
 */
const { app, BrowserWindow, ipcMain } = require('electron');
const path = require('path');

// Production resizer (same module main.cjs uses) + the same IPC wiring.
const { createVoiceResizer } = require('../frontend/electron/resizeAnim.cjs');
const voiceResizer = createVoiceResizer();

const VOICE_HTML = path.join(__dirname, '..', 'frontend', 'electron', 'voice.html');
const VOICE_PRELOAD = path.join(__dirname, '..', 'frontend', 'electron', 'voice-preload.cjs');

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

app.whenReady().then(async () => {
  const win = new BrowserWindow({
    width: 84,
    height: 84,
    x: 300,
    y: 200,
    frame: false,
    transparent: true,
    show: false,
    webPreferences: { contextIsolation: true, preload: VOICE_PRELOAD },
  });
  ipcMain.on('voice-resize', (_event, w, h, animate) => {
    voiceResizer.resize(win, w, h, animate);
  });
  await win.loadFile(VOICE_HTML);
  await sleep(400);

  const results = [];
  const snap = () => win.getBounds();

  // ── 1. Grow with animation ──
  const before = snap();
  await win.webContents.executeJavaScript('window.voiceBridge.resize(380, 520, true)');
  const samples = [];
  for (let i = 0; i < 8; i++) {
    await sleep(45);
    samples.push(snap());
  }
  await sleep(300);
  const afterGrow = snap();
  const widths = samples.map((b) => b.width);
  const intermediate = widths.filter((w) => w > before.width + 4 && w < 380 - 4);
  const rightEdgeBefore = before.x + before.width;
  const rightEdgeAfter = afterGrow.x + afterGrow.width;
  const anchored = Math.abs(rightEdgeBefore - rightEdgeAfter) <= 2;
  const grew = afterGrow.width === 380 && afterGrow.height === 520;
  console.log(`[resize] grow samples: ${widths.join(', ')}`);
  console.log(`[resize] intermediate frames: ${intermediate.length} | anchored: ${anchored} (${rightEdgeBefore} -> ${rightEdgeAfter}) | final: ${afterGrow.width}x${afterGrow.height}`);
  results.push(intermediate.length >= 2, anchored, grew);

  // ── 2. Shrink with animation back to orb ──
  await win.webContents.executeJavaScript('window.voiceBridge.resize(84, 84, true)');
  await sleep(450);
  const afterShrink = snap();
  console.log(`[resize] shrink -> ${afterShrink.width}x${afterShrink.height} at x=${afterShrink.x}`);
  results.push(afterShrink.width === 84 && afterShrink.height === 84);

  // ── 3. Instant path (animate=false) ──
  await win.webContents.executeJavaScript('window.voiceBridge.resize(380, 520, false)');
  const instantaneous = snap();
  console.log(`[resize] instant -> ${instantaneous.width}x${instantaneous.height}`);
  results.push(instantaneous.width === 380 && instantaneous.height === 520);

  // ── 4. Rapid retarget: grow then shrink mid-flight, must settle cleanly ──
  await win.webContents.executeJavaScript('window.voiceBridge.resize(84, 84, false)');
  await win.webContents.executeJavaScript('window.voiceBridge.resize(380, 520, true)');
  await sleep(80);
  await win.webContents.executeJavaScript('window.voiceBridge.resize(84, 84, true)');
  await sleep(900);
  let settled = snap();
  if (settled.width !== 84 || settled.height !== 84) {
    // Hidden windows throttle timers; give the last tick a moment.
    await sleep(600);
    settled = snap();
  }
  console.log(`[resize] retarget mid-flight -> ${settled.width}x${settled.height}`);
  results.push(settled.width === 84 && settled.height === 84);

  const ok = results.every(Boolean);
  console.log(`[resize] RESULT: ${ok ? 'PASS' : 'FAIL'}`);
  app.exit(ok ? 0 : 1);
});
