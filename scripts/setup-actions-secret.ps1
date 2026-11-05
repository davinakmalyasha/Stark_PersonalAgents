# Publishes the Actions deploy key + pinned host key as repository secrets.
#
#   powershell -ExecutionPolicy Bypass -File scripts/setup-actions-secret.ps1
#
# Requires: gh auth login, and the public half of the actions key already
# installed on the VPS by scripts/setup-deploy-key.ps1 -Install.

param(
  [string] $ActionsKeyPath = "$env:USERPROFILE\.ssh\stark_actions_ed25519",
  [string] $Repo = "davinakmalyasha/Stark_PersonalAgents",
  [string] $VpsHost = "103.67.79.106",
  [int] $Port = 20128
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $ActionsKeyPath)) {
  throw "actions key not found: $ActionsKeyPath - run setup-deploy-key.ps1 first"
}

$private = Get-Content $ActionsKeyPath -Raw
gh secret set VPS_SSH_KEY --repo $Repo --body $private
if ($LASTEXITCODE -ne 0) { throw "could not set VPS_SSH_KEY" }

# Pin the host key now instead of trusting first contact inside CI.
$scan = & ssh-keyscan -p $Port $VpsHost 2>$null | Where-Object { $_ -and -not $_.StartsWith("#") }
if ($LASTEXITCODE -ne 0 -or -not $scan) {
  Write-Host "!! ssh-keyscan failed; the workflow will pin on first contact"
} else {
  gh secret set VPS_KNOWN_HOSTS --repo $Repo --body (($scan | Out-String).Trim())
  if ($LASTEXITCODE -ne 0) { throw "could not set VPS_KNOWN_HOSTS" }
  Write-Host "==> VPS_KNOWN_HOSTS pinned for ${VpsHost}:$Port"
}

Write-Host "==> VPS_SSH_KEY stored for $Repo"
Write-Host "    check: gh secret list --repo $Repo"
