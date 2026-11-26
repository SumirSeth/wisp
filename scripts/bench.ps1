<#
.SYNOPSIS
  Compare your branch with main on this machine, and print a summary to paste
  into the pull request.

.DESCRIPTION
  -Perf builds main in a worktree under target\bench\base and your checkout as
  it is, both with nightly,perf-trace, then runs them in turn, A/B/A/B:

    demo   The launcher_demo scenarios from measure-perf.ps1 (launcher toggles,
           typing, dock slide, idle), at 100% and 125% scale. The demo has its
           own APPDATA, window names and mutex, so it never touches your Wisp
           or its config.
    start  The real nightly wisp.exe starting with hide_taskbar = true, also
           with its own APPDATA: when its UI thread is free, and when
           Explorer's taskbar was last visible.

  Before each run it waits until total CPU stays under 10% for 5 s, and prints
  the load it measured. A metric is flagged when every branch round lies
  outside every main round, and the medians differ by more than 10%, by at
  least 0.1 ms or MB (5 points for idle CPU, which is noisy), and by more than
  the spread within either build. It needs three rounds or more to flag
  anything.

  Other Wisp instances would skew the numbers and fight over the taskbar, so
  the script quits them first and starts them again at the end. It asks before
  it starts. Don't use the keyboard or mouse while it runs.

  Results: target\bench\summary.md, target\bench\metrics.csv, and the traces
  in target\perf and target\bench\start.

.EXAMPLE
  scripts\bench.ps1 -Perf

.EXAMPLE
  # Compare with a tag instead of origin/main, without the start runs.
  scripts\bench.ps1 -Perf -Base v0.8.2 -SkipStart
#>
param(
    # Frame timings, startup, CPU, memory and size against main.
    [switch]$Perf,
    # What to compare with. Fetch first if it should be the latest main.
    [string]$Base = 'origin/main',
    # A/B rounds.
    [int]$Rounds = 3,
    # wisp.exe starts per build in each round.
    [int]$Starts = 3,
    # Skip the wisp.exe start runs, which hide and restore the taskbar.
    [switch]$SkipStart,
    # Skip building, and reuse the binaries from the last run.
    [switch]$NoBuild,
    # Don't ask before starting.
    [switch]$Yes
)
$ErrorActionPreference = 'Stop'
# Exit with 1 on any error, also under powershell -File.
trap { $host.UI.WriteErrorLine("$_"); exit 1 }
# The summary goes into English pull requests: 1,234.5 on every system.
[Threading.Thread]::CurrentThread.CurrentCulture = [Globalization.CultureInfo]::InvariantCulture

if (-not $Perf) {
    Get-Help $PSCommandPath -Detailed
    return
}

$repo = Split-Path $PSScriptRoot -Parent
$out = Join-Path $repo 'target\bench'
$baseTree = Join-Path $out 'base'
$bin = Join-Path $out 'bin'
$builds = @('main', 'branch')
$IdleLoad = 10.0
$IdleSeconds = 5
$IdleTimeoutSeconds = 120
$StartWatchMs = 5000
# A flag needs this change in percent, and this much in the metric's unit.
$FlagPercent = 10
$FlagFloor = 0.1
$FlagCpuFloor = 5
# Wisp reads a config without the dock's keys as one from before the dock,
# and turns the dock off. These lines keep it on, at the bottom.
$dockOn = @('dock = true', 'position = "bottom"')
# PowerShell 7 writes no type line and warns that the switch is obsolete; 5.1
# needs it.
$csvOptions = @{}
if ($PSVersionTable.PSVersion.Major -lt 6) { $csvOptions.NoTypeInformation = $true }

Add-Type @"
using System;
using System.Text;
using System.Threading;
using System.Runtime.InteropServices;
public static class BenchNative {
    [DllImport("kernel32.dll")] static extern bool GetSystemTimes(out long idle, out long kernel, out long user);
    // Total CPU load over the next ms milliseconds, in percent of all cores.
    public static double Load(int ms) {
        long i0, k0, u0, i1, k1, u1;
        GetSystemTimes(out i0, out k0, out u0);
        Thread.Sleep(ms);
        GetSystemTimes(out i1, out k1, out u1);
        double total = (k1 - k0) + (u1 - u0);
        return total <= 0 ? 0 : 100.0 * (total - (i1 - i0)) / total;
    }
    public delegate bool EnumProc(IntPtr h, IntPtr l);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    // Whether any of Explorer's taskbars is visible. Wisp's dock keeps a
    // window of the same class for notification icons, so only Explorer's count.
    public static bool TaskbarVisible() {
        bool any = false;
        EnumWindows((h, l) => {
            var name = new StringBuilder(64);
            GetClassNameW(h, name, 64);
            var c = name.ToString();
            if ((c == "Shell_TrayWnd" || c == "Shell_SecondaryTrayWnd") && IsWindowVisible(h)) {
                uint pid;
                GetWindowThreadProcessId(h, out pid);
                try { if (System.Diagnostics.Process.GetProcessById((int)pid).ProcessName == "explorer") any = true; } catch { }
            }
            return true;
        }, IntPtr.Zero);
        return any;
    }
}
"@

