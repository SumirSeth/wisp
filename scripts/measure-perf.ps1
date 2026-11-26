param(
    [string]$Phase = 'before',
    [int]$Repeats = 3,
    [string]$Demo = 'target/release/examples/launcher_demo.exe',
    [string[]]$Scenarios = @('hidden', 'morph', 'typing', 'slide', 'idle-hidden', 'idle-shown'),
    [int]$DurationMs = 3500,
    [int]$WarmupMs = 1000
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
$variables = @('APPDATA', 'WISP_CAPTURE', 'WISP_CAPTURE_QUERY', 'WISP_CAPTURE_ROWS',
    'DOCK_DEMO_DPI', 'WISP_PERF_SCENARIO', 'WISP_PERF_TRACE', 'WISP_CAPTURE_DURATION_MS')
$saved = @{}
foreach ($name in $variables) { $saved[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
try {
    $demoPath = if ([IO.Path]::IsPathRooted($Demo)) { $Demo } else { Join-Path $repo $Demo }
    $output = Join-Path $repo 'target/perf'
    New-Item -ItemType Directory -Force $output | Out-Null
    $metadata = Join-Path $output "$Phase-process.jsonl"
    [IO.File]::WriteAllText($metadata, '')
    $env:APPDATA = Join-Path $output 'data'
    $data = Join-Path $env:APPDATA 'wisp-demo'
    New-Item -ItemType Directory -Force $data | Out-Null
    'theme="dark"', 'position="bottom"', 'dock=true', 'hide_taskbar=false', 'file_search=false' |
        Set-Content (Join-Path $data 'config.toml')
    $env:WISP_CAPTURE = '1'
    $env:WISP_CAPTURE_DURATION_MS = "$DurationMs"
    $env:WISP_CAPTURE_QUERY = 'time'
    $env:WISP_CAPTURE_ROWS = '3'
    foreach ($dpi in 96, 120) {
        foreach ($scenario in $Scenarios) {
            for ($i = 0; $i -lt $Repeats; $i++) {
                $env:DOCK_DEMO_DPI = "$dpi"
                $env:WISP_PERF_SCENARIO = $scenario
                $env:WISP_PERF_TRACE = Join-Path $output "$Phase-$dpi-$scenario-$i.csv"
                $log = Join-Path $output "$Phase-$dpi-$scenario-$i.log"
                $p = Start-Process $demoPath -PassThru -WindowStyle Hidden -RedirectStandardError $log
                $sw = [Diagnostics.Stopwatch]::StartNew()
                $maxWorking = 0
                $cpu = 0
                $steadyCpu = $null
                $steadyStart = 0
                while (-not $p.WaitForExit(100)) {
                    $p.Refresh()
                    $maxWorking = [Math]::Max($maxWorking, $p.WorkingSet64)
                    $cpu = $p.TotalProcessorTime.TotalMilliseconds
                    if ($null -eq $steadyCpu -and $sw.Elapsed.TotalMilliseconds -ge $WarmupMs) {
                        $steadyCpu = $cpu
                        $steadyStart = $sw.Elapsed.TotalMilliseconds
                    }
                }
                if (($null -ne $p.ExitCode -and $p.ExitCode -ne 0) -or -not (Test-Path -LiteralPath $env:WISP_PERF_TRACE)) {
                    throw "Capture failed: $Phase $dpi $scenario $i; see $log"
                }
                [pscustomobject]@{
                    phase = $Phase
                    dpi = $dpi
                    scenario = $scenario
                    run = $i
                    wall_ms = $sw.Elapsed.TotalMilliseconds
                    cpu_ms = $cpu
                    peak_sampled_ws_mb = $maxWorking / 1MB
                    steady_cpu_one_core_pct = 100 * ($cpu - $steadyCpu) /
                        ([Math]::Max(1, $sw.Elapsed.TotalMilliseconds - $steadyStart))
                } | ConvertTo-Json -Compress | Add-Content $metadata
                Write-Output "$Phase $dpi $scenario $i"
                $p.Dispose()
            }
        }
    }
} finally {
    foreach ($name in $variables) {
        # PowerShell passes $null on as an empty string, and Wisp's test hooks
        # see an empty variable as set, so remove the ones that weren't there.
        if ($null -eq $saved[$name]) { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
        else { [Environment]::SetEnvironmentVariable($name, $saved[$name], 'Process') }
    }
}
