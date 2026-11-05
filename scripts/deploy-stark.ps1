# Build artifact -> drain-safe deploy, from the laptop.
#
#   pwsh scripts/setup-deploy-key.ps1 -Install      # once per machine
#   pwsh scripts/deploy-stark.ps1 -VpsHost 103.67.79.106 -Port 20128
#
# Downloads the newest successful `build-linux` artifact from GitHub Actions,
# uploads it over key-only SSH, then runs the drain-safe deploy on the VPS.
# Rollback is automatic if the new binary fails its health check.

param(
  [string] $VpsHost = "103.67.79.106",
  [int] $Port = 20128,
  [string] $User = "root",
  [string] $Repo = "davinakmalyasha/Stark_PersonalAgents",
  [string] $KeyPath = "$env:USERPROFILE\.ssh\stark_deploy_ed25519",
  [switch] $Verify
)

$ErrorActionPreference = "Stop"

if (-not (Test-Path $KeyPath)) {
  throw "deploy key not found: $KeyPath - run: pwsh scripts/setup-deploy-key.ps1 -Install"
}

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

$temp = Join-Path $env:TEMP ("stark-deploy-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temp | Out-Null

try {
  Write-Host "==> finding latest build-linux artifact"
  $runId = gh run list --repo $Repo --workflow build-linux.yml --status success --limit 1 --json databaseId --jq '.[0].databaseId'
  if (-not $runId) { throw "no successful build-linux run found" }
  $revision = (gh run view $runId --repo $Repo --json headSha --jq .headSha).Trim()
  Write-Host "    run $runId ($revision)"

  gh run download $runId --repo $Repo -n stark-linux-x86_64 -D $temp
  $archive = Join-Path $temp "stark-linux-x86_64.tar.gz"
  if (-not (Test-Path $archive)) { throw "artifact missing: $archive" }

  Write-Host "==> upload to ${User}@${VpsHost}:$Port"
  & ssh @sshArgs "$User@$VpsHost" "rm -rf /root/stark-deploy && mkdir -p /root/stark-deploy"
  if ($LASTEXITCODE -ne 0) { throw "ssh failed" }

  & scp @scpArgs $archive "$User@${VpsHost}:/root/stark-deploy/stark-linux-x86_64.tar.gz"
  if ($LASTEXITCODE -ne 0) { throw "artifact upload failed" }
  foreach ($name in @("deploy-vps.sh", "verify-cloud.sh", "restore-drill.sh")) {
    & scp @scpArgs (Join-Path $PSScriptRoot $name) "$User@${VpsHost}:/root/stark-deploy/$name"
    if ($LASTEXITCODE -ne 0) { throw "upload failed: $name" }
  }

  Write-Host "==> deploy (drain-safe, auto-rollback)"
  & ssh @sshArgs "$User@$VpsHost" "bash /root/stark-deploy/deploy-vps.sh /root/stark-deploy/stark-linux-x86_64.tar.gz $revision"
  if ($LASTEXITCODE -ne 0) { throw "deploy failed; the VPS rolled back automatically" }

  if ($Verify) {
    Write-Host "==> verify (health, export, restore drill)"
    & ssh @sshArgs "$User@$VpsHost" "bash /root/stark-deploy/verify-cloud.sh"
    if ($LASTEXITCODE -ne 0) { throw "verification failed" }
  }

  & ssh @sshArgs "$User@$VpsHost" "rm -rf /root/stark-deploy"
  Write-Host "==> done"
} finally {
  Remove-Item -Recurse -Force $temp -ErrorAction SilentlyContinue
}
