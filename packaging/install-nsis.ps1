[CmdletBinding()]
param(
  [string]$ToolsDirectory = (Join-Path $env:LOCALAPPDATA "tauri")
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Save-VerifiedDownload([string]$Url, [string]$Path, [string]$Sha256) {
  Invoke-WebRequest -Uri $Url -OutFile $Path -UseBasicParsing
  if ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -ne $Sha256) {
    throw "SHA-256 mismatch for $Url"
  }
}

$nsis = Join-Path $ToolsDirectory "NSIS"
if (Test-Path -LiteralPath $nsis) {
  throw "NSIS already exists at $nsis. Use a clean tools directory."
}

New-Item -ItemType Directory -Path $ToolsDirectory -Force | Out-Null
$archive = Join-Path $ToolsDirectory "nsis-3.13.zip"
Save-VerifiedDownload "https://github.com/tauri-apps/binary-releases/releases/download/nsis-3.13/nsis-3.13.zip" $archive "ba63dffc4410ee89193e1cb5a41989991bd77c61068da17e3156d136b7b0b3d8"
Expand-Archive -LiteralPath $archive -DestinationPath $ToolsDirectory
Rename-Item -LiteralPath (Join-Path $ToolsDirectory "nsis-3.13") -NewName "NSIS"
$plugins = Join-Path $nsis "Plugins/x86-unicode/additional"
New-Item -ItemType Directory -Path $plugins -Force | Out-Null
Save-VerifiedDownload "https://github.com/tauri-apps/nsis-tauri-utils/releases/download/nsis_tauri_utils-v0.5.3/nsis_tauri_utils.dll" (Join-Path $plugins "nsis_tauri_utils.dll") "5ba143b5db4a87d32d6e7802e033330aae56cbceabe0d1e3ba41948385ad4709"
