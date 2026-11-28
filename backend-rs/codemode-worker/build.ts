import { mkdirSync } from "node:fs"
import { join } from "node:path"

const name = process.platform === "win32" ? "codemode-worker.exe" : "codemode-worker"
const outfile = join(import.meta.dir, "dist", name)
mkdirSync(join(import.meta.dir, "dist"), { recursive: true })

const result = Bun.spawnSync(
  ["bun", "build", "--compile", "./src/main.ts", "--outfile", outfile],
  { cwd: import.meta.dir, stdout: "inherit", stderr: "inherit" },
)
process.exit(result.exitCode ?? 1)
