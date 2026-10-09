# M2: does Chromium map prefers-reduced-motion to SPI_GETCLIENTAREAANIMATION?
# The setting changes in memory only (fWinIni = 0: no WM_SETTINGCHANGE broadcast,
# no registry write) and is restored in `finally`.
$ErrorActionPreference = 'Stop'
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Spi {
  [DllImport("user32.dll", SetLastError = true)]
  public static extern bool SystemParametersInfoW(uint a, uint b, ref int v, uint w);
  [DllImport("user32.dll", SetLastError = true)]
  public static extern bool SystemParametersInfoW(uint a, uint b, IntPtr v, uint w);
  public static int Get() { int v = 0; SystemParametersInfoW(0x1042, 0, ref v, 0); return v; }
  public static bool Set(bool on) { return SystemParametersInfoW(0x1043, 0, new IntPtr(on ? 1 : 0), 0); }
}
'@
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$chrome = 'C:\Program Files\Google\Chrome\Application\chrome.exe'
$page = 'file:///' + ((Join-Path $here 'baseline.html') -replace '\\', '/')

function Read-Chrome([string]$name) {
  $profile = Join-Path $here "hl-$name"
  $outFile = Join-Path $here "hl-$name.html"
  Start-Process -FilePath $chrome -Wait -NoNewWindow -RedirectStandardOutput $outFile -ArgumentList @(
    '--headless=new', '--disable-gpu', '--no-first-run', "--user-data-dir=`"$profile`"",
    '--virtual-time-budget=2000', '--dump-dom', $page)
  $dom = Get-Content -Raw $outFile
  if ($dom -notmatch 'RESULT (\{.*?\})</pre>') { throw "no RESULT in $outFile" }
  $line = $Matches[1] -replace '&quot;', '"'
  (ConvertFrom-Json $line).reduced_motion
}

$original = [Spi]::Get()
$results = @()
try {
  foreach ($on in @($false, $true)) {
    [void][Spi]::Set($on)
    $spi = [Spi]::Get()
    $reduce = Read-Chrome ("anim" + [int]$on)
    $results += [pscustomobject]@{ animation_effects = [bool]$spi; chromium_prefers_reduced_motion = $reduce }
  }
} finally {
  [void][Spi]::Set([bool]$original)
}
$results | ConvertTo-Json -Compress
"restored animation_effects=$([bool][Spi]::Get()) (original $([bool]$original))"
