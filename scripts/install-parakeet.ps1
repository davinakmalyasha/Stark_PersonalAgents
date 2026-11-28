# Install the exact Parakeet Unified English model Hex uses.
#
#   powershell -ExecutionPolicy Bypass -File scripts/install-parakeet.ps1
#
# The shared models directory is normally the `models` junction in the
# StarkRune install root. Pass -ModelsDir to override it.
param(
    [string]$ModelsDir = "$env:LOCALAPPDATA\Programs\StarkRune\models"
)

$ErrorActionPreference = "Stop"
$model = "parakeet-unified-en-0.6b-Q8_0.gguf"
$sha256 = "4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795"
$url = "https://huggingface.co/handy-computer/parakeet-unified-en-0.6b-gguf/resolve/7e948f21b7bdbac698d3318db9d350f1096f3b6c/$model"
$dir = Join-Path $ModelsDir "stt\parakeet-unified-en-0.6b"
$dest = Join-Path $dir $model

New-Item -ItemType Directory -Force -Path $dir | Out-Null
if (Test-Path -LiteralPath $dest) {
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $dest).Hash.ToLowerInvariant()
    if ($actual -eq $sha256) {
        Write-Host "Parakeet already installed: $dest"
        exit 0
    }
    Write-Host "Existing file does not match; downloading again."
}

Write-Host "Downloading $model (731 MB)..."
$temp = "$dest.partial"
Invoke-WebRequest -Uri $url -OutFile $temp
$actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash.ToLowerInvariant()
if ($actual -ne $sha256) {
    Remove-Item -LiteralPath $temp -Force
    throw "Parakeet model SHA-256 mismatch: $actual"
}
Move-Item -LiteralPath $temp -Destination $dest -Force
Write-Host "Installed $dest"
