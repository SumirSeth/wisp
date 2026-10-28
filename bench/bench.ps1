<#
.SYNOPSIS
  Compare Wisp and PowerToys Run latency, memory use, idle CPU time and disk size.

.DESCRIPTION
  Send keystrokes and poll the screen to measure both launchers from outside
  their processes:

    open    shortcut pressed -> launcher drawn / launcher settled
    query   query typed      -> first results drawn / results settled

  Both launchers must be running. The benchmark shows a static gradient window
  behind them to prevent other screen changes from affecting their translucent
  backdrops. Do not use the keyboard or mouse during the benchmark. It takes
  about two minutes with the default 20 trials per launcher.

.EXAMPLE
  # PowerToys Run uses Alt+Space. Set hotkey = "Ctrl+Alt+F9" in
  # %APPDATA%\wisp\config.toml, then run:
  .\bench\bench.ps1 -Hotkey "Ctrl+Alt+F9"

  # Or give PowerToys Run another shortcut in PowerToys Settings:
  .\bench\bench.ps1 -Hotkey "Alt+Space" -PowerToysHotkey "Alt+Shift+Space"
#>
param(
    # Measure only the launcher under test, for example when PowerToys Run is turned off.
    [switch]$SkipPowerToys,
    [string]$Process = "wisp",
    [string]$Hotkey = "Ctrl+Alt+F9",
    [string]$PowerToysHotkey = "Alt+Space",
    [int]$Trials = 20,
    [string]$Queries = "notepad,paint,terminal,settings,calculator",
    [string]$Out = (Join-Path $PSScriptRoot "RESULTS.md")
)

$ErrorActionPreference = "Stop"
# A new variable: $Queries keeps its [string] type, so assigning the array back
# to it would join the words into one string again.
$queryList = $Queries.Split(",")
Add-Type -Path (Join-Path $PSScriptRoot "Native.cs")
[Native]::SetProcessDPIAware() | Out-Null

$vk = @{ Ctrl = 0x11; Alt = 0x12; Shift = 0x10; Win = 0x5B; Space = 0x20; Esc = 0x1B; A = 0x41 }
1..24 | ForEach-Object { $vk["F$_"] = 0x6F + $_ }

function Chord([string[]]$keys) {
    $codes = [uint16[]]@($keys | ForEach-Object { $vk[$_] })
    [uint16[]]$mods = @()
    if ($codes.Length -gt 1) { $mods = $codes[0..($codes.Length - 2)] }
    [Native]::Chord($mods, $codes[-1])
}

function Stats([double[]]$xs) {
    $s = $xs | Where-Object { $_ -ge 0 } | Sort-Object
    if (-not $s) { return [pscustomobject]@{ Median = [double]::NaN; P90 = [double]::NaN } }
    [pscustomobject]@{
        Median = $s[[int][math]::Floor(($s.Count - 1) / 2)]
        P90    = $s[[int][math]::Ceiling(0.9 * $s.Count) - 1]
    }
}

function Bench-Launcher([string]$name, [string]$processName, [string[]]$hotkey) {
    $proc = Get-Process $processName -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $proc) { throw "$name ($processName) is not running" }

    # Open the launcher once before measuring to exclude first-open costs.
    # A launcher that started recently can take several seconds to open the first time.
    Chord $hotkey
    $deadline = (Get-Date).AddSeconds(10)
    while ([Native]::ForegroundPid() -ne $proc.Id -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 100 }
    if ([Native]::ForegroundPid() -ne $proc.Id) { throw "$name did not open on $($hotkey -join '+')" }
    Start-Sleep -Milliseconds 700
    $rect = New-Object Native+RECT
    [Native]::GetWindowRect([Native]::GetForegroundWindow(), [ref]$rect) | Out-Null
    Chord @("Esc")
    Start-Sleep -Milliseconds 500

    # Exclude the search box so the blinking caret does not affect measurements.
    $region = New-Object Native+Region($rect.Left, ($rect.Top + 70), ($rect.Right - $rect.Left), 400)
    $open = @(); $openSettled = @(); $focus = @(); $query = @(); $querySettled = @()

    for ($i = 0; $i -lt $Trials; $i++) {
        $o = [Native]::Measure({ Chord $hotkey }.GetNewClosure(), $region, $proc.Id, 150, 2000)
        Start-Sleep -Milliseconds 250
        $text = $queryList[$i % $queryList.Count]
        $q = [Native]::Measure({ Chord @("Ctrl", "A"); [Native]::Type($text) }.GetNewClosure(), $region, 0, 350, 3000)
        Chord @("Esc")
        Start-Sleep -Milliseconds 450

        $open += $o[0]; $openSettled += $o[1]; $focus += $o[2]
        $query += $q[0]; $querySettled += $q[1]
        Write-Host -NoNewline "."
    }
    Write-Host " $name done"

    # Hide the launcher to exclude caret repaints from idle CPU time.
    $proc.Refresh(); $cpu0 = $proc.TotalProcessorTime
    Start-Sleep -Seconds 10
    $proc.Refresh(); $idleCpuMs = ($proc.TotalProcessorTime - $cpu0).TotalMilliseconds

    [pscustomobject]@{
        Name         = $name
        Open         = Stats $open
        OpenSettled  = Stats $openSettled
        Focus        = Stats $focus
        Query        = Stats $query
        QuerySettled = Stats $querySettled
        PrivateMB    = [math]::Round($proc.PrivateMemorySize64 / 1MB, 1)
        WorkingSetMB = [math]::Round($proc.WorkingSet64 / 1MB, 1)
        Threads      = $proc.Threads.Count
        IdleCpuMs    = [math]::Round($idleCpuMs, 1)
    }
}

