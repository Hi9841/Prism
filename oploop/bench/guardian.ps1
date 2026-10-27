# Restores the owner's setup when a bench process ends, also when it is killed.
# apps.py starts this script through WMI (detached), one per bench run:
#   powershell -File guardian.ps1 -BenchPid <pid> -Launcher <owner's prism.exe>
# When <pid> exits: stop every test build (exe under PrismGpuiE2E) except the
# owner's launcher, start the owner's launcher if no Prism runs, and restore
# prism.json if it changed. The launcher defaults to the installed Prism.
param(
    [Parameter(Mandatory = $true)][int]$BenchPid,
    [string]$Launcher = (Join-Path $env:LOCALAPPDATA 'Prism\prism.exe')
)

$ErrorActionPreference = 'Continue'
$e2e = Join-Path $env:LOCALAPPDATA 'PrismGpuiE2E'
$json = Join-Path $env:APPDATA 'app.prism.launcher\prism.json'
$backup = Join-Path $e2e 'backup\prism.json'
$logFile = Join-Path $e2e 'guardian.log'

function Log($text) { Add-Content -Path $logFile -Value "$(Get-Date -Format s) [$BenchPid] $text" -Encoding utf8 }

Log 'watching'
while (Get-Process -Id $BenchPid -ErrorAction SilentlyContinue) { Start-Sleep -Seconds 2 }

$test = Get-CimInstance Win32_Process -Filter "Name='prism.exe'" |
    Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.StartsWith($e2e, [StringComparison]::OrdinalIgnoreCase) -and
        -not [string]::Equals($_.ExecutablePath, $Launcher, [StringComparison]::OrdinalIgnoreCase)
    }
foreach ($p in $test) {
    Log "stop test build $($p.ProcessId)"
    Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue
}
if ($test) { Start-Sleep -Seconds 2 }

# Helper modes and parity captures are not launcher instances.
$running = Get-CimInstance Win32_Process -Filter "Name='prism.exe'" |
    Where-Object { $_.CommandLine -notmatch '"?\s+--(prism-start-restore-watchdog|prism-restore-start-menu|parity)' }
if (-not $running) {
    $r = Invoke-CimMethod -ClassName Win32_Process -MethodName Create -Arguments @{
        CommandLine = "`"$Launcher`" --autostart"; CurrentDirectory = (Split-Path $Launcher)
    }
    Log "started the owner's Prism ($Launcher): status $($r.ReturnValue) pid $($r.ProcessId)"
}

if ((Test-Path $backup) -and ((Get-FileHash $json).Hash -ne (Get-FileHash $backup).Hash)) {
    Copy-Item $backup $json -Force
    Log 'prism.json changed: restored the backup'
} else {
    Log 'prism.json unchanged'
}
