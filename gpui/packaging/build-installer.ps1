[CmdletBinding()]
param(
  # Defaults produce the public release: the real updater key in
  # ~/.prism/signing and the public update endpoint compiled into the app.
  [string]$ArtifactsDirectory = "artifacts",
  [string]$PrivateKeyPath = "$env:USERPROFILE\.prism\signing\updater.key",
  [string]$PasswordPath = "$env:USERPROFILE\.prism\signing\updater.key.password",
  # Test channels only: compile a different endpoint and public key into the app.
  [string]$UpdateEndpoint = "",
  [string]$UpdatePublicKey = "",
  # Test channels only: build a version other than the workspace version.
  [string]$Version = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# Windows PowerShell turns native stderr (cargo progress) into errors under
# "Stop"; native tools are checked through their exit codes instead.
function Invoke-Native([scriptblock]$Command, [string]$Failure) {
  $previous = $ErrorActionPreference
  $ErrorActionPreference = "Continue"
  try { & $Command 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $previous }
  if ($LASTEXITCODE -ne 0) { throw $Failure }
}

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$packaging = Join-Path $PSScriptRoot "nsis"
if (-not [System.IO.Path]::IsPathRooted($ArtifactsDirectory)) {
  $ArtifactsDirectory = Join-Path $repoRoot $ArtifactsDirectory
}
New-Item -ItemType Directory -Path $ArtifactsDirectory -Force | Out-Null

$workspaceVersion = (Select-String -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Pattern '^version = "([^"]+)"').Matches[0].Groups[1].Value
if (-not $Version) { $Version = $workspaceVersion }

# This shell may not write to %TEMP%: keep the build's temporary files in target.
$tmp = Join-Path $repoRoot "target\tmp"
New-Item -ItemType Directory -Path $tmp -Force | Out-Null
$env:TMP = $tmp
$env:TEMP = $tmp

# A test version builds in its own target directory so it never mixes with
# the release binary.
$targetDir = Join-Path $repoRoot "target"
if ($Version -ne $workspaceVersion -or $UpdateEndpoint -or $UpdatePublicKey) {
  $targetDir = Join-Path $repoRoot "target\channel-$Version"
}
$env:CARGO_TARGET_DIR = $targetDir
if ($Version -ne $workspaceVersion) { $env:PRISM_VERSION_OVERRIDE = $Version } else { Remove-Item Env:PRISM_VERSION_OVERRIDE -ErrorAction SilentlyContinue }
if ($UpdateEndpoint) { $env:PRISM_UPDATE_ENDPOINT = $UpdateEndpoint } else { Remove-Item Env:PRISM_UPDATE_ENDPOINT -ErrorAction SilentlyContinue }
if ($UpdatePublicKey) { $env:PRISM_UPDATE_PUBKEY = $UpdatePublicKey } else { Remove-Item Env:PRISM_UPDATE_PUBKEY -ErrorAction SilentlyContinue }

Push-Location $repoRoot
try {
  Invoke-Native { cargo build --release -p prism-app } "cargo build failed."
} finally {
  Pop-Location
  Remove-Item Env:CARGO_TARGET_DIR, Env:PRISM_VERSION_OVERRIDE, Env:PRISM_UPDATE_ENDPOINT, Env:PRISM_UPDATE_PUBKEY -ErrorAction SilentlyContinue
}

$binaryPath = Join-Path $targetDir "release\prism.exe"
$binaryVersion = [string](Get-Item -LiteralPath $binaryPath).VersionInfo.ProductVersion
if ($binaryVersion -ne $Version) {
  throw "Stale binary: expected $Version, found $binaryVersion at $binaryPath"
}

$nsisRoot = Join-Path $env:LOCALAPPDATA "tauri\NSIS"
$makensis = Join-Path $nsisRoot "makensis.exe"
if (-not (Test-Path -LiteralPath $makensis)) {
  throw "makensis.exe not found at $makensis (installed by the Tauri CLI on first bundle)."
}
$numeric = (($Version -split '[-+]')[0] -split '\.') + @('0', '0', '0')
$versionWithBuild = ($numeric[0..2] -join '.') + '.0'
$installerName = "Prism_${Version}_x64-setup.exe"
$installerPath = Join-Path $ArtifactsDirectory $installerName

Invoke-Native { & $makensis -V2 -INPUTCHARSET UTF8 `
  "-DVERSION=$Version" `
  "-DVERSIONWITHBUILD=$versionWithBuild" `
  "-DMAINBINARYSRCPATH=$binaryPath" `
  "-DESTIMATEDSIZE=$([math]::Ceiling((Get-Item -LiteralPath $binaryPath).Length / 1KB))" `
  "-DOUTFILE=$installerPath" `
  "-DPACKAGING=$packaging" `
  "-DADDITIONALPLUGINSPATH=$nsisRoot\Plugins\x86-unicode\additional" `
  (Join-Path $packaging "installer.nsi") } "makensis failed."

# Same signer as the Tauri release (tauri signer sign, minisign format).
$tauri = Join-Path $repoRoot "..\node_modules\.bin\tauri.exe"
if (-not (Test-Path -LiteralPath $tauri)) { throw "Tauri CLI not found at $tauri (run 'bun install' in the repo root)." }
$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -LiteralPath $PrivateKeyPath -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = if (Test-Path -LiteralPath $PasswordPath) { (Get-Content -LiteralPath $PasswordPath -Raw).Trim() } else { "" }
try {
  Invoke-Native { & $tauri signer sign $installerPath } "Signing failed."
} finally {
  Remove-Item Env:TAURI_SIGNING_PRIVATE_KEY, Env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD -ErrorAction SilentlyContinue
}
if (-not (Test-Path -LiteralPath "$installerPath.sig")) { throw "Signature missing: $installerPath.sig" }

Write-Host "Built Prism $Version (GPUI)."
Write-Host "Installer: $installerPath"
Write-Host "Signature: $installerPath.sig"
