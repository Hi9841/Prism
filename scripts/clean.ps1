[CmdletBinding(SupportsShouldProcess)]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$repoPrefix = $repoRoot.TrimEnd([System.IO.Path]::DirectorySeparatorChar) +
  [System.IO.Path]::DirectorySeparatorChar

$generatedTargets = @(
  ".tmp-issue-video",
  "artifacts\Prism_0.3.0_x64-setup.exe",
  "dist",
  "performance",
  "src-tauri\gen",
  "src-tauri\icons\128x128.png",
  "src-tauri\icons\128x128@2x.png",
  "src-tauri\icons\32x32.png",
  "src-tauri\icons\64x64.png",
  "src-tauri\icons\android",
  "src-tauri\icons\icon.icns",
  "src-tauri\icons\ios",
  "src-tauri\icons\Square107x107Logo.png",
  "src-tauri\icons\Square142x142Logo.png",
  "src-tauri\icons\Square150x150Logo.png",
  "src-tauri\icons\Square284x284Logo.png",
  "src-tauri\icons\Square30x30Logo.png",
  "src-tauri\icons\Square310x310Logo.png",
  "src-tauri\icons\Square44x44Logo.png",
  "src-tauri\icons\Square71x71Logo.png",
  "src-tauri\icons\Square89x89Logo.png",
  "src-tauri\icons\StoreLogo.png",
  "src-tauri\scan.log",
  "src-tauri\target"
)

foreach ($relativeTarget in $generatedTargets) {
  $target = [System.IO.Path]::GetFullPath((Join-Path $repoRoot $relativeTarget))
  if (-not $target.StartsWith($repoPrefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing to clean a path outside the repository: $target"
  }
  if ((Test-Path -LiteralPath $target) -and $PSCmdlet.ShouldProcess($target, "Remove generated content")) {
    Remove-Item -LiteralPath $target -Recurse -Force
  }
}
