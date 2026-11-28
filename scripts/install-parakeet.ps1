# Install the exact speech models Hex uses: Parakeet Unified English and the
# Silero VAD that keeps noise-only turns away from it.
#
#   powershell -ExecutionPolicy Bypass -File scripts/install-parakeet.ps1
#
# The shared models directory is normally the `models` junction in the
# StarkRune install root. Pass -ModelsDir to override it.
param(
    [string]$ModelsDir = "$env:LOCALAPPDATA\Programs\StarkRune\models",
    [switch]$SkipVad
)

$ErrorActionPreference = "Stop"

function Install-Model {
    param(
        [string]$Name,
        [string]$Sha256,
        [string]$Url,
        [string]$Dir,
        [string]$Label
    )
    $dest = Join-Path $Dir $Name
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    if (Test-Path -LiteralPath $dest) {
        $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $dest).Hash.ToLowerInvariant()
        if ($actual -eq $Sha256) {
            Write-Host "$Label already installed: $dest"
            return
        }
        Write-Host "Existing $Label does not match; downloading again."
    }

    Write-Host "Downloading $Name..."
    $temp = "$dest.partial"
    Invoke-WebRequest -Uri $Url -OutFile $temp
    $actual = (Get-FileHash -Algorithm SHA256 -LiteralPath $temp).Hash.ToLowerInvariant()
    if ($actual -ne $Sha256) {
        Remove-Item -LiteralPath $temp -Force
        throw "$Label SHA-256 mismatch: $actual"
    }
    Move-Item -LiteralPath $temp -Destination $dest -Force
    Write-Host "Installed $dest"
}

Install-Model `
    -Name "parakeet-unified-en-0.6b-Q8_0.gguf" `
    -Sha256 "4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795" `
    -Url "https://huggingface.co/handy-computer/parakeet-unified-en-0.6b-gguf/resolve/7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf" `
    -Dir (Join-Path $ModelsDir "stt\parakeet-unified-en-0.6b") `
    -Label "Parakeet Unified English"

if (-not $SkipVad) {
    Install-Model `
        -Name "silero-vad-v6.2-F32.gguf" `
        -Sha256 "0d871a800c420fdd5d1b17a3aea4296d1c39608f0ff4a9295eb76457eb1c88e6" `
        -Url "https://huggingface.co/handy-computer/silero-vad-v6.2-gguf/resolve/a9b2d61c20ae4ddf3644edb34969e8185a485448/silero-vad-v6.2-F32.gguf" `
        -Dir (Join-Path $ModelsDir "stt") `
        -Label "Silero VAD v6.2"
}
