import fs from 'node:fs';
import path from 'node:path';

const frontend = path.resolve(import.meta.dirname, '..');
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || 'x86_64-pc-windows-gnu';
const candidates = [
  process.env.STARK_SERVER_BIN,
  'D:/cargo-target/stark-server/release/stark-server.exe',
  path.resolve(frontend, '..', 'backend-rs', 'target', 'release', 'stark-server.exe'),
  path.resolve(frontend, '..', '..', 'cargo-target', 'stark-server', 'release', 'stark-server.exe'),
  path.resolve(frontend, '..', '..', 'cargo-target', 'stark', 'release', 'stark-server.exe'),
].filter(Boolean);
// Freshness beats order: an explicit env override wins, otherwise the newest
// binary wins. A stale leftover silently shipped once (Oct 27 backend with an
// Oct 30 frontend), resurrecting removed behavior. Never again.
const existing = candidates
  .filter((candidate) => fs.existsSync(candidate))
  .map((candidate) => ({ candidate, mtime: fs.statSync(candidate).mtimeMs }))
  .sort((a, b) => b.mtime - a.mtime);
if (!existing.length) {
  console.error('[sidecar] no Rust server binary found; checked:');
  for (const candidate of candidates) console.error(`  - ${candidate}`);
  console.error('[sidecar] build it with: cargo build --release -p stark-server (backend-rs)');
  process.exit(1);
}
const explicit = process.env.STARK_SERVER_BIN;
const chosen = explicit && fs.existsSync(explicit)
  ? { candidate: explicit, mtime: fs.statSync(explicit).mtimeMs }
  : existing[0];
const source = chosen.candidate;
const destination = path.join(frontend, 'src-tauri', 'binaries', `stark-server-${triple}.exe`);

// Refuse to bundle a backend older than the newest Rust source: that means the
// build is stale and the app would ship old tool behavior under a new UI.
const backendRoot = path.resolve(frontend, '..', 'backend-rs');
let newestSource = 0;
const walk = (dir) => {
  let entries;
  try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { return; }
  for (const entry of entries) {
    if (entry.name === 'target' || entry.name === 'models' || entry.name.startsWith('.')) continue;
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) walk(full);
    else if (entry.name.endsWith('.rs') || entry.name.endsWith('.toml')) {
      try { newestSource = Math.max(newestSource, fs.statSync(full).mtimeMs); } catch { /* ignore */ }
    }
  }
};
walk(backendRoot);
if (!explicit && newestSource > 0 && chosen.mtime + 5_000 < newestSource) {
  console.error('[sidecar] refusing stale backend binary:');
  console.error(`  chosen: ${source} (${new Date(chosen.mtime).toISOString()})`);
  console.error(`  newest backend source: ${new Date(newestSource).toISOString()}`);
  console.error('[sidecar] rebuild first: cargo build --release -p stark-server');
  process.exit(1);
}
console.log(`[sidecar] ${source} (${new Date(chosen.mtime).toISOString()}) -> ${destination}`);

fs.mkdirSync(path.dirname(destination), { recursive: true });
fs.copyFileSync(source, destination);
console.log(`[sidecar] ${source} -> ${destination}`);

// Voice assets are not committed (the 18M model is ~112 MB). A release build must either set
// STARK_MODELS_DIR to a prepared directory or place it at backend-rs/models.
const modelsSource =
  process.env.STARK_MODELS_DIR ||
  path.resolve(frontend, '..', 'backend-rs', 'models');
const modelsDest = path.join(frontend, 'src-tauri', 'resources', 'models');
if (fs.existsSync(modelsSource)) {
  fs.rmSync(modelsDest, { recursive: true, force: true });
  fs.mkdirSync(path.dirname(modelsDest), { recursive: true });
  fs.cpSync(modelsSource, modelsDest, { recursive: true });
  console.log(`[voice] ${modelsSource} -> ${modelsDest}`);
} else {
  console.warn(
    `[voice] models not found at ${modelsSource}; set STARK_MODELS_DIR to bundle TTS/STT`,
  );
}

const onnxCandidates = [
  process.env.STARK_ONNXRUNTIME_DLL,
  path.resolve(frontend, '..', 'backend-rs', 'onnxruntime.dll'),
].filter(Boolean);
const onnx = onnxCandidates.find((candidate) => fs.existsSync(candidate));
if (onnx) {
  const target = path.join(frontend, 'src-tauri', 'binaries', 'onnxruntime.dll');
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.copyFileSync(onnx, target);
  console.log(`[voice] ${onnx} -> ${target}`);
} else {
  console.warn('[voice] onnxruntime.dll not found; STT will not run in the packaged app');
}

const licenses = [
  [
    path.resolve(frontend, '..', 'backend-rs', 'crates', 'stark-voice', 'POCKET_TTS_LICENSE'),
    'POCKET_TTS_LICENSE',
  ],
  [
    path.resolve(frontend, '..', 'docs', 'rust-rewrite', 'THIRD_PARTY_VOICE.md'),
    'THIRD_PARTY_VOICE.md',
  ],
];
const licenseDest = path.join(frontend, 'src-tauri', 'resources', 'licenses');
fs.mkdirSync(licenseDest, { recursive: true });
for (const [source, name] of licenses) {
  if (fs.existsSync(source)) {
    fs.copyFileSync(source, path.join(licenseDest, name));
    console.log(`[voice] ${source} -> ${path.join(licenseDest, name)}`);
  }
}
