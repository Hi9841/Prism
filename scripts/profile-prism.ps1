param(
  [Parameter(Mandatory = $true)]
  [string]$ExePath,

  [Parameter(Mandatory = $true)]
  [string]$Label,

  [string]$OutputDir = "performance",
  [switch]$EnableNativeLog
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

Add-Type -TypeDefinition @"
using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Runtime.InteropServices;

public static class PrismProfileNative {
  private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

  [DllImport("user32.dll")]
  public static extern IntPtr GetForegroundWindow();

  [DllImport("user32.dll")]
  public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

  [DllImport("user32.dll")]
  private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

  [DllImport("user32.dll")]
  private static extern bool IsWindowVisible(IntPtr hWnd);

  public static long[] GetVisibleWindows(string[] processNames) {
    var names = new HashSet<string>(processNames, StringComparer.OrdinalIgnoreCase);
    var handles = new List<long>();
    EnumWindows((handle, _) => {
      if (!IsWindowVisible(handle)) return true;
      uint processId;
      GetWindowThreadProcessId(handle, out processId);
      try {
        if (names.Contains(Process.GetProcessById((int)processId).ProcessName)) {
          handles.Add(handle.ToInt64());
        }
      } catch { }
      return true;
    }, IntPtr.Zero);
    return handles.ToArray();
  }
}
"@

function Get-ForegroundProcess {
  $handle = [PrismProfileNative]::GetForegroundWindow()
  $processId = [uint32]0
  [void][PrismProfileNative]::GetWindowThreadProcessId($handle, [ref]$processId)
  $process = Get-Process -Id $processId -ErrorAction SilentlyContinue
  [pscustomobject]@{
    Handle = $handle
    Id = $processId
    Name = if ($process) { $process.ProcessName } else { "" }
  }
}

function Wait-ForegroundProcess {
  param(
    [string[]]$Names,
    [int]$TimeoutMs = 5000
  )

  $timer = [System.Diagnostics.Stopwatch]::StartNew()
  do {
    $foreground = Get-ForegroundProcess
    if ($Names -contains $foreground.Name) {
      $timer.Stop()
      return [pscustomobject]@{
        ElapsedMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 2)
        Process = $foreground.Name
        ProcessId = $foreground.Id
        WindowHandle = $foreground.Handle
        TimedOut = $false
      }
    }
    Start-Sleep -Milliseconds 5
  } while ($timer.ElapsedMilliseconds -lt $TimeoutMs)

  $timer.Stop()
  $foreground = Get-ForegroundProcess
  [pscustomobject]@{
    ElapsedMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 2)
    Process = $foreground.Name
    ProcessId = $foreground.Id
    WindowHandle = $foreground.Handle
    TimedOut = $true
  }
}

function Wait-NewWindow {
  param(
    [string[]]$Names,
    [long[]]$ExistingHandles,
    [int]$TimeoutMs = 5000
  )

  $existing = [System.Collections.Generic.HashSet[long]]::new()
  foreach ($handle in $ExistingHandles) { [void]$existing.Add($handle) }
  $timer = [System.Diagnostics.Stopwatch]::StartNew()
  do {
    $foreground = Get-ForegroundProcess
    if ($Names -contains $foreground.Name) {
      $timer.Stop()
      return [pscustomobject]@{
        ElapsedMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 2)
        WindowHandle = $foreground.Handle.ToInt64()
        TimedOut = $false
      }
    }
    foreach ($handle in [PrismProfileNative]::GetVisibleWindows($Names)) {
      if (-not $existing.Contains($handle)) {
        $timer.Stop()
        return [pscustomobject]@{
          ElapsedMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 2)
          WindowHandle = $handle
          TimedOut = $false
        }
      }
    }
    Start-Sleep -Milliseconds 5
  } while ($timer.ElapsedMilliseconds -lt $TimeoutMs)

  $timer.Stop()
  [pscustomobject]@{
    ElapsedMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 2)
    WindowHandle = 0
    TimedOut = $true
  }
}

