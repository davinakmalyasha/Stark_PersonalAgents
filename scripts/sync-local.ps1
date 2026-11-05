# Fast-forward the laptop checkout to master.
#
#   pwsh scripts/sync-local.ps1              # pull only
#   pwsh scripts/sync-local.ps1 -Build       # pull + build server and frontend
#
# The installed desktop app updates itself from signed releases; this is for
# the development checkout. It never merges or stashes: a dirty tree stops it.
param(
  [string] $Remote = "origin",
  [string] $Branch = "master",
  [switch] $Build
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
  if (git status --porcelain) {
    throw "working tree is dirty - commit or stash before syncing"
  }
  git pull --ff-only $Remote $Branch
  if ($LASTEXITCODE -ne 0) { throw "git pull failed" }

  if ($Build) {
    Push-Location (Join-Path $root "backend-rs")
    try {
      cargo build -p stark-server
      if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
    } finally { Pop-Location }

    Push-Location (Join-Path $root "frontend")
    try {
      if (Test-Path "package-lock.json") { npm ci } else { npm install }
      if ($LASTEXITCODE -ne 0) { throw "npm install failed" }
      npm run build
      if ($LASTEXITCODE -ne 0) { throw "frontend build failed" }
    } finally { Pop-Location }
  }
  Write-Host "==> synced to $(git rev-parse --short HEAD)"
} finally {
  Pop-Location
}
