# Benchmark results

2026-10-27 · 12th Gen Intel(R) Core(TM) i5-12500H · 16 GB RAM · Microsoft Windows 11 Home Single Language (build 26300) · 144 Hz display
PowerToys 0.94.1.0 with default Run settings (`search_input_delay` 150 ms, `search_input_delay_fast` 50 ms) · 30 trials each

Latencies are **median / p90** in milliseconds, measured from outside the process by
synthesizing keystrokes and polling the screen for repaints, so they include
compositor latency (one frame at 144 Hz is 6.9 ms) and apply equally to both.
"Settled" is the last visible change, including any backdrop animation; changes
too faint to see (a few levels of 255) are ignored. Both run over the same still
backdrop window.

| | wisp | PowerToys Run |
| --- | --- | --- |
| Hotkey → window painted | **11 / 23 ms** | 29 / 35 ms |
| Hotkey → window settled | **273 / 282 ms** | 42 / 58 ms |
| Hotkey → keyboard focus | **23 / 31 ms** | 16 / 23 ms |
| Typing → first results | **21 / 21 ms** | 209 / 355 ms |
| Typing → results settled | **21 / 21 ms** | 333 / 356 ms |
| Private memory | **18.1 MB** | 297.7 MB |
| Working set | **66.4 MB** | 219.1 MB |
| Threads | **14** | 47 |
| CPU time while idle (10 s) | **0 ms** | 62.5 ms |
| On disk | **0.8 MB** | 794.2 MB |

PowerToys Run also needs the PowerToys host process running
(12.9 MB private), not counted above. Its disk figure is the
whole PowerToys install, which it can't be installed without.

Reproduce with `bench/bench.ps1`.