function Size-MB([string]$path) {
    $item = Get-Item $path
    $files = if ($item.PSIsContainer) { Get-ChildItem $path -Recurse -File -Force } else { $item }
    $bytes = ($files | Measure-Object Length -Sum).Sum
    [math]::Round($bytes / 1MB, 1)
}

$backdrop = Start-Process powershell -PassThru -WindowStyle Hidden -ArgumentList @(
    "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $PSScriptRoot "backdrop.ps1"))
Start-Sleep -Seconds 3
try {
    $ours = Bench-Launcher $Process $Process $Hotkey.Split("+")
    if (-not $SkipPowerToys) {
        $pt = Bench-Launcher "PowerToys Run" "PowerToys.PowerLauncher" $PowerToysHotkey.Split("+")
    }
} finally {
    Stop-Process -Id $backdrop.Id -ErrorAction SilentlyContinue
}

if ($SkipPowerToys) {
    function Ms($s) { "{0:0} / {1:0}" -f $s.Median, $s.P90 }
    $md = @"
| $Process | median / p90 |
| --- | --- |
| Shortcut → launcher drawn | $(Ms $ours.Open) ms |
| Shortcut → launcher settled | $(Ms $ours.OpenSettled) ms |
| Shortcut → keyboard focus | $(Ms $ours.Focus) ms |
| Typing → first results | $(Ms $ours.Query) ms |
| Typing → results settled | $(Ms $ours.QuerySettled) ms |
| Private memory | $($ours.PrivateMB) MB |
| Working set | $($ours.WorkingSetMB) MB |
| Threads | $($ours.Threads) |
| CPU time while idle (10 s) | $($ours.IdleCpuMs) ms |
"@
    $md | Set-Content -Encoding utf8 $Out
    $md
    return
}

$ptHost = Get-Process PowerToys -ErrorAction SilentlyContinue | Select-Object -First 1
$ptDir = Split-Path (Get-Process PowerToys.PowerLauncher).Path
$ptVersion = (Get-Item (Join-Path $ptDir "PowerToys.exe")).VersionInfo.ProductVersion
$oursExe = (Get-Process $Process).Path
$ptSettings = Get-Content "$env:LOCALAPPDATA\Microsoft\PowerToys\PowerToys Run\settings.json" -Raw | ConvertFrom-Json

$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
$ram = [math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB)
$os = (Get-CimInstance Win32_OperatingSystem)
$hz = (Get-CimInstance Win32_VideoController | Where-Object CurrentRefreshRate | Select-Object -First 1).CurrentRefreshRate

function Row($label, $a, $b, $unit = "ms") {
    "| $label | **$("$a $unit".Trim())** | $("$b $unit".Trim()) |"
}
function Ms($s) { "{0:0} / {1:0}" -f $s.Median, $s.P90 }

$md = @"
# Benchmark results

$(Get-Date -Format "yyyy-MM-dd") · $cpu · $ram GB RAM · $($os.Caption) (build $($os.BuildNumber)) · $hz Hz display
PowerToys $ptVersion with default Run config (``search_input_delay`` $($ptSettings.properties.search_input_delay) ms, ``search_input_delay_fast`` $($ptSettings.properties.search_input_delay_fast) ms) · $Trials trials each

Times are **median / p90** in milliseconds. The benchmark sends keystrokes and
polls the screen to measure both launchers from outside their processes.
Times include compositor latency (one frame at $hz Hz takes $([math]::Round(1000 / $hz, 1)) ms).
"Settled" means the time of the last detected screen change, including backdrop
animation. A change must affect more than 0.1% of the measured pixels.
Each changed pixel must have a total difference of at least 24 across its red,
green and blue channels. Both launchers run over the same static backdrop window.

| | $Process | PowerToys Run |
| --- | --- | --- |
$(Row "Shortcut → launcher drawn" (Ms $ours.Open) (Ms $pt.Open))
$(Row "Shortcut → launcher settled" (Ms $ours.OpenSettled) (Ms $pt.OpenSettled))
$(Row "Shortcut → keyboard focus" (Ms $ours.Focus) (Ms $pt.Focus))
$(Row "Typing → first results" (Ms $ours.Query) (Ms $pt.Query))
$(Row "Typing → results settled" (Ms $ours.QuerySettled) (Ms $pt.QuerySettled))
$(Row "Private memory" $ours.PrivateMB $pt.PrivateMB "MB")
$(Row "Working set" $ours.WorkingSetMB $pt.WorkingSetMB "MB")
$(Row "Threads" $ours.Threads $pt.Threads "")
$(Row "CPU time while idle (10 s)" $ours.IdleCpuMs $pt.IdleCpuMs "ms")
$(Row "On disk" (Size-MB $oursExe) (Size-MB $ptDir) "MB")

PowerToys Run also requires the PowerToys host process
($([math]::Round($ptHost.PrivateMemorySize64 / 1MB, 1)) MB of private memory). The table excludes the host process's memory.
The disk size covers the required PowerToys install.

Run ``bench/bench.ps1`` to repeat the measurements.
"@

$md | Set-Content -Encoding utf8 $Out
$md
