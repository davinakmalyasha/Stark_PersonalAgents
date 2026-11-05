# Creates the Stark deploy keypairs on this machine.
#
#   powershell -File scripts/setup-deploy-key.ps1            # create, print keys
#   powershell -File scripts/setup-deploy-key.ps1 -Install    # create + install
#
# Two keys, one per actor: the laptop key for manual deploys, the actions key
# for GitHub Actions. Private halves never enter git; the actions private key
# is stored as the VPS_SSH_KEY repository secret.

param(
  [string] $KeyPath = "$env:USERPROFILE\.ssh\stark_deploy_ed25519",
  [string] $ActionsKeyPath = "$env:USERPROFILE\.ssh\stark_actions_ed25519",
  [switch] $Install,
  [string] $VpsHost = "103.67.79.106",
  [int] $Port = 20128,
  [string] $User = "root"
)

$ErrorActionPreference = "Stop"

function New-DeployKey([string] $path, [string] $comment) {
  if (Test-Path $path) {
    Write-Host "==> key already exists: $path"
    return
  }
  $dir = Split-Path -Parent $path
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  # -N '""' is how PowerShell passes an empty passphrase to a native exe.
  & ssh-keygen -t ed25519 -C $comment -f $path -N '""'
  if ($LASTEXITCODE -ne 0) { throw "ssh-keygen failed for $path" }
  Write-Host "==> created $path"
}

New-DeployKey $KeyPath "stark-deploy-laptop"
New-DeployKey $ActionsKeyPath "stark-deploy-actions"

Write-Host ""
Write-Host "Laptop public key:"
Write-Host ((Get-Content "$KeyPath.pub" -Raw).TrimEnd())
Write-Host ""
Write-Host "Actions public key:"
Write-Host ((Get-Content "$ActionsKeyPath.pub" -Raw).TrimEnd())
Write-Host ""

if (-not $Install) {
  Write-Host "Install both once (asks for the VPS password):"
  Write-Host "  powershell -ExecutionPolicy Bypass -File scripts/setup-deploy-key.ps1 -Install"
  exit 0
}

# First contact is still password-authenticated. The key line is piped to the
# remote command while ssh reads the password from the console.
Get-Content "$KeyPath.pub" -Raw | & ssh -p $Port "$User@$VpsHost" `
  "mkdir -p ~/.ssh && chmod 700 ~/.ssh && cat >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys && echo KEY_INSTALLED"
if ($LASTEXITCODE -ne 0) { throw "key install failed" }

$sshArgs = @(
  "-i", $KeyPath,
  "-o", "IdentitiesOnly=yes",
  "-o", "StrictHostKeyChecking=accept-new",
  "-o", "BatchMode=yes",
  "-p", "$Port"
)
$scpArgs = @(
  "-i", $KeyPath,
  "-o", "IdentitiesOnly=yes",
  "-o", "StrictHostKeyChecking=accept-new",
  "-o", "BatchMode=yes",
  "-P", "$Port"
)

# Upload both public halves, the VPS setup script, and the release scripts.
$uploads = @(
  "$KeyPath.pub",
  "$ActionsKeyPath.pub",
  (Join-Path $PSScriptRoot "setup-vps-ssh.sh"),
  (Join-Path $PSScriptRoot "deploy-vps.sh"),
  (Join-Path $PSScriptRoot "verify-cloud.sh"),
  (Join-Path $PSScriptRoot "restore-drill.sh")
)
foreach ($file in $uploads) {
  $name = Split-Path -Leaf $file
  & scp @scpArgs $file "$User@${VpsHost}:/root/$name"
  if ($LASTEXITCODE -ne 0) { throw "upload failed: $name" }
}
& ssh @sshArgs "$User@$VpsHost" `
  "bash /root/setup-vps-ssh.sh /root/stark_deploy_ed25519.pub /root/stark_actions_ed25519.pub"
if ($LASTEXITCODE -ne 0) { throw "VPS setup failed" }

& ssh @sshArgs "$User@$VpsHost" "echo KEY_LOGIN_OK"
if ($LASTEXITCODE -ne 0) { throw "key login failed" }

Write-Host ""
Write-Host "==> key auth works."
Write-Host "Test in a second terminal, then disable passwords:"
Write-Host "  ssh -i $KeyPath -p $Port $User@$VpsHost 'echo OK'"
Write-Host "  ssh -i $KeyPath -p $Port $User@$VpsHost 'bash /root/setup-vps-ssh.sh --harden'"
Write-Host ""
Write-Host "Store the Actions key as a repository secret:"
Write-Host "  powershell -ExecutionPolicy Bypass -File scripts/setup-actions-secret.ps1"
