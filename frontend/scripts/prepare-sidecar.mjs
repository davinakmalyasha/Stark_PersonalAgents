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
