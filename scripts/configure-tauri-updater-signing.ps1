[CmdletBinding()]
param(
  [string]$Repository = "zcb617/panes_zh",
  [switch]$CheckOnly
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$keyRelativePath = ".tauri-signing/panes-updater.key"
$publicKeyRelativePath = "$keyRelativePath.pub"
$privateKeyPath = Join-Path $repoRoot $keyRelativePath
$publicKeyPath = Join-Path $repoRoot $publicKeyRelativePath
$tauriConfigPath = Join-Path $repoRoot "src-tauri/tauri.conf.json"

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
  throw "GitHub CLI (gh) is required. Install it and authenticate before continuing."
}

foreach ($requiredPath in @($privateKeyPath, $publicKeyPath, $tauriConfigPath)) {
  if (-not (Test-Path -LiteralPath $requiredPath)) {
    throw "Required signing file is missing: $requiredPath"
  }
}

foreach ($keyPath in @($keyRelativePath, $publicKeyRelativePath)) {
  & git -C $repoRoot check-ignore -q -- $keyPath
  if ($LASTEXITCODE -ne 0) {
    throw "The local signing-key files must be ignored by Git before use."
  }
}

$publicKeyText = [System.IO.File]::ReadAllText($publicKeyPath)
$expectedPublicKey = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($publicKeyText))
$tauriConfig = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json

if ($tauriConfig.plugins.updater.pubkey -ne $expectedPublicKey) {
  throw "The public key in src-tauri/tauri.conf.json does not match the local recovery key."
}

if ($CheckOnly) {
  Write-Output "Tauri updater signing configuration is consistent for $Repository."
  exit 0
}

[System.IO.File]::ReadAllText($privateKeyPath) |
  & gh secret set TAURI_SIGNING_PRIVATE_KEY --repo $Repository --app actions
if ($LASTEXITCODE -ne 0) {
  throw "Unable to configure TAURI_SIGNING_PRIVATE_KEY in $Repository."
}

$secretNames = & gh secret list --repo $Repository --app actions --json name --jq '.[].name'
if ($LASTEXITCODE -ne 0 -or $secretNames -notcontains "TAURI_SIGNING_PRIVATE_KEY") {
  throw "The signing secret was written but could not be verified."
}

Write-Output "Tauri updater signing secret is configured for $Repository."