function Get-ProcessTreeIds {
  param([int]$RootId)

  $processes = Get-CimInstance Win32_Process -Property ProcessId, ParentProcessId, Name
  $runtimeNames = [System.Collections.Generic.HashSet[string]]::new(
    [string[]]@("prism.exe", "msedgewebview2.exe"),
    [System.StringComparer]::OrdinalIgnoreCase
  )
  $ids = [System.Collections.Generic.HashSet[int]]::new()
  [void]$ids.Add($RootId)
  $changed = $true
  while ($changed) {
    $changed = $false
    foreach ($process in $processes) {
      if (
        $ids.Contains([int]$process.ParentProcessId) -and
        $runtimeNames.Contains([string]$process.Name) -and
        -not $ids.Contains([int]$process.ProcessId)
      ) {
        [void]$ids.Add([int]$process.ProcessId)
        $changed = $true
      }
    }
  }
  return @($ids)
}

function Add-ResourceSamples {
  param(
    [int]$RootId,
    [string]$Phase,
    [int]$DurationMs,
    [System.Collections.Generic.List[object]]$Destination
  )

  $ids = Get-ProcessTreeIds -RootId $RootId
  $timer = [System.Diagnostics.Stopwatch]::StartNew()
  do {
    $members = @($ids | ForEach-Object { Get-Process -Id $_ -ErrorAction SilentlyContinue })
    $working = ($members | Measure-Object WorkingSet64 -Sum).Sum
    $private = ($members | Measure-Object PrivateMemorySize64 -Sum).Sum
    $cpu = ($members | Measure-Object CPU -Sum).Sum
    $handles = ($members | Measure-Object HandleCount -Sum).Sum
    $threads = ($members | ForEach-Object { $_.Threads.Count } | Measure-Object -Sum).Sum
    $Destination.Add([pscustomobject]@{
      Label = $Label
      Phase = $Phase
      ElapsedMs = $timer.ElapsedMilliseconds
      ProcessCount = $members.Count
      WorkingSetMB = [math]::Round($working / 1MB, 2)
      PrivateMB = [math]::Round($private / 1MB, 2)
      CpuSeconds = [math]::Round($cpu, 4)
      Handles = $handles
      Threads = $threads
    })
    Start-Sleep -Milliseconds 100
  } while ($timer.ElapsedMilliseconds -lt $DurationMs)
}

function Show-Prism {
  param($Shell)

  $Shell.SendKeys("^% ")
  return Wait-ForegroundProcess -Names @("prism")
}

function Invoke-PrismTarget {
  param(
    $Shell,
    [string]$Query,
    [string[]]$TargetProcesses
  )

  $reveal = Show-Prism -Shell $Shell
  # Foreground ownership changes before React remounts and focuses the input.
  Start-Sleep -Milliseconds 500
  $Shell.SendKeys("^a")
  $Shell.SendKeys("{BACKSPACE}")
  Start-Sleep -Milliseconds 100
  $Shell.SendKeys($Query)
  Start-Sleep -Milliseconds 300
  $existingWindows = [PrismProfileNative]::GetVisibleWindows($TargetProcesses)
  $timer = [System.Diagnostics.Stopwatch]::StartNew()
  $Shell.SendKeys("{ENTER}")
  $newWindow = Wait-NewWindow -Names $TargetProcesses -ExistingHandles $existingWindows
  $timer.Stop()
  $foreground = Get-ForegroundProcess
  [pscustomobject]@{
    Label = $Label
    Target = $Query
    RevealMs = $reveal.ElapsedMs
    LaunchToForegroundMs = [math]::Round($timer.Elapsed.TotalMilliseconds, 2)
    ForegroundProcess = $foreground.Name
    ForegroundProcessId = $foreground.Id
    NewWindowHandle = $newWindow.WindowHandle
    TimedOut = $newWindow.TimedOut
  }
}

