# Fast local desktop loop: rebuild the NSIS bundle and launch the installer.
# No tag, no CI wait (~2-4 min incremental after the first build).
# Use for UI iteration; cut a v* tag only for shippable checkpoints.
#
#   pwsh scripts/rebuild-desktop.ps1
#   pwsh scripts/rebuild-desktop.ps1 -SkipBuild   # reinstall last local bundle
param([switch]$SkipBuild)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$fe = Join-Path $root 'frontend'

if (-not $SkipBuild) {
  Write-Host '==> Building desktop bundle (frontend + sidecar)...'
  Push-Location $fe
  try {
    npx tauri build --bundles nsis
    if ($LASTEXITCODE -ne 0) { throw "tauri build failed ($LASTEXITCODE)" }
  } finally {
    Pop-Location
  }
}

$bundle = Join-Path $fe 'src-tauri\target\release\bundle\nsis'
$exe = Get-ChildItem $bundle -Filter 'Stark_*_x64-setup.exe' -ErrorAction SilentlyContinue |
  Sort-Object LastWriteTime -Descending | Select-Object -First 1
if (-not $exe) { throw "No installer found in $bundle. Run without -SkipBuild first." }

Write-Host "==> Launching $($exe.Name) ($([math]::Round($exe.Length / 1MB, 1)) MB)"
Write-Host '    Close Stark first if the installer asks. SmartScreen: More info > Run anyway.'
Unblock-File -Path $exe.FullName -ErrorAction SilentlyContinue
Start-Process $exe.FullName
