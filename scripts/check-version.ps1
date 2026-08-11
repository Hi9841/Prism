[CmdletBinding()]
param(
  [string]$ExpectedTag = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$package = Get-Content -LiteralPath (Join-Path $repoRoot "package.json") -Raw | ConvertFrom-Json
$tauri = Get-Content -LiteralPath (Join-Path $repoRoot "src-tauri\tauri.conf.json") -Raw | ConvertFrom-Json
$cargoToml = Get-Content -LiteralPath (Join-Path $repoRoot "src-tauri\Cargo.toml") -Raw
$cargoLock = Get-Content -LiteralPath (Join-Path $repoRoot "src-tauri\Cargo.lock") -Raw

$cargoVersion = [regex]::Match(
  $cargoToml,
  '(?ms)^\[package\].*?^version\s*=\s*"([^"]+)"'
).Groups[1].Value
$lockVersion = [regex]::Match(
  $cargoLock,
  '(?ms)^name\s*=\s*"prism"\s*\r?\nversion\s*=\s*"([^"]+)"'
).Groups[1].Value

$versions = @(
  [string]$package.version,
  [string]$tauri.version,
  $cargoVersion,
  $lockVersion
)
if ($versions | Where-Object { $_ -ne $versions[0] }) {
  throw "Version mismatch: package=$($versions[0]), tauri=$($versions[1]), cargo=$($versions[2]), lock=$($versions[3])"
}

if ($ExpectedTag -and $ExpectedTag -notin @($versions[0], "v$($versions[0])")) {
  throw "Release tag '$ExpectedTag' does not match application version '$($versions[0])'."
}

Write-Host "Prism version $($versions[0]) is consistent."