function Median([double[]]$values) {
    $sorted = @($values | Sort-Object)
    if ($sorted.Count -eq 0) { return [double]::NaN }
    $mid = [int][Math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2) { return $sorted[$mid] }
    return ($sorted[$mid - 1] + $sorted[$mid]) / 2
}

function Percentile([double[]]$values, [double]$p) {
    $sorted = @($values | Sort-Object)
    if ($sorted.Count -eq 0) { return [double]::NaN }
    return $sorted[[int][Math]::Min($sorted.Count - 1, [Math]::Ceiling($p * $sorted.Count) - 1)]
}

# Waits until the machine is idle, and returns the load it measured.
function Wait-Idle {
    $samples = New-Object System.Collections.Generic.List[double]
    $watch = [Diagnostics.Stopwatch]::StartNew()
    while ($true) {
        $samples.Add([BenchNative]::Load(1000))
        if ($samples.Count -gt $IdleSeconds) { $samples.RemoveAt(0) }
        $load = ($samples | Measure-Object -Average).Average
        if ($samples.Count -eq $IdleSeconds -and $load -lt $IdleLoad) { return $load }
        if ($watch.Elapsed.TotalSeconds -gt $IdleTimeoutSeconds) {
            throw ("The machine stayed busy: {0:N1}% CPU over the last {1} s, the limit is {2}%. Close what's running and try again." -f $load, $IdleSeconds, $IdleLoad)
        }
    }
}

function Read-Trace([string]$path) {
    if (-not (Test-Path -LiteralPath $path)) { return @() }
    @(Import-Csv -LiteralPath $path | ForEach-Object {
            [pscustomobject]@{ metric = $_.metric; start = [double]$_.start_ms; duration = [double]$_.duration_ms }
        })
}

# Spans and inputs worth comparing, as the median duration within a run.
$spanMetrics = @('hotkey_to_content', 'key_to_content', 'show', 'morph', 'paint', 'search', 'slide', 'dock_paint')

# Per-run values from one demo trace.
function Get-DemoMetrics($rows, [string]$scenario) {
    $result = [ordered]@{}
    foreach ($name in $spanMetrics) {
        $durations = @($rows | Where-Object metric -eq $name | ForEach-Object duration)
        if ($durations.Count) { $result["$name ms"] = Median $durations }
    }
    # Gaps between frames, in the scenarios that animate: the launcher morph
    # and the dock slide. Longer gaps are pauses between steps.
    $frameEvent = @{ morph = 'content_frame'; slide = 'slide_frame' }[$scenario]
    if ($frameEvent) {
        $frames = @($rows | Where-Object metric -eq $frameEvent | ForEach-Object start | Sort-Object)
        $gaps = @(for ($i = 1; $i -lt $frames.Count; $i++) {
                $gap = $frames[$i] - $frames[$i - 1]
                if ($gap -lt 50) { $gap }
            })
        if ($gaps.Count -ge 3) { $result['frame gap p95 ms'] = Percentile $gaps 0.95 }
    }
    # The start span holds the first frames and everything else at start.
    $spans = @($rows | Where-Object { $_.duration -gt 0 -and $_.metric -ne 'start' } | ForEach-Object duration)
    if ($spans.Count) { $result['longest span ms'] = ($spans | Measure-Object -Maximum).Maximum }
    $result
}

function Get-OtherWisps {
    $mine = (Resolve-Path -LiteralPath $repo).Path
    @(Get-Process -Name wisp, launcher_demo -ErrorAction SilentlyContinue | Where-Object {
            $_.Path -and -not $_.Path.StartsWith($mine, [StringComparison]::OrdinalIgnoreCase)
        })
}

