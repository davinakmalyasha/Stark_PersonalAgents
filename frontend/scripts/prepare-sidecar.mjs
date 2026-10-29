import fs from 'node:fs';
import path from 'node:path';

const frontend = path.resolve(import.meta.dirname, '..');
const triple = process.env.TAURI_ENV_TARGET_TRIPLE || 'x86_64-pc-windows-gnu';
const candidates = [
  process.env.STARK_SERVER_BIN,
  'D:/cargo-target/stark-server/release/stark-server.exe',
  path.resolve(frontend, '..', 'backend-rs', 'target', 'release', 'stark-server.exe'),
  path.resolve(frontend, '..', '..', 'cargo-target', 'stark-server', 'release', 'stark-server.exe'),
].filter(Boolean);
const source = candidates.find((candidate) => fs.existsSync(candidate));
const destination = path.join(frontend, 'src-tauri', 'binaries', `stark-server-${triple}.exe`);

if (!source) {
  console.error(`[sidecar] no Rust server binary found; checked:\n${candidates.join('\n')}`);
  console.error('[sidecar] build it with: cargo build --release -p stark-server (backend-rs)');
  process.exit(1);
}

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
