/**
 * Electron screen-capture smoke test.
 *
 * Validates the exact production mechanism of "look at screen" mode without
 * the full app: desktopCapturer.getSources + setDisplayMediaRequestHandler +
 * getDisplayMedia + canvas frame extraction, writing one JPEG to scratch/.
 *
 * Run:  frontend\\node_modules\\.bin\\electron scratch\\electron_screen_smoke.cjs
 */
const { app, BrowserWindow, desktopCapturer, session } = require('electron');
const path = require('path');
const fs = require('fs');

app.whenReady().then(async () => {
  const sources = await desktopCapturer.getSources({
    types: ['screen', 'window'],
    thumbnailSize: { width: 320, height: 200 },
  });
  console.log(`[smoke] sources: ${sources.length}`);
  for (const s of sources.slice(0, 6)) {
    console.log(`[smoke]   ${s.id.startsWith('screen:') ? 'screen' : 'window'} | ${s.name} | thumb=${!s.thumbnail.isEmpty()}`);
  }
  if (!sources.length) {
    console.log('[smoke] RESULT: FAIL (no sources)');
    app.exit(1);
    return;
  }

  session.defaultSession.setDisplayMediaRequestHandler((_request, callback) => {
    callback({ video: sources[0] });
  }, { useSystemPicker: false });

  const win = new BrowserWindow({ width: 400, height: 300, show: false });
  await win.loadFile(path.join(__dirname, '_screen_smoke_page.html'));

  const deadline = Date.now() + 20000;
  let frame = '';
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 500));
    frame = await win.webContents.executeJavaScript('window.__frame || window.__error || ""').catch(() => '');
    if (frame) break;
  }

  let ok = false;
  if (frame.includes('|')) {
    const [dataUrl, size] = frame.split('|');
    const b64 = dataUrl.split(',')[1] || '';
    const out = path.join(__dirname, 'screen_smoke_frame.jpg');
    fs.writeFileSync(out, Buffer.from(b64, 'base64'));
    console.log(`[smoke] frame captured: ${size}, ${Buffer.byteLength(b64, 'base64')} bytes -> ${out}`);
    const timing = await win.webContents.executeJavaScript('window.__timing || ""').catch(() => '');
    if (timing) console.log(`[smoke] timing: ${timing}`);
    ok = Buffer.byteLength(b64, 'base64') > 1000;
  } else {
    console.log(`[smoke] capture error: ${frame || 'timed out'}`);
  }
  console.log(`[smoke] RESULT: ${ok ? 'PASS' : 'FAIL'}`);
  app.exit(ok ? 0 : 1);
});
