# Benchmark results

## Wisp 0.2.0 (whole-word queries, Wisp only)

2026-10-28 · same machine as below · 30 trials · queries: notepad, paint, terminal, settings, calculator.
Typing results include file search, which 0.2.0 adds. PowerToys Run was turned off, so it was not measured.

| wisp | median / p90 |
| --- | --- |
| Shortcut → launcher drawn | 9 / 15 ms |
| Shortcut → keyboard focus | 12 / 20 ms |
| Typing → first results | 9 / 18 ms |
| Typing → results settled | 125 / 207 ms |
| Private memory | 19.4 MB |
| Working set | 71.8 MB |
| Threads | 17 |
| CPU time while idle (10 s) | 0 ms |

## Wisp 0.1.0 and PowerToys Run (one-letter queries)

A bug in bench.ps1 joined the query list into one string, so each typing
trial below typed a single letter instead of a word. Both launchers received
the same input. bench.ps1 now types whole words.

2026-10-27 · 12th Gen Intel(R) Core(TM) i5-12500H · 16 GB RAM · Microsoft Windows 11 Home Single Language (build 26300) · 144 Hz display
PowerToys 0.94.1.0 with default Run config (`search_input_delay` 150 ms, `search_input_delay_fast` 50 ms) · 30 trials each

Times are **median / p90** in milliseconds. The benchmark sends keystrokes and
polls the screen to measure both launchers from outside their processes.
Times include compositor latency (one frame at 144 Hz takes 6.9 ms).
"Settled" means the time of the last detected screen change, including backdrop
animation. A change must affect more than 0.1% of the measured pixels.
Each changed pixel must have a total difference of at least 24 across its red,
green and blue channels. Both launchers run over the same static backdrop window.

| | wisp | PowerToys Run |
| --- | --- | --- |
| Shortcut → launcher drawn | **11 / 23 ms** | 29 / 35 ms |
| Shortcut → launcher settled | **273 / 282 ms** | 42 / 58 ms |
| Shortcut → keyboard focus | **23 / 31 ms** | 16 / 23 ms |
| Typing → first results | **21 / 21 ms** | 209 / 355 ms |
| Typing → results settled | **21 / 21 ms** | 333 / 356 ms |
| Private memory | **18.1 MB** | 297.7 MB |
| Working set | **66.4 MB** | 219.1 MB |
| Threads | **14** | 47 |
| CPU time while idle (10 s) | **0 ms** | 62.5 ms |
| On disk | **0.8 MB** | 794.2 MB |

PowerToys Run also requires the PowerToys host process
(12.9 MB of private memory). The table excludes the host process's memory.
The disk size covers the required PowerToys install.

Run `bench/bench.ps1` to repeat the measurements.
