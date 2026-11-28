# Install Hex's Silero VAD v6.2 GGUF (1.2 MB) for the speech gate.
#
#   powershell -ExecutionPolicy Bypass -File scripts/install-silero-vad.ps1
#
# Parakeet uses `%LOCALAPPDATA%\Programs\StarkRune\models` by default; pass
# -ModelsDir to override it (the portable install uses D:\Stark\resources\models).
param(
    [string]$ModelsDir = "$env:LOCALAPPDATA\Programs\StarkRune\models"
)

$ErrorActionPreference = "Stop"
$model = "silero-vad-v6.2-F32.gguf"
$sha256 = "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6"
$url = "https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/a9b2d61c20ae4ddf3644edb34969e8185a485448/$model"
$dir = Join-Path $ModelsDir "stt"
$dest = Join-Path $dir $model

New-Item -ItemType Directory -Force -Path $dir | Out-Null
if (Test-Path -LiteralPath $dest) {
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $dest).Hash.ToLowerInvariant()
    if ($actual -eq $sha256) {
        Write-Host "Silero VAD already installed: $dest"
        exit 0
    }
    Write-Host "Existing file does not match; downloading again."
}

Write-Host "Downloading $model (1.2 MB)..."
$temp = "$dest.partial"
Invoke-WebRequest -Uri $url -OutFile $temp
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash.ToLowerInvariant()
if ($actual -ne $sha256) {
    Remove-Item -LiteralPath $temp -Force
    throw "Silero VAD SHA-256 mismatch: $actual"
}
Move-Item -LiteralPath $temp -Destination $dest -Force
Write-Host "Installed $dest"
