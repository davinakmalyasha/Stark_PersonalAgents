# Build → ship → drain-safe deploy, from the laptop.
#
#   pwsh scripts/deploy-stark.ps1 -VpsHost 103.67.79.106 -Port 20128
#
# Downloads the newest successful `build-linux` artifact from GitHub Actions,
# uploads it to the VPS, then runs the drain-safe deploy there. Rollback is
# automatic if the new binary fails its health check.

param(
  [Parameter(Mandatory = $true)] [string] $VpsHost,
  [int] $Port = 20128,
  [string] $User = "root",
  [string] $Repo = "davinakmalyasha/Stark_PersonalAgents"
)

$ErrorActionPreference = "Stop"
$temp = Join-Path $env:TEMP ("stark-deploy-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $temp | Out-Null

Write-Host "==> finding latest build-linux artifact"
$runId = gh run list --repo $Repo --workflow build-linux.yml --status success --limit 1 --json databaseId --jq '.[0].databaseId'
if (-not $runId) { throw "no successful build-linux run found" }
Write-Host "    run $runId"

gh run download $runId --repo $Repo -n stark-linux-x86_64 -D $temp
$archive = Join-Path $temp "stark-linux-x86_64.tar.gz"
if (-not (Test-Path $archive)) { throw "artifact missing: $archive" }

Write-Host "==> upload to ${User}@${VpsHost}:$Port"
scp -P $Port $archive "${User}@${VpsHost}:/root/stark-linux-x86_64.tar.gz"
scp -P $Port (Join-Path $PSScriptRoot "deploy-vps.sh") "${User}@${VpsHost}:/opt/stark/deploy-vps.sh"

Write-Host "==> deploy (drain-safe, auto-rollback)"
ssh -p $Port "${User}@${VpsHost}" "bash /opt/stark/deploy-vps.sh /root/stark-linux-x86_64.tar.gz"

Remove-Item -Recurse -Force $temp
Write-Host "==> done"
