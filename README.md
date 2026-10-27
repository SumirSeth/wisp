<p align="center">
  <img src="assets/icon.svg" width="96" height="96" alt="Wisp logo">
</p>

<h1 align="center">Wisp</h1>

<p align="center">
  <b>A tiny, instant app launcher for Windows.</b><br>
  A standalone alternative to PowerToys Run in a single ~0.8 MB executable.
</p>

<p align="center">
  <a href="https://github.com/SumirSeth/wisp/releases/latest"><img src="https://img.shields.io/github/v/release/SumirSeth/wisp?style=flat-square&color=0891b2" alt="Latest release"></a>
  <a href="https://github.com/SumirSeth/wisp/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/SumirSeth/wisp/ci.yml?branch=main&style=flat-square" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/SumirSeth/wisp?style=flat-square" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2011-0078d4?style=flat-square" alt="Windows 11">
</p>

<p align="center">
  <img src="assets/screenshot-apps.png" width="700" alt="Wisp searching for apps">
</p>

Press <kbd>Alt</kbd>+<kbd>Space</kbd>, type a few letters, hit <kbd>Enter</kbd>. Wisp is written in Rust directly on Win32, Direct2D and DirectWrite. There's no .NET, no web view and no UI framework, and it doesn't need PowerToys installed.

## Benchmarks

Measured against PowerToys Run 0.94 with its default settings, on the same machine (i5-12500H, 144 Hz display), 30 trials each. Times are median / p90:

| | **Wisp** | PowerToys Run | |
| --- | --- | --- | --- |
| Typing → results on screen | **21 / 21 ms** | 333 / 356 ms | **~16× faster** |
| Hotkey → window on screen | **11 / 23 ms** | 29 / 35 ms | **~2.5× faster** |
| Memory (private bytes) | **18 MB** | 298 MB + 13 MB PowerToys host | **~16× less** |
| CPU time while idle (10 s) | **0 ms** | 63 ms | |
| Download / install size | **0.8 MB** | 794 MB (PowerToys install) | |
| Threads | **14** | 47 | |

Both launchers are measured the same way, from outside the process. [`bench/bench.ps1`](bench/bench.ps1) sends real keystrokes and polls the screen until the pixels stop changing, so the numbers include rendering and compositor time, not just internal search time. Both run over the same still background, and changes too faint to see are ignored. Full results are in [`bench/RESULTS.md`](bench/RESULTS.md). You can rerun it on your own machine.

> [!NOTE]
> With the default acrylic backdrop, Wisp's window and results appear in ~11 ms, but Windows then fades the blur in over ~0.25 s, as it does for the Start menu. PowerToys Run's own backdrop finishes in ~42 ms. If you'd rather everything land at once, set `backdrop = "solid"`: the window is then fully drawn in ~25 ms.

## Features

