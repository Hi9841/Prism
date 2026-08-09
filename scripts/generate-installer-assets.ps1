[CmdletBinding()]
param()

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

Add-Type -AssemblyName System.Drawing

$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
$sourcePath = Join-Path $repoRoot "src-tauri\icons\icon-source.png"
$outputDir = Join-Path $repoRoot "src-tauri\nsis"
$headerPath = Join-Path $outputDir "header.bmp"
$sidebarPath = Join-Path $outputDir "sidebar.bmp"

New-Item -ItemType Directory -Path $outputDir -Force | Out-Null
$icon = [System.Drawing.Image]::FromFile($sourcePath)

function New-PrismBitmap {
  param(
    [int]$Width,
    [int]$Height,
    [System.Drawing.Color]$Background,
    [int]$IconX,
    [int]$IconY,
    [int]$IconSize,
    [string]$OutputPath
  )

  $bitmap = [System.Drawing.Bitmap]::new(
    $Width,
    $Height,
    [System.Drawing.Imaging.PixelFormat]::Format24bppRgb
  )
  $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
  try {
    $graphics.Clear($Background)
    $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
    $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $graphics.DrawImage($icon, $IconX, $IconY, $IconSize, $IconSize)
    $bitmap.Save($OutputPath, [System.Drawing.Imaging.ImageFormat]::Bmp)
  } finally {
    $graphics.Dispose()
    $bitmap.Dispose()
  }
}

try {
  New-PrismBitmap `
    -Width 150 `
    -Height 57 `
    -Background ([System.Drawing.Color]::FromArgb(248, 248, 251)) `
    -IconX 100 `
    -IconY 7 `
    -IconSize 43 `
    -OutputPath $headerPath

  New-PrismBitmap `
    -Width 164 `
    -Height 314 `
    -Background ([System.Drawing.Color]::FromArgb(20, 20, 26)) `
    -IconX 24 `
    -IconY 86 `
    -IconSize 116 `
    -OutputPath $sidebarPath
} finally {
  $icon.Dispose()
}
