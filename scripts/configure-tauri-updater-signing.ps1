[CmdletBinding()]
param(
  [string]$Repository = "zcb617/panes_zh",
  [switch]$CheckOnly,
  [switch]$Rotate
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$keyRelativePath = ".tauri-signing/panes-updater.key"
$publicKeyRelativePath = "$keyRelativePath.pub"
$passwordRelativePath = "$keyRelativePath.password"
$privateKeyPath = Join-Path $repoRoot $keyRelativePath
$publicKeyPath = Join-Path $repoRoot $publicKeyRelativePath
$privateKeyPasswordPath = Join-Path $repoRoot $passwordRelativePath
$tauriConfigPath = Join-Path $repoRoot "src-tauri/tauri.conf.json"

if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
  throw "GitHub CLI (gh) is required. Install it and authenticate before continuing."
}

if (-not (Get-Command pnpm -ErrorAction SilentlyContinue)) {
  throw "pnpm is required to verify the Tauri updater signing key."
}

function Test-TauriUpdaterSigningMaterial {
  param(
    [Parameter(Mandatory = $true)]
    [string]$KeyPath,
    [Parameter(Mandatory = $true)]
    [string]$Password
  )

  $validationFilePath = Join-Path (Split-Path -Parent $KeyPath) ".signing-validation"
  [System.IO.File]::WriteAllText($validationFilePath, "Tauri updater signing validation.`n", [System.Text.UTF8Encoding]::new($false))

  try {
    $null = & pnpm exec tauri signer sign --private-key-path $KeyPath --password $Password $validationFilePath
    if ($LASTEXITCODE -ne 0) {
      throw "The local Tauri updater signing key and password do not form a usable signing pair."
    }
  }
  finally {
    Remove-Item -LiteralPath $validationFilePath, "$validationFilePath.sig" -Force -ErrorAction SilentlyContinue
  }
}

if ($Rotate -and $CheckOnly) {
  throw "-Rotate cannot be used together with -CheckOnly."
}

if ($Rotate) {
  $archiveDir = Join-Path $repoRoot (".tauri-signing/retired/" + (Get-Date -Format "yyyyMMdd-HHmmss"))
  New-Item -ItemType Directory -Path $archiveDir -Force | Out-Null

  foreach ($existingPath in @($privateKeyPath, $publicKeyPath, $privateKeyPasswordPath)) {
    if (Test-Path -LiteralPath $existingPath) {
      Move-Item -LiteralPath $existingPath -Destination $archiveDir
    }
  }

  $passwordBytes = [byte[]]::new(32)
  [System.Security.Cryptography.RandomNumberGenerator]::Fill($passwordBytes)
  $signingPassword = [Convert]::ToBase64String($passwordBytes).TrimEnd("=").Replace("+", "-").Replace("/", "_")
  [System.IO.File]::WriteAllText($privateKeyPasswordPath, $signingPassword, [System.Text.UTF8Encoding]::new($false))

  $null = & pnpm exec tauri signer generate --ci --password $signingPassword --write-keys $privateKeyPath
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to generate a new Tauri updater signing key."
  }

  Test-TauriUpdaterSigningMaterial -KeyPath $privateKeyPath -Password $signingPassword

  $newPublicKeyText = [System.IO.File]::ReadAllText($publicKeyPath)
  $newEmbeddedPublicKey = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($newPublicKeyText))
  $tauriConfigText = [System.IO.File]::ReadAllText($tauriConfigPath)
  $updatedTauriConfigText = [System.Text.RegularExpressions.Regex]::Replace(
    $tauriConfigText,
    '("pubkey"\s*:\s*")[^"]+("\s*)',
    ('${1}' + $newEmbeddedPublicKey + '${2}'),
    1
  )
  if ($updatedTauriConfigText -eq $tauriConfigText) {
    throw "Unable to update the updater public key in src-tauri/tauri.conf.json."
  }
  [System.IO.File]::WriteAllText($tauriConfigPath, $updatedTauriConfigText, [System.Text.UTF8Encoding]::new($false))

  Write-Output "A new Tauri updater signing key was generated. Previous local recovery material was preserved in $archiveDir."
}

foreach ($requiredPath in @($privateKeyPath, $publicKeyPath, $privateKeyPasswordPath, $tauriConfigPath)) {
  if (-not (Test-Path -LiteralPath $requiredPath)) {
    throw "Required signing file is missing: $requiredPath"
  }
}

foreach ($keyPath in @($keyRelativePath, $publicKeyRelativePath, $passwordRelativePath)) {
  & git -C $repoRoot check-ignore -q -- $keyPath
  if ($LASTEXITCODE -ne 0) {
    throw "The local signing-key and password files must be ignored by Git before use."
  }
}

$publicKeyText = [System.IO.File]::ReadAllText($publicKeyPath)
$signingPassword = [System.IO.File]::ReadAllText($privateKeyPasswordPath).Trim()
if ([string]::IsNullOrWhiteSpace($signingPassword)) {
  throw "The local signing-key password file is empty: $privateKeyPasswordPath"
}
$expectedPublicKey = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($publicKeyText))
$tauriConfig = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json

if ($tauriConfig.plugins.updater.pubkey -ne $expectedPublicKey) {
  throw "The public key in src-tauri/tauri.conf.json does not match the local recovery key."
}

Test-TauriUpdaterSigningMaterial -KeyPath $privateKeyPath -Password $signingPassword

if ($CheckOnly) {
  Write-Output "Tauri updater signing configuration is consistent for $Repository."
  exit 0
}

[System.IO.File]::ReadAllText($privateKeyPath) |
  & gh secret set TAURI_SIGNING_PRIVATE_KEY --repo $Repository --app actions
if ($LASTEXITCODE -ne 0) {
  throw "Unable to configure TAURI_SIGNING_PRIVATE_KEY in $Repository."
}

$signingPassword |
  & gh secret set TAURI_SIGNING_PRIVATE_KEY_PASSWORD --repo $Repository --app actions
if ($LASTEXITCODE -ne 0) {
  throw "Unable to configure TAURI_SIGNING_PRIVATE_KEY_PASSWORD in $Repository."
}

$secretNames = & gh secret list --repo $Repository --app actions --json name --jq '.[].name'
if ($LASTEXITCODE -ne 0 -or $secretNames -notcontains "TAURI_SIGNING_PRIVATE_KEY" -or $secretNames -notcontains "TAURI_SIGNING_PRIVATE_KEY_PASSWORD") {
  throw "The signing secrets were written but could not be verified."
}

Write-Output "Tauri updater signing secrets are configured for $Repository."
