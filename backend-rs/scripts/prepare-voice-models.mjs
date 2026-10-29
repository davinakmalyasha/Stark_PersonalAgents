#!/usr/bin/env node
/**
 * Prepare the local voice asset tree used by release builds.
 *
 * Layout:
 *   models/tts/english_18m_2026-10/model.safetensors
 *   models/tts/english_18m_2026-10/tokenizer.json
 *   models/tts/voices/{jarvis,alba,anna}.safetensors
 *   models/stt/tiny-streaming-en/*.ort + tokenizer.bin
 *
 * The public TTS weights/tokenizer/catalog voices are downloaded from the non-gated
 * `kyutai/pocket-tts-without-voice-cloning` repository. JARVIS must be exported once with the
 * gated model (see the Rust voice docs); pass it with --jarvis-state. The Moonshine tiny-streaming
 * files are copied from an existing moonshine_voice cache or STARK_STT_MODEL_DIR.
 *
 * Node port of the former prepare-voice-models.py: same flags, same layout.
 */

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const HF = 'https://huggingface.co';
const TTS_REPO = 'kyutai/pocket-tts-without-voice-cloning';
const TTS_REV = 'a7056f505a4a8ca72d8b5fb1440071d6f1a35739';
const TTS_FILES = [
  'languages/english_18m_2026-10/model.safetensors',
  'languages/english_18m_2026-10/tokenizer.json',
  'languages/english_18m_2026-10/embeddings/alba.safetensors',
  'languages/english_18m_2026-10/embeddings/anna.safetensors',
];
const STT_FILES = [
  'encoder.ort',
  'decoder_kv.ort',
  'frontend.weights.ort',
  'cross_kv.ort',
  'adapter.ort',
  'frontend.model.ort',
  'streaming_config.json',
  'tokenizer.bin',
];

function parseArgs(argv) {
  const args = {
    output: path.join(ROOT, 'models'),
    jarvisState: null,
    skipTts: false,
    skipStt: false,
    sttModel: 'medium',
  };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--output') args.output = path.resolve(argv[++i]);
    else if (arg.startsWith('--output=')) args.output = path.resolve(arg.slice(9));
    else if (arg === '--jarvis-state') args.jarvisState = path.resolve(argv[++i]);
    else if (arg.startsWith('--jarvis-state=')) args.jarvisState = path.resolve(arg.slice(15));
    else if (arg === '--skip-tts') args.skipTts = true;
    else if (arg === '--skip-stt') args.skipStt = true;
    else if (arg === '--stt-model') args.sttModel = argv[++i];
    else if (arg.startsWith('--stt-model=')) args.sttModel = arg.slice(12);
    else if (arg === '--help' || arg === '-h') {
      console.log(`usage: prepare-voice-models.mjs [--output DIR] [--jarvis-state FILE] [--skip-tts] [--skip-stt] [--stt-model medium|small|tiny]`);
      process.exit(0);
    } else {
      console.error(`unknown argument: ${arg}`);
      process.exit(2);
    }
  }
  if (!['medium', 'small', 'tiny'].includes(args.sttModel)) {
    console.error(`invalid --stt-model (expected medium|small|tiny): ${args.sttModel}`);
    process.exit(2);
  }
  return args;
}

async function download(url, target) {
  fs.mkdirSync(path.dirname(target), { recursive: true });
  try {
    const stat = fs.statSync(target);
    if (stat.isFile() && stat.size > 0) {
      console.log(`keep ${target}`);
      return;
    }
  } catch { /* missing — download it */ }
  console.log(`download ${url}`);
  const response = await fetch(url);
  if (!response.ok || !response.body) {
    throw new Error(`download failed: ${response.status} ${url}`);
  }
  const tmp = `${target}.tmp`;
  const out = fs.createWriteStream(tmp);
  await response.body.pipeTo(
    new WritableStream({
      write(chunk) {
        return new Promise((resolve, reject) => {
          out.write(Buffer.from(chunk), (error) => (error ? reject(error) : resolve()));
        });
      },
      close() {
        return new Promise((resolve) => out.end(resolve));
      },
    }),
  );
  fs.renameSync(tmp, target);
}

async function downloadTts(output) {
  for (const rel of TTS_FILES) {
    const url = `${HF}/${TTS_REPO}/resolve/${TTS_REV}/${rel}`;
    let target;
    if (rel.includes('/embeddings/')) {
      target = path.join(output, 'tts', 'voices', path.basename(rel));
    } else {
      const rest = path.relative('languages/english_18m_2026-10', rel);
      target = path.join(output, 'tts', rest);
    }
    await download(url, target);
  }
}

function findStt(model) {
  const envDir = process.env.STARK_STT_MODEL_DIR;
  if (envDir) {
    const dir = path.resolve(envDir);
    if (fs.existsSync(dir) && fs.statSync(dir).isDirectory()) return dir;
  }
  const localAppData = process.env.LOCALAPPDATA || '';
  const cacheBase = path.join(localAppData, 'moonshine_voice', 'moonshine_voice', 'Cache', 'download.moonshine.ai', 'model');
  let entries = [];
  try {
    entries = fs.readdirSync(cacheBase)
      .filter((name) => name.startsWith(`${model}-streaming-en`))
      .map((name) => path.join(cacheBase, name))
      .sort()
      .reverse();
  } catch { /* no cache */ }
  for (const candidate of entries) {
    try {
      if (
        fs.statSync(candidate).isDirectory() &&
        STT_FILES.every((name) => fs.existsSync(path.join(candidate, name)))
      ) {
        return candidate;
      }
    } catch { /* skip */ }
  }
  console.error(
    `${model}-streaming model not found. Set STARK_STT_MODEL_DIR to the quantized directory.`,
  );
  process.exit(1);
}

function copyStt(output, model) {
  const source = findStt(model);
  const target = path.join(output, 'stt', `${model}-streaming-en`);
  fs.mkdirSync(target, { recursive: true });
  for (const name of STT_FILES) {
    fs.cpSync(path.join(source, name), path.join(target, name));
    console.log(`copy ${path.join(source, name)} -> ${path.join(target, name)}`);
  }
}

function copyJarvis(output, source) {
  if (!fs.existsSync(source) || !fs.statSync(source).isFile()) {
    console.error(`JARVIS voice state not found: ${source}`);
    process.exit(1);
  }
  const target = path.join(output, 'tts', 'voices', 'jarvis.safetensors');
  fs.mkdirSync(path.dirname(target), { recursive: true });
  fs.cpSync(source, target);
  console.log(`copy ${source} -> ${target}`);
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  fs.mkdirSync(args.output, { recursive: true });
  if (!args.skipTts) {
    await downloadTts(args.output);
  }
  const jarvis = args.jarvisState || process.env.STARK_JARVIS_STATE || null;
  if (jarvis) {
    copyJarvis(args.output, path.resolve(jarvis));
  } else if (!args.skipTts) {
    console.error('warning: no --jarvis-state; the default JARVIS voice will not be bundled');
  }
  if (!args.skipStt) {
    copyStt(args.output, args.sttModel);
  }
  console.log(`voice assets ready at ${args.output}`);
}

await main();
