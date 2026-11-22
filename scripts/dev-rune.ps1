# Latest-master development loop for the Rune desktop.
#
# Starts or reuses a local stark-server on 127.0.0.1:8124, then runs the Rune
# UI against it. The repo harness root is used so installed-app data is never
# touched. Ctrl+C stops the UI; a server started by this script is stopped too.
param(
    [switch]$Pull,
    [switch]$Release,
    [switch]$NoServer,
    [string]$ModelsDir = "D:\Stark\resources\models"
)

$ErrorActionPreference = "Stop"
$root = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$port = 8124
$health = "http://127.0.0.1:$port/api/health"
$server = $null
$serverLog = Join-Path $env:TEMP "stark-dev-server.log"

function Test-Health {
    try {
        $response = Invoke-RestMethod -Uri $health -TimeoutSec 2
        return [bool]$response.ok
    } catch {
        return $false
    }
}

if ($Pull) {
    Write-Host "==> git pull --ff-only"
    git -C $root pull --ff-only
    if ($LASTEXITCODE -ne 0) { throw "git pull failed" }
}

$env:STARK_HARNESS_ROOT = $root
$env:STARK_PORT = "$port"
$env:STARK_API_PORT = "$port"
$env:STARK_APP_VERSION = "dev"
if (Test-Path $ModelsDir) {
    $env:STARK_MODELS_DIR = (Resolve-Path $ModelsDir).Path
}

Write-Host "==> harness: $root"
Write-Host "==> api:     http://127.0.0.1:$port"

if (-not $NoServer) {
    if (Test-Health) {
        Write-Host "==> reusing the backend already listening on $port"
    } else {
        Write-Host "==> starting stark-server (log: $serverLog)"
        $server = Start-Process -FilePath "cargo" `
            -ArgumentList @(
                "run",
                "--manifest-path", (Join-Path $root "backend-rs\Cargo.toml"),
                "-p", "stark-server"
            ) `
            -PassThru -WindowStyle Hidden `
            -RedirectStandardOutput $serverLog `
            -RedirectStandardError "$serverLog.err"

        $deadline = (Get-Date).AddSeconds(90)
        while (-not (Test-Health)) {
            if ($server.HasExited) {
                Get-Content $serverLog -ErrorAction SilentlyContinue | Select-Object -Last 20
                Get-Content "$serverLog.err" -ErrorAction SilentlyContinue | Select-Object -Last 20
                throw "stark-server exited before becoming healthy"
            }
            if ((Get-Date) -gt $deadline) {
                throw "stark-server did not become healthy within 90s"
            }
            Start-Sleep -Milliseconds 500
        }
        Write-Host "==> backend healthy"
    }
}

$code = 1
try {
    Write-Host "==> starting the Rune UI"
    $arguments = @(
        "run",
        "--manifest-path", (Join-Path $root "desktop-rune\Cargo.toml"),
        "-p", "stark-rune-app",
        "--features", "rune-real"
    )
    if ($Release) { $arguments += "--release" }
    & cargo @arguments
    $code = $LASTEXITCODE
} finally {
    if ($server -and -not $server.HasExited) {
        Write-Host "==> stopping the backend started by this script"
        Stop-Process -Id $server.Id -Force -ErrorAction SilentlyContinue
    }
}

exit $code