- **Apps**: everything in Start's "All apps", both desktop programs and Microsoft Store apps, with their real icons.
- **Fuzzy search** powered by [nucleo](https://github.com/helix-editor/nucleo), the matcher from the Helix editor. `vsc` finds **V**isual **S**tudio **C**ode, and scattered junk matches are filtered out.
- **Learns what you use**: frequently and recently launched apps rank higher, and they're listed as soon as Wisp opens.
- **Calculator**: `sqrt(2) * 10`, `2^10`, `(3 + 4) % 5`, `2pi + 1`. Press <kbd>Enter</kbd> to copy the result.
- **Shell commands**: `> ipconfig /all` runs in Command Prompt.
- **URLs and web search**: `https://…` and `www.…` links open in your browser, and anything else can be searched on the web.
- **Native look**: acrylic backdrop, rounded corners, light and dark themes that follow Windows, sharp at any display scaling.
- **Lightweight**: zero CPU while idle. Indexing and icon loading run on a background thread.

<p align="center">
  <img src="assets/screenshot-calc.png" width="700" alt="Wisp calculator">
</p>

## Wisp vs PowerToys Run

PowerToys Run is excellent and does much more. Wisp trades breadth for speed and independence. An honest comparison:

**Only in Wisp**
- Runs on its own as one portable `.exe`, with no PowerToys install and no .NET runtime.
- Instant results on every keystroke. PowerToys Run waits 50–150 ms after you type before it searches.
- fzf-style query operators: `^start`, `end$`, `'exact`, `!exclude`.
- Recently used apps are shown on an empty query.
- Plain-text TOML config that's easy to keep in your dotfiles.

**In both**
- App search with icons and fuzzy matching, with frequently used apps ranked higher
- Calculator, web search, URLs, shell commands
- Run as administrator, open the file location
- Light/dark theme, multi-monitor, per-monitor DPI

**Only in PowerToys Run** (Wisp's roadmap)
- File search through the Windows Search index
- Switching between open windows (Window Walker)
- System commands (shut down, sleep, lock, empty Recycle Bin)
- Unit converter, time & date, value generator (GUIDs, hashes, Base64)
- Windows Settings, services and registry search, folder browsing
- VS Code workspaces, Windows Terminal profiles, OneNote
- A third-party plugin ecosystem, plugin action keywords, and a settings UI

## Install

1. Download `wisp-vX.Y.Z-x86_64.exe` from the [latest release](https://github.com/SumirSeth/wisp/releases/latest). Put it anywhere, for example `%LOCALAPPDATA%\Programs\Wisp\wisp.exe`.
2. Run it. Wisp starts in the system tray. Press <kbd>Alt</kbd>+<kbd>Space</kbd>.
3. Optional: right-click the tray icon and choose **Start with Windows**.

> [!NOTE]
> **Already using PowerToys Run?** It also uses <kbd>Alt</kbd>+<kbd>Space</kbd>, and only one app can have a shortcut. Either turn off PowerToys Run in PowerToys Settings, or give Wisp a different `hotkey` (see [Configuration](#configuration)). Wisp shows a notification if its shortcut is taken.

> [!NOTE]
> Release builds aren't code-signed yet, so Windows SmartScreen may warn the first time you run Wisp. Choose **More info → Run anyway**, or [build it yourself](#building-from-source).

## Usage

| Keys | Action |
| --- | --- |
| <kbd>Alt</kbd>+<kbd>Space</kbd> | Show / hide Wisp |
| <kbd>Enter</kbd> | Open the selected result |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> | Run as administrator |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> | Show the app in File Explorer |
| <kbd>↑</kbd> <kbd>↓</kbd> · <kbd>Tab</kbd> · <kbd>PgUp</kbd> <kbd>PgDn</kbd> · mouse wheel | Move the selection |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> | Copy the selected text, or the calculator result |
| <kbd>Esc</kbd> | Hide |

The search box supports the usual text editing: <kbd>Ctrl</kbd>+<kbd>←</kbd>/<kbd>→</kbd> to jump words, <kbd>Ctrl</kbd>+<kbd>Backspace</kbd> to delete a word, <kbd>Shift</kbd> to select, and <kbd>Ctrl</kbd>+<kbd>A</kbd>/<kbd>V</kbd>/<kbd>X</kbd>.

| Type | Get |
| --- | --- |
| `code` | Apps matching "code" |
| `^vis` · `term$` · `'note` · `!edge` | Starts with · ends with · exact substring · exclude |
| `12 * (3 + 4)` | Calculator result (<kbd>Enter</kbd> copies it) |
| `> winget upgrade` | Run in Command Prompt |
| `https://…` or `www.…` | Open in your browser |
| anything else | "Search the web for …" |

Command line: `wisp.exe --show` opens the running instance (or starts one with the window visible), and `wisp.exe --quit` stops it. Only one instance runs at a time.

## Configuration

Open the config from the tray menu with **Edit settings**, or edit `%APPDATA%\wisp\config.toml` directly. It's created with comments on first run, and changes apply the next time Wisp opens.

```toml
hotkey = "Alt+Space"        # e.g. "Ctrl+Shift+Space", "Win+Alt+W", "Ctrl+F12"
max_results = 8             # rows in the result list
theme = "system"            # "system" | "dark" | "light"
backdrop = "acrylic"        # "acrylic" | "mica" | "solid"
show_recent = true          # recently used apps on an empty query
search_url = "https://www.google.com/search?q={query}"
```

Launch history, which drives the ranking, is stored next to it in `history.toml`.

## Building from source

You need [Rust](https://rustup.rs) (stable, MSVC toolchain) and the Windows SDK, which comes with Visual Studio Build Tools.

```sh
git clone https://github.com/SumirSeth/wisp
cd wisp
cargo build --release   # → target/release/wisp.exe
cargo test
```

Debug builds keep a console window open for logs. Release builds run without one.

## How it works

| File | What it does |
| --- | --- |
| [`src/app.rs`](src/app.rs) | Window, message loop, keyboard and mouse input, show/hide |
| [`src/render.rs`](src/render.rs) | Direct2D/DirectWrite drawing and layout |
| [`src/search.rs`](src/search.rs) | Turns a query into ranked results from all sources |
| [`src/indexer.rs`](src/indexer.rs) | Background app and icon indexing from `shell:AppsFolder` |
| [`src/calc.rs`](src/calc.rs) | Calculator expression parser |
| [`src/editor.rs`](src/editor.rs) | Text editing for the search box |
| [`src/launch.rs`](src/launch.rs) | Starting apps, URLs and commands through the shell |
| [`src/tray.rs`](src/tray.rs) | Tray icon, menu and notifications |
| [`src/config.rs`](src/config.rs), [`src/history.rs`](src/history.rs) | Settings and launch history |

A few decisions that keep it fast:

- **Apps come from `shell:AppsFolder`**, the same source as Start's "All apps". One enumeration covers desktop and Store apps, and every entry launches the same way.
- **Rendering uses Direct2D's software rasterizer.** At this window size a frame takes about a millisecond on the CPU, and not loading the GPU driver saves about 30 MB of memory.
- **The window has a caption style with its frame removed** (`WM_NCCALCSIZE`). Windows only renders real acrylic for windows that have a DWM frame; a plain borderless popup gets a flat grey fallback.
- **The window keeps a fixed height while you type.** Resizing a window that has an acrylic or mica backdrop makes Windows redo the blur, which takes about 250 ms to settle. At a fixed height, each keystroke is a single repaint.

## Contributing

Issues and pull requests are welcome. Before opening a PR, please run `cargo fmt`, `cargo clippy --all-targets` and `cargo test`. CI checks all three. For performance-related changes, include before/after numbers from `bench/bench.ps1`.

## License

[MIT](LICENSE) © Sumir Seth
