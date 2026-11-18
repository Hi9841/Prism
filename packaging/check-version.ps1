[CmdletBinding()]
param(
  [string]$ExpectedTag = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# The workspace version (Cargo.toml) is the release version of every crate;
# Cargo.lock records it for prism-app and prism-core.
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$workspaceToml = Get-Content -LiteralPath (Join-Path $repoRoot "Cargo.toml") -Raw
$cargoLock = Get-Content -LiteralPath (Join-Path $repoRoot "Cargo.lock") -Raw

$workspaceVersion = [regex]::Match(
  $workspaceToml,
  '(?ms)^\[workspace\.package\].*?^version\s*=\s*"([^"]+)"'
).Groups[1].Value
$lockApp = [regex]::Match(
  $cargoLock,
  '(?ms)^name\s*=\s*"prism-app"\s*\r?\nversion\s*=\s*"([^"]+)"'
).Groups[1].Value
$lockCore = [regex]::Match(
  $cargoLock,
  '(?ms)^name\s*=\s*"prism-core"\s*\r?\nversion\s*=\s*"([^"]+)"'
).Groups[1].Value

$versions = @($workspaceVersion, $lockApp, $lockCore)
if (-not $workspaceVersion -or ($versions | Where-Object { $_ -ne $workspaceVersion })) {
  throw "Version mismatch: workspace=$workspaceVersion, lock prism-app=$lockApp, lock prism-core=$lockCore"
}

if ($ExpectedTag -and $ExpectedTag -notin @($workspaceVersion, "v$workspaceVersion")) {
  throw "Release tag '$ExpectedTag' does not match application version '$workspaceVersion'."
}

Write-Host "Prism version $workspaceVersion is consistent."