$resolvedExe = (Resolve-Path -LiteralPath $ExePath).Path
$resolvedOutput = [System.IO.Path]::GetFullPath((Join-Path (Get-Location) $OutputDir))
[System.IO.Directory]::CreateDirectory($resolvedOutput) | Out-Null

$nativeLog = Join-Path $resolvedOutput "$Label-native.jsonl"
if ($EnableNativeLog -and (Test-Path -LiteralPath $nativeLog)) {
  Remove-Item -LiteralPath $nativeLog
}
$previousPerfLog = $env:PRISM_PERF_LOG
try {
  if ($EnableNativeLog) {
    $env:PRISM_PERF_LOG = $nativeLog
  } else {
    Remove-Item Env:PRISM_PERF_LOG -ErrorAction SilentlyContinue
  }
  $prism = Start-Process -FilePath $resolvedExe -WindowStyle Hidden -PassThru
} finally {
  if ($null -eq $previousPerfLog) {
    Remove-Item Env:PRISM_PERF_LOG -ErrorAction SilentlyContinue
  } else {
    $env:PRISM_PERF_LOG = $previousPerfLog
  }
}

$resources = [System.Collections.Generic.List[object]]::new()
$timings = [System.Collections.Generic.List[object]]::new()
$shell = New-Object -ComObject WScript.Shell

try {
  Add-ResourceSamples -RootId $prism.Id -Phase "startup_hidden" -DurationMs 2500 -Destination $resources

  $reveal = Show-Prism -Shell $shell
  $timings.Add([pscustomobject]@{
    Label = $Label
    Target = "palette"
    RevealMs = $reveal.ElapsedMs
    LaunchToForegroundMs = 0
    ForegroundProcess = $reveal.Process
    ForegroundProcessId = $reveal.ProcessId
    NewWindowHandle = 0
    TimedOut = $reveal.TimedOut
  })
  Add-ResourceSamples -RootId $prism.Id -Phase "visible_idle" -DurationMs 1500 -Destination $resources
  $shell.SendKeys("{ESC}")
  Start-Sleep -Milliseconds 300

  $timings.Add((Invoke-PrismTarget -Shell $shell -Query "C:\Users\hi\Documents\Marvel" -TargetProcesses @("explorer")))
  Add-ResourceSamples -RootId $prism.Id -Phase "after_explorer" -DurationMs 1200 -Destination $resources

  $timings.Add((Invoke-PrismTarget -Shell $shell -Query "wez" -TargetProcesses @("wezterm-gui", "wezterm")))
  Add-ResourceSamples -RootId $prism.Id -Phase "after_wezterm" -DurationMs 1200 -Destination $resources

  $shell.SendKeys("{ESC}")
  Start-Sleep -Milliseconds 300
  Add-ResourceSamples -RootId $prism.Id -Phase "final_hidden" -DurationMs 1500 -Destination $resources
} finally {
  $resources | Export-Csv -NoTypeInformation -LiteralPath (Join-Path $resolvedOutput "$Label-resources.csv")
  $timings | Export-Csv -NoTypeInformation -LiteralPath (Join-Path $resolvedOutput "$Label-timings.csv")
  if (-not $prism.HasExited) {
    Stop-Process -Id $prism.Id
  }
}

$summary = $resources |
  Group-Object Phase |
  ForEach-Object {
    $working = $_.Group | Measure-Object WorkingSetMB -Average -Maximum
    $private = $_.Group | Measure-Object PrivateMB -Average -Maximum
    [pscustomobject]@{
      Phase = $_.Name
      WorkingAverageMB = [math]::Round($working.Average, 2)
      WorkingPeakMB = [math]::Round($working.Maximum, 2)
      PrivateAverageMB = [math]::Round($private.Average, 2)
      PrivatePeakMB = [math]::Round($private.Maximum, 2)
    }
  }

$summary | Format-Table -AutoSize
$timings | Format-Table -AutoSize
