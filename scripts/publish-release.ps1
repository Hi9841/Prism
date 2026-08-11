[CmdletBinding()]
param(
  [string]$Repository = "Hi9841/Prism",
  [string]$Tag = "",
  [string]$ArtifactsDirectory = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$tauriConfigPath = Join-Path $repoRoot "src-tauri\tauri.conf.json"
$tauri = Get-Content -LiteralPath $tauriConfigPath -Raw | ConvertFrom-Json
$version = [string]$tauri.version

if (-not $Tag) {
  $Tag = $version
}

& (Join-Path $PSScriptRoot "check-version.ps1") -ExpectedTag $Tag

if (-not $ArtifactsDirectory) {
  $ArtifactsDirectory = Join-Path $repoRoot "artifacts"
} elseif (-not [System.IO.Path]::IsPathRooted($ArtifactsDirectory)) {
  $ArtifactsDirectory = Join-Path $repoRoot $ArtifactsDirectory
}
$ArtifactsDirectory = [System.IO.Path]::GetFullPath($ArtifactsDirectory)

$installerName = "Prism_${version}_x64-setup.exe"
$signatureName = "$installerName.sig"
$notesName = "Prism_${version}_release-notes.md"
$installerPath = Join-Path $ArtifactsDirectory $installerName
$signaturePath = Join-Path $ArtifactsDirectory $signatureName
$notesPath = Join-Path $ArtifactsDirectory $notesName
$manifestPath = Join-Path $ArtifactsDirectory "latest.json"

foreach ($requiredPath in @($installerPath, $signaturePath, $notesPath)) {
  if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
    throw "Required release file is missing: $requiredPath"
  }
}

$signature = (Get-Content -LiteralPath $signaturePath -Raw).Trim()
if (-not $signature) {
  throw "Updater signature is empty: $signaturePath"
}
$releaseNotes = (Get-Content -LiteralPath $notesPath -Raw).Trim()
if (-not $releaseNotes) {
  throw "Release notes are empty: $notesPath"
}

& gh auth status --hostname github.com *> $null
if ($LASTEXITCODE -ne 0) {
  throw "GitHub CLI is not authenticated. Run 'gh auth login' first."
}

$releaseJson = & gh release view $Tag --repo $Repository --json publishedAt 2>$null
$releaseExists = $LASTEXITCODE -eq 0
$publishedAt = (Get-Date).ToUniversalTime().ToString("yyyy-MM-ddTHH:mm:ssZ")
if ($releaseExists) {
  $publishedAt = [string](($releaseJson | ConvertFrom-Json).publishedAt)
}

$downloadUrl = "https://github.com/$Repository/releases/download/$Tag/$installerName"
$platform = [ordered]@{
  signature = $signature
  url = $downloadUrl
}
$manifest = [ordered]@{
  version = $version
  notes = $releaseNotes
  pub_date = $publishedAt
  platforms = [ordered]@{
    "windows-x86_64-nsis" = $platform
    "windows-x86_64" = $platform
  }
}
$manifestJson = $manifest | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText(
  $manifestPath,
  "$manifestJson`n",
  [System.Text.UTF8Encoding]::new($false)
)

if ($releaseExists) {
  & gh release edit $Tag --repo $Repository --notes-file $notesPath --latest
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to update GitHub release '$Tag'."
  }
  & gh release upload $Tag $installerPath $signaturePath $manifestPath --repo $Repository --clobber
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to upload release assets for '$Tag'."
  }
} else {
  & gh release create $Tag $installerPath $signaturePath $manifestPath `
    --repo $Repository `
    --verify-tag `
    --title "Prism_$version" `
    --notes-file $notesPath `
    --latest
  if ($LASTEXITCODE -ne 0) {
    throw "Failed to create GitHub release '$Tag'. Push the tag before publishing."
  }
}

$assetJson = & gh release view $Tag --repo $Repository --json assets
if ($LASTEXITCODE -ne 0) {
  throw "Could not verify GitHub release '$Tag'."
}
$assetNames = @(($assetJson | ConvertFrom-Json).assets | ForEach-Object { $_.name })
foreach ($requiredAsset in @($installerName, $signatureName, "latest.json")) {
  if ($requiredAsset -notin $assetNames) {
    throw "GitHub release '$Tag' is missing required asset '$requiredAsset'."
  }
}

$latestManifestUrl = "https://github.com/$Repository/releases/latest/download/latest.json"
$publicManifest = $null
for ($attempt = 1; $attempt -le 6; $attempt++) {
  try {
    $cacheBuster = [DateTimeOffset]::UtcNow.ToUnixTimeMilliseconds()
    $publicManifest = Invoke-RestMethod -Uri "$latestManifestUrl`?t=$cacheBuster"
    if ([string]$publicManifest.version -eq $version) {
      break
    }
  } catch {
    if ($attempt -eq 6) {
      throw
    }
  }
  Start-Sleep -Seconds 2
}

if (-not $publicManifest -or [string]$publicManifest.version -ne $version) {
  throw "The public latest.json does not advertise Prism $version."
}
if (-not $publicManifest.platforms."windows-x86_64-nsis") {
  throw "The public latest.json is missing the Windows NSIS updater entry."
}

$localSize = (Get-Item -LiteralPath $installerPath).Length
$remoteInstaller = Invoke-WebRequest -Uri $downloadUrl -Method Head -UseBasicParsing
$remoteSize = [long]$remoteInstaller.Headers["Content-Length"]
if ($remoteSize -ne $localSize) {
  throw "Published installer size $remoteSize does not match local installer size $localSize."
}

Write-Host "Published Prism $version as '$Tag'."
Write-Host "Verified updater manifest: $latestManifestUrl"
Write-Host "Verified installer: $downloadUrl"
