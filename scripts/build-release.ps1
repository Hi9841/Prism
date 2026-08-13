[CmdletBinding()]
param(
  [string]$Target = "x86_64-pc-windows-msvc",
  [string]$ArtifactsDirectory = "artifacts"
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$tauriConfigPath = Join-Path $repoRoot "src-tauri\tauri.conf.json"
$tauri = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json
$version = [string]$tauri.version

if (-not [System.IO.Path]::IsPathRooted($ArtifactsDirectory)) {
  $ArtifactsDirectory = Join-Path $repoRoot $ArtifactsDirectory
}
$ArtifactsDirectory = [System.IO.Path]::GetFullPath($ArtifactsDirectory)
New-Item -ItemType Directory -Path $ArtifactsDirectory -Force | Out-Null

$env:TAURI_SIGNING_PRIVATE_KEY = Get-Content -LiteralPath "$env:USERPROFILE\.prism\signing\updater.key" -Raw
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD = (Get-Content -LiteralPath "$env:USERPROFILE\.prism\signing\updater.key.password" -Raw).Trim()

# Keep the target flag on the Tauri CLI. Passing an extra `--` forwards the
# flag only to Cargo and can leave Tauri bundling a stale generic target.
& bun run tauri build --target $Target
if ($LASTEXITCODE -ne 0) {
  throw "Tauri release build failed for target '$Target'."
}

$binaryPath = Join-Path $repoRoot "src-tauri\target\$Target\release\prism.exe"
$bundlePath = Join-Path $repoRoot "src-tauri\target\$Target\release\bundle\nsis\Prism_${version}_x64-setup.exe"
$signatureSourcePath = "$bundlePath.sig"
foreach ($requiredPath in @($binaryPath, $bundlePath, $signatureSourcePath)) {
  if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
    throw "Release build output is missing: $requiredPath"
  }
}

$binaryVersion = [string](Get-Item -LiteralPath $binaryPath).VersionInfo.ProductVersion
if ($binaryVersion -ne $version) {
  throw "Stale target binary detected: expected $version, found $binaryVersion at $binaryPath"
}

$installerPath = Join-Path $ArtifactsDirectory "Prism_${version}_x64-setup.exe"
$signaturePath = "$installerPath.sig"
Copy-Item -LiteralPath $bundlePath -Destination $installerPath -Force
Copy-Item -LiteralPath $signatureSourcePath -Destination $signaturePath -Force

Write-Host "Built Prism $version for $Target."
Write-Host "Installer: $installerPath"
Write-Host "Signature: $signaturePath"
