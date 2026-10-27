<#
.SYNOPSIS
  Black-box benchmark: this launcher vs. PowerToys Run.

.DESCRIPTION
  Drives both launchers with real synthesized keystrokes and watches the screen
  for repaints, so both are measured the same way, from the outside:

    open    hotkey pressed  -> window painted / animation settled
    query   query typed     -> first results drawn / results settled

  Plus memory, idle CPU and disk footprint. A still gradient window is shown
  behind the launchers for the duration, so nothing changing on screen bleeds
  through their translucent backdrops. Don't touch the keyboard or mouse while
  it runs (about two minutes). Both launchers must already be running.

.EXAMPLE
  # Give the launcher a free hotkey first (PowerToys Run owns Alt+Space), e.g.
  # hotkey = "Ctrl+Alt+F9" in %APPDATA%\<name>\config.toml, then:
  .\bench\bench.ps1 -Hotkey "Ctrl+Alt+F9"
#>
param(
    [string]$Process = "wisp",
    [string]$Hotkey = "Ctrl+Alt+F9",
    [int]$Trials = 20,
    [string]$Queries = "notepad,paint,terminal,settings,calculator",
    [string]$Out = (Join-Path $PSScriptRoot "RESULTS.md")
)

$ErrorActionPreference = "Stop"
$Queries = $Queries.Split(",")
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

    # Warm up once and find where the window appears.
    Chord $hotkey
    Start-Sleep -Milliseconds 700
    if ([Native]::ForegroundPid() -ne $proc.Id) { throw "$name did not open on $($hotkey -join '+')" }
    $rect = New-Object Native+RECT
    [Native]::GetWindowRect([Native]::GetForegroundWindow(), [ref]$rect) | Out-Null
    Chord @("Esc")
    Start-Sleep -Milliseconds 500

    # Watch the results area below the search box (the blinking caret stays out of it).
    $region = New-Object Native+Region($rect.Left, ($rect.Top + 70), ($rect.Right - $rect.Left), 400)
    $open = @(); $openSettled = @(); $focus = @(); $query = @(); $querySettled = @()

    for ($i = 0; $i -lt $Trials; $i++) {
        $o = [Native]::Measure({ Chord $hotkey }.GetNewClosure(), $region, $proc.Id, 150, 2000)
        Start-Sleep -Milliseconds 250
        $text = $Queries[$i % $Queries.Count]
        $q = [Native]::Measure({ Chord @("Ctrl", "A"); [Native]::Type($text) }.GetNewClosure(), $region, 0, 350, 3000)
        Chord @("Esc")
        Start-Sleep -Milliseconds 450

        $open += $o[0]; $openSettled += $o[1]; $focus += $o[2]
        $query += $q[0]; $querySettled += $q[1]
        Write-Host -NoNewline "."
    }
    Write-Host " $name done"

    # Idle cost, measured while hidden.
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

# ---------------------------------------------------------------------------

$backdrop = Start-Process powershell -PassThru -WindowStyle Hidden -ArgumentList @(
    "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $PSScriptRoot "backdrop.ps1"))
Start-Sleep -Seconds 3
try {
    $ours = Bench-Launcher $Process $Process $Hotkey.Split("+")
    $pt = Bench-Launcher "PowerToys Run" "PowerToys.PowerLauncher" @("Alt", "Space")
} finally {
    Stop-Process -Id $backdrop.Id -ErrorAction SilentlyContinue
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
PowerToys $ptVersion with default Run settings (``search_input_delay`` $($ptSettings.properties.search_input_delay) ms, ``search_input_delay_fast`` $($ptSettings.properties.search_input_delay_fast) ms) · $Trials trials each

Latencies are **median / p90** in milliseconds, measured from outside the process by
synthesizing keystrokes and polling the screen for repaints, so they include
compositor latency (one frame at $hz Hz is $([math]::Round(1000 / $hz, 1)) ms) and apply equally to both.
"Settled" is the last visible change, including any backdrop animation; changes
too faint to see (a few levels of 255) are ignored. Both run over the same still
backdrop window.

| | $Process | PowerToys Run |
| --- | --- | --- |
$(Row "Hotkey → window painted" (Ms $ours.Open) (Ms $pt.Open))
$(Row "Hotkey → window settled" (Ms $ours.OpenSettled) (Ms $pt.OpenSettled))
$(Row "Hotkey → keyboard focus" (Ms $ours.Focus) (Ms $pt.Focus))
$(Row "Typing → first results" (Ms $ours.Query) (Ms $pt.Query))
$(Row "Typing → results settled" (Ms $ours.QuerySettled) (Ms $pt.QuerySettled))
$(Row "Private memory" $ours.PrivateMB $pt.PrivateMB "MB")
$(Row "Working set" $ours.WorkingSetMB $pt.WorkingSetMB "MB")
$(Row "Threads" $ours.Threads $pt.Threads "")
$(Row "CPU time while idle (10 s)" $ours.IdleCpuMs $pt.IdleCpuMs "ms")
$(Row "On disk" (Size-MB $oursExe) (Size-MB $ptDir) "MB")

PowerToys Run also needs the PowerToys host process running
($([math]::Round($ptHost.PrivateMemorySize64 / 1MB, 1)) MB private), not counted above. Its disk figure is the
whole PowerToys install, which it can't be installed without.

Reproduce with ``bench/bench.ps1``.
"@

$md | Set-Content -Encoding utf8 $Out
$md
