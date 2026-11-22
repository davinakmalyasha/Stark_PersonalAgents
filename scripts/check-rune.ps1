# Format, lint and test the two Stark workspaces the same way CI does.
param(
    [switch]$SkipBackend,
    [switch]$SkipWasm
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$desktop = Join-Path $root "desktop-rune"
$backend = Join-Path $root "backend-rs"

function Invoke-Step([string]$label, [string[]]$arguments, [string]$workingDirectory) {
    Write-Host ""
    Write-Host "==> $label"
    Push-Location $workingDirectory
    try {
        & cargo @arguments
        if ($LASTEXITCODE -ne 0) {
            throw "$label failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
    }
}

Invoke-Step "rune fmt" @(
    "fmt", "-p", "stark-theme", "-p", "stark-kit", "-p", "stark-rune-app",
    "-p", "stark-rune-web", "-p", "stark-launcher", "--", "--check"
) $desktop

Invoke-Step "rune clippy (app)" @(
    "clippy", "-p", "stark-rune-app", "--features", "rune-real",
    "--all-targets", "--", "-D", "warnings"
) $desktop

Invoke-Step "rune clippy (launcher)" @(
    "clippy", "-p", "stark-launcher", "--all-targets", "--", "-D", "warnings"
) $desktop

Invoke-Step "rune tests (app)" @(
    "test", "-p", "stark-rune-app", "--features", "rune-real"
) $desktop

Invoke-Step "rune tests (launcher)" @("test", "-p", "stark-launcher") $desktop

if (-not $SkipWasm) {
    Invoke-Step "rune wasm check" @(
        "check", "-p", "stark-rune-web", "--target", "wasm32-unknown-unknown"
    ) $desktop
}

if (-not $SkipBackend) {
    Invoke-Step "backend fmt" @("fmt", "--all", "--check") $backend
    Invoke-Step "backend clippy" @(
        "clippy", "--workspace", "--all-targets", "--", "-D", "warnings"
    ) $backend
    Invoke-Step "backend tests" @("test", "--workspace") $backend
}

Write-Host ""
Write-Host "All checks passed."
