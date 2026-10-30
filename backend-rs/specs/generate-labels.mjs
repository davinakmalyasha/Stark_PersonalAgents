// Generates frontend/src/utils/toolLabels.generated.ts from the unified
// Rust inventory (backend-rs/crates/stark-tools/src/inventory.rs).
// Run from the repo root: node backend-rs/specs/generate-labels.mjs
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', '..');
const rs = fs.readFileSync(
  path.join(root, 'backend-rs', 'crates', 'stark-tools', 'src', 'inventory.rs'),
  'utf8',
);
const re = /ToolMeta\s*\{\s*name:\s*"([^"]+)",\s*plan_write:\s*(?:true|false),\s*read_blocked:\s*(?:true|false),\s*verb:\s*"([^"]+)",\s*summary:\s*"([^"]+)"\s*\}/g;
const entries = [...rs.matchAll(re)].map((m) => ({ name: m[1], verb: m[2], summary: m[3] }));
if (!entries.length) throw new Error('no ToolMeta entries parsed from inventory.rs');
entries.sort((a, b) => a.name.localeCompare(b.name));

const q = (s) => `'${s.replace(/\\/g, '\\\\').replace(/'/g, "\\'")}'`;
const lines = [
  '// GENERATED from backend-rs/crates/stark-tools/src/inventory.rs — do not edit by hand.',
  '// Regenerate with: node backend-rs/specs/generate-labels.mjs',
  'export const TOOL_VERBS: Record<string, string> = {',
  ...entries.map((e) => `  ${e.name}: ${q(e.verb)},`),
  '};',
  'export const TOOL_SUMMARIES: Record<string, string> = {',
  ...entries.map((e) => `  ${e.name}: ${q(e.summary)},`),
  '};',
  '',
];
fs.writeFileSync(
  path.join(root, 'frontend', 'src', 'utils', 'toolLabels.generated.ts'),
  lines.join('\n'),
);
console.log(`wrote toolLabels.generated.ts (${entries.length} tools)`);