function Stop-Wisp([string]$exe, [hashtable]$environment) {
    $saved = @{}
    foreach ($name in $environment.Keys) {
        $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
        [Environment]::SetEnvironmentVariable($name, $environment[$name], 'Process')
    }
    try {
        $log = Join-Path $out 'quit.log'
        $quit = Start-Process $exe '--quit' -PassThru -WindowStyle Hidden -RedirectStandardError $log -RedirectStandardOutput "$log.out"
        if (-not $quit.WaitForExit(10000)) { $quit.Kill() }
    } finally {
        # Setting $null from PowerShell leaves an empty variable behind.
        foreach ($name in $saved.Keys) {
            if ($null -eq $saved[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
            else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
        }
    }
}

# --- Plan ---------------------------------------------------------------------

Push-Location $repo
try {
    $baseCommit = (git rev-parse --short --verify --quiet "$Base^{commit}")
    if (-not $baseCommit) { throw "Can't find $Base. Fetch it first, or pass -Base." }
    $baseDate = (git log -1 --format=%cs $baseCommit)
    $branchCommit = (git rev-parse --short HEAD)
    $branchName = (git branch --show-current)
    $dirty = [bool](git status --porcelain --untracked-files=no)
} finally {
    Pop-Location
}
$others = Get-OtherWisps
$canStart = -not $SkipStart

Write-Host ''
Write-Host 'scripts\bench.ps1 -Perf will:'
if (-not $NoBuild) {
    Write-Host "  - check out $Base ($baseCommit, $baseDate) in $baseTree"
    Write-Host "  - build it and your checkout ($branchName at $branchCommit$(if ($dirty) { ' with uncommitted changes' })) with nightly,perf-trace, and without features for the size"
}
Write-Host "  - run $Rounds rounds of main, then branch:"
Write-Host '      the launcher_demo scenarios at 100% and 125% scale (own APPDATA, nothing of yours touched)'
if ($canStart) {
    Write-Host "      $Starts starts of the nightly wisp.exe with hide_taskbar = true (own APPDATA); the taskbar hides and comes back each time"
}
Write-Host "  - wait before each run until CPU stays under $IdleLoad% for $IdleSeconds s"
foreach ($p in $others) {
    Write-Host "  - quit $($p.Path) (PID $($p.Id)) first and start it again at the end"
}
Write-Host "  - write $out\summary.md"
Write-Host ''
Write-Host "It takes about $([Math]::Ceiling($Rounds * 2 * (1 + $(if ($canStart) { $Starts * 0.25 } else { 0 }))) + $(if ($NoBuild) { 0 } else { 6 })) minutes. Don't use the keyboard or mouse meanwhile."
if (-not $Yes) {
    $answer = Read-Host 'Start? [y/N]'
    if ($answer -notmatch '^(y|yes|j|ja)$') { Write-Host 'Nothing done.'; return }
}

# --- Run ----------------------------------------------------------------------

$started = New-Object System.Collections.Generic.List[System.Diagnostics.Process]
$quitOthers = @()
$exes = @{}
$sizes = @{}
$loads = New-Object System.Collections.Generic.List[double]
$values = New-Object System.Collections.Generic.List[object]
$startRuns = New-Object System.Collections.Generic.List[object]
$startData = Join-Path $out 'start-data'
$savedAppData = $env:APPDATA
try {
    New-Item -ItemType Directory -Force $out, $bin | Out-Null
    # A run that stops early leaves no summary that looks like its own.
    Remove-Item -LiteralPath (Join-Path $out 'summary.md'), (Join-Path $out 'metrics.csv') -ErrorAction SilentlyContinue

    # Builds.
    $trees = @{ main = $baseTree; branch = $repo }
    if (-not $NoBuild) {
        Push-Location $repo
        try {
            if (Test-Path -LiteralPath (Join-Path $baseTree '.git')) {
                git -C $baseTree checkout -q --detach $baseCommit
            } else {
                git worktree add -q --detach $baseTree $baseCommit
            }
            if ($LASTEXITCODE) { throw "Couldn't check out $Base in $baseTree." }
        } finally {
            Pop-Location
        }
        # A target folder each: in a shared one, cargo copies a binary to
        # target\release only when it rebuilds it, so a fresh build would
        # leave the other tree's binary there.
        $targets = @{ main = Join-Path $out 'target'; branch = Join-Path $repo 'target' }
        # Builds a tree and copies its binaries, making sure they're its own.
        function Build-Tree([string]$name, [string]$features, [hashtable]$copies) {
            $release = Join-Path $targets[$name] 'release'
            foreach ($from in $copies.Keys) { Remove-Item -LiteralPath (Join-Path $release $from) -ErrorAction SilentlyContinue }
            $arguments = @('build', '--release', '--quiet', '--target-dir', $targets[$name], '--bin', 'wisp')
            if ($features) { $arguments += '--features', $features, '--example', 'launcher_demo' }
            & cargo @arguments
            if ($LASTEXITCODE) { throw "The $name build failed." }
            foreach ($from in $copies.Keys) {
                $file = Join-Path $release $from
                if (-not (Test-Path -LiteralPath $file)) { throw "The $name build left no $file." }
                Copy-Item -LiteralPath $file (Join-Path (Join-Path $bin $name) $copies[$from]) -Force
            }
        }
        foreach ($name in $builds) {
            Push-Location $trees[$name]
            try {
                Write-Host "Building $name..."
                New-Item -ItemType Directory -Force (Join-Path $bin $name) | Out-Null
                Build-Tree $name '' @{ 'wisp.exe' = 'wisp-release.exe' }
                Build-Tree $name 'nightly,perf-trace' @{ 'wisp.exe' = 'wisp.exe'; 'examples\launcher_demo.exe' = 'launcher_demo.exe' }
            } finally {
                Pop-Location
            }
        }
    }
    foreach ($name in $builds) {
        $dir = Join-Path $bin $name
        foreach ($file in 'wisp.exe', 'launcher_demo.exe', 'wisp-release.exe') {
            if (-not (Test-Path -LiteralPath (Join-Path $dir $file))) { throw "Missing $dir\$file. Run without -NoBuild." }
        }
        $exes[$name] = Join-Path $dir 'wisp.exe'
        $sizes[$name] = (Get-Item (Join-Path $dir 'wisp-release.exe')).Length
    }

    # Other instances.
    foreach ($p in $others) {
        Write-Host "Quitting $($p.Path)..."
        $quitOthers += $p.Path
        Stop-Wisp $p.Path @{}
        if (-not $p.WaitForExit(10000)) { throw "$($p.Path) didn't quit. Quit it yourself and try again." }
    }

    if ($canStart) {
        $config = Join-Path $startData 'wisp-nightly'
        New-Item -ItemType Directory -Force $config | Out-Null
        @('hide_taskbar = true', 'file_search = false') + $dockOn | Set-Content (Join-Path $config 'config.toml')
        New-Item -ItemType Directory -Force (Join-Path $out 'start') | Out-Null
        if (-not [BenchNative]::TaskbarVisible()) {
            Write-Warning "Explorer's taskbar is already hidden, so the start runs are skipped."
            $canStart = $false
        }
    }

    # Each build goes first in every other round, and the start runs come after
    # all demo runs, because Explorer is still busy for a while after the
    # taskbar comes back.
    $order = { param($round) if ($round % 2) { $builds } else { $builds[1], $builds[0] } }
    for ($round = 1; $round -le $Rounds; $round++) {
        foreach ($name in & $order $round) {
            $load = Wait-Idle
            $loads.Add($load)
            Write-Host ("Round {0} of {1}, {2}: demo scenarios (CPU before: {3:N1}%)" -f $round, $Rounds, $name, $load)
            $phase = "$name-r$round"
            & (Join-Path $PSScriptRoot 'measure-perf.ps1') -Phase $phase -Repeats 1 -Demo (Join-Path $bin "$name\launcher_demo.exe") | Out-Null
            $perfDir = Join-Path $repo 'target\perf'
            foreach ($run in Get-Content (Join-Path $perfDir "$phase-process.jsonl") | ConvertFrom-Json) {
                $group = "$($run.scenario) @$($run.dpi)"
                $metrics = Get-DemoMetrics (Read-Trace (Join-Path $perfDir "$phase-$($run.dpi)-$($run.scenario)-0.csv")) $run.scenario
                if ($run.scenario -like 'idle*') {
                    $metrics['CPU % of one core'] = $run.steady_cpu_one_core_pct
                    $metrics['peak memory MB'] = $run.peak_sampled_ws_mb
                }
                foreach ($key in $metrics.Keys) {
                    $values.Add([pscustomobject]@{ build = $name; round = $round; group = $group; metric = $key; value = [double]$metrics[$key] })
                }
            }
        }
    }

    for ($round = 1; $round -le $Rounds -and $canStart; $round++) {
        foreach ($name in & $order $round) {
            $startResults = @()
            for ($i = 1; $i -le $Starts; $i++) {
                $load = Wait-Idle
                $loads.Add($load)
                Write-Host ("Round {0} of {1}, {2}: start {3} of {4} (CPU before: {5:N1}%)" -f $round, $Rounds, $name, $i, $Starts, $load)
                $trace = Join-Path $out "start\$name-r$round-$i.csv"
                if (Test-Path -LiteralPath $trace) { Remove-Item -LiteralPath $trace }
                $env:APPDATA = $startData
                $env:WISP_PERF_TRACE = $trace
                $watch = [Diagnostics.Stopwatch]::StartNew()
                $p = Start-Process $exes[$name] -PassThru -RedirectStandardError (Join-Path $out "start\$name-r$round-$i.log") -RedirectStandardOutput (Join-Path $out "start\$name-r$round-$i.out")
                $started.Add($p)
                $env:APPDATA = $savedAppData
                $env:WISP_PERF_TRACE = $null
                $lastVisible = 0.0
                while ($watch.ElapsedMilliseconds -lt $StartWatchMs) {
                    if ([BenchNative]::TaskbarVisible()) { $lastVisible = $watch.Elapsed.TotalMilliseconds }
                    Start-Sleep -Milliseconds 5
                }
                $hidden = -not [BenchNative]::TaskbarVisible()
                Stop-Wisp $exes[$name] @{ APPDATA = $startData }
                if (-not $p.WaitForExit(10000)) { $p.Kill(); throw "The $name wisp.exe didn't quit." }
                $restored = $false
                for ($t = 0; $t -lt 60 -and -not $restored; $t++) {
                    $restored = [BenchNative]::TaskbarVisible()
                    if (-not $restored) { Start-Sleep -Milliseconds 50 }
                }
                if (-not $restored) { throw "The taskbar didn't come back after the $name wisp.exe quit." }
                $rows = Read-Trace $trace
                $start = $rows | Where-Object metric -eq 'start' | Select-Object -First 1
                $startResults += [pscustomobject]@{
                    ui = $(if ($start) { $start.start + $start.duration } else { [double]::NaN })
                    taskbar = $lastVisible
                    hidden = $hidden
                }
                $startRuns.Add([pscustomobject]@{ build = $name; hidden = $hidden; restored = $restored; traced = [bool]$start })
            }
            $group = 'start, hide_taskbar'
            foreach ($pair in @(@('UI thread free ms', 'ui'), @('taskbar last visible ms', 'taskbar'))) {
                $measured = @($startResults | ForEach-Object { $_.($pair[1]) } | Where-Object { -not [double]::IsNaN($_) })
                if ($measured.Count) {
                    $values.Add([pscustomobject]@{ build = $name; round = $round; group = $group; metric = $pair[0]; value = (Median $measured) })
                }
            }
        }
    }
} finally {
    $env:APPDATA = $savedAppData
    $env:WISP_PERF_TRACE = $null
    foreach ($p in $started) {
        if (-not $p.HasExited) {
            $p.Kill()
            $p.WaitForExit(5000) | Out-Null
        }
    }
    # A start that was cut short may have left the taskbar hidden.
    if ($started.Count -and -not [BenchNative]::TaskbarVisible()) {
        foreach ($name in $builds) {
            if ($exes[$name]) { Stop-Wisp $exes[$name] @{ APPDATA = $startData } }
        }
    }
    foreach ($path in $quitOthers) {
        Write-Host "Starting $path again..."
        Start-Process $path
    }
}

# --- Summary ------------------------------------------------------------------

$values | ConvertTo-Csv @csvOptions | Set-Content -LiteralPath (Join-Path $out 'metrics.csv') -Encoding UTF8

function Format-Value([double]$v) {
    if ([double]::IsNaN($v)) { return '-' }
    if ([Math]::Abs($v) -ge 100) { return '{0:N0}' -f $v }
    if ([Math]::Abs($v) -ge 10) { return '{0:N1}' -f $v }
    return '{0:N2}' -f $v
}

$rowsOut = @()
foreach ($g in $values | Group-Object group, metric) {
    $a = @($g.Group | Where-Object build -eq 'main' | ForEach-Object value)
    $b = @($g.Group | Where-Object build -eq 'branch' | ForEach-Object value)
    if (-not $a.Count -or -not $b.Count) { continue }
    $ma = Median $a
    $mb = Median $b
    $delta = if ($ma -ne 0) { 100 * ($mb - $ma) / [Math]::Abs($ma) } else { 0 }
    $aMin = ($a | Measure-Object -Minimum).Minimum; $aMax = ($a | Measure-Object -Maximum).Maximum
    $bMin = ($b | Measure-Object -Minimum).Minimum; $bMax = ($b | Measure-Object -Maximum).Maximum
    $verdict = ''
    # With three rounds each, two builds that don't differ still separate for
    # one metric in ten, so a flag also needs a change worth looking at, and
    # larger than the spread within either build.
    $change = [Math]::Abs($mb - $ma)
    # Idle CPU comes from a few seconds of process time and moves by several points.
    $floor = if ($g.Group[0].metric -like 'CPU*') { $FlagCpuFloor } else { $FlagFloor }
    $worth = [Math]::Abs($delta) -gt $FlagPercent -and $change -ge $floor -and $a.Count -ge 3 -and
        $change -gt [Math]::Max($aMax - $aMin, $bMax - $bMin)
    if ($worth -and $bMin -gt $aMax) { $verdict = 'higher' }
    elseif ($worth -and $bMax -lt $aMin) { $verdict = 'lower' }
    $first = $g.Group[0]
    $rowsOut += [pscustomobject]@{
        group = $first.group
        metric = $first.metric
        main = '{0} ({1}-{2})' -f (Format-Value $ma), (Format-Value $aMin), (Format-Value $aMax)
        branch = '{0} ({1}-{2})' -f (Format-Value $mb), (Format-Value $bMin), (Format-Value $bMax)
        delta = '{0:+0;-0;0}%' -f $delta
        verdict = $verdict
    }
}

function Table($rows) {
    '| Scenario | Metric | main | branch | Change | |'
    '| --- | --- | --- | --- | --- | --- |'
    foreach ($r in $rows) { "| $($r.group) | $($r.metric) | $($r.main) | $($r.branch) | $($r.delta) | $($r.verdict) |" }
}

$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1).Name.Trim()
$windows = Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion'
Add-Type -AssemblyName System.Windows.Forms
$screens = @([System.Windows.Forms.Screen]::AllScreens | ForEach-Object { "$($_.Bounds.Width)x$($_.Bounds.Height)" }) -join ', '
$flagged = @($rowsOut | Where-Object verdict)
$loadText = if ($loads.Count) { '{0:N1}-{1:N1}%' -f ($loads | Measure-Object -Minimum).Minimum, ($loads | Measure-Object -Maximum).Maximum } else { '-' }

$summary = @()
$summary += "**Performance** (``scripts\bench.ps1 -Perf``): $branchName at ``$branchCommit``$(if ($dirty) { ' with uncommitted changes' }) against $Base at ``$baseCommit``, $Rounds rounds."
$summary += ''
$summary += "Machine: $cpu, Windows $($windows.DisplayVersion) build $($windows.CurrentBuild).$($windows.UBR), screens $screens. CPU before each run: $loadText."
$summary += ''
$summary += 'Medians of the per-round values, with their range. Flagged when every branch round lies outside every main round, and the medians differ by more than 10%, by at least 0.1 (5 points for CPU), and by more than the range within either build.'
$summary += ''
if ($flagged.Count) {
    $summary += Table $flagged
} else {
    $summary += 'Nothing outside the run-to-run noise.'
}
$summary += ''
$summary += '| Size | main | branch | Change |'
$summary += '| --- | --- | --- | --- |'
$summary += '| wisp.exe, release | {0:N0} KB | {1:N0} KB | {2:+0.0;-0.0;0.0}% |' -f ($sizes.main / 1KB), ($sizes.branch / 1KB), (100 * ($sizes.branch - $sizes.main) / $sizes.main)
if ($startRuns.Count) {
    foreach ($name in $builds) {
        $runs = @($startRuns | Where-Object build -eq $name)
        $summary += ''
        $summary += "Starts of $name with hide_taskbar: taskbar hidden after 5 s in $(@($runs | Where-Object hidden).Count) of $($runs.Count), back after quitting in $(@($runs | Where-Object restored).Count) of $($runs.Count)."
    }
}
$summary += ''
$summary += '<details><summary>All metrics</summary>'
$summary += ''
$summary += Table $rowsOut
$summary += ''
$summary += '</details>'

$summaryPath = Join-Path $out 'summary.md'
$summary | Set-Content -LiteralPath $summaryPath -Encoding UTF8
Write-Host ''
$summary | Write-Host
Write-Host ''
Write-Host "Saved to $summaryPath. Traces are in target\perf and $out\start."
