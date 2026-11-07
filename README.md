<p align="center">
  <img src="assets/icon.svg" width="96" height="96" alt="Wisp logo">
</p>

<h1 align="center">Wisp</h1>

<p align="center">
  <b>A dock and a launcher for Windows.</b><br>
  Replaces the taskbar. One ~1.5 MB executable.
</p>

<p align="center">
  <a href="https://github.com/SumirSeth/wisp/releases/latest"><img src="https://img.shields.io/github/v/release/SumirSeth/wisp?style=flat-square&color=0891b2" alt="Latest release"></a>
  <a href="https://github.com/SumirSeth/wisp/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/SumirSeth/wisp/ci.yml?branch=main&style=flat-square" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/SumirSeth/wisp?style=flat-square" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2011-0078d4?style=flat-square" alt="Windows 11">
</p>

<p align="center">
  <img src="assets/dock-to-launcher.webp" width="700" alt="The dock slides up, then grows into the launcher">
</p>

Wisp hides the taskbar. A dock takes its place.

Touch the bottom edge of the screen. The dock slides up. To keep it on screen, turn on **Keep the dock visible** in Settings.

Press <kbd>Alt</kbd>+<kbd>Space</kbd>. The dock grows into the launcher. Type, then press <kbd>Enter</kbd>.

Wisp is written in Rust with Win32 and Direct2D. It needs no .NET, web view or PowerToys.

## Dock

- Shows your running apps. Click one to switch to it.
- Holds your tray icons. Icons you pin in Windows sit in the dock. The rest are behind **⌄**.
- Shows volume, battery and the clock. Scroll over the volume to change it. Click it to mute.
- Scroll anywhere else on the dock to switch virtual desktops. A small pill shows which one you are on.
- Hides when you move away. Stays hidden over full-screen apps.
- Needs the real taskbar? Right-click **⌄** and choose **Show taskbar**.

If Wisp quits or crashes, the taskbar comes back. No tray icon is lost.

## Launcher

<p align="center">
  <img src="assets/screenshot-launcher.png" width="700" alt="The launcher searching for apps">
</p>

| Type | Get |
| --- | --- |
| `code` | Apps, then files. `vsc` finds Visual Studio Code. |
| `?invoice` | Files only |
| `^vis` · `term$` · `'note` · `!edge` | Starts with · ends with · exact · exclude |
| `sqrt(2) * 10` | A result. <kbd>Enter</kbd> copies it. |
| `> ipconfig` | Runs in Command Prompt |
| `wifi off` · `bt` · `vol 40` · `mute` | Changes the setting. <kbd>Tab</kbd> lists networks, devices or outputs. |
| `play` · `pause` · `next` | Controls media |
| `w code` | Switches to an open window |
| `lock` · `sleep` · `restart` · `shut down` | Does it. Some ask for a second <kbd>Enter</kbd>. |
| `time tokyo` · `date` | Copies the time or date |
| `dark mode` · `display` · … | Opens the Settings page |
| `now` · `sound` | Live panels for status and audio |
| `quit` | Quits Wisp |
| anything else | Searches the web |

Recent apps rank higher. They show when the box is empty.

| Keys | Action |
| --- | --- |
| <kbd>Alt</kbd>+<kbd>Space</kbd> | Show or hide the launcher |
| <kbd>Enter</kbd> | Open or run the selected result |
| <kbd>Tab</kbd> | More actions, such as **Run as administrator** |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> | Run as administrator |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> | Show in File Explorer |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> | Copy the path |
| <kbd>Esc</kbd> | Close |

## Drag and drop

Drag a result out of the launcher. No need to open File Explorer.

- **Files and folders:** drop into a chat, an email, an upload box or a folder. The original stays put.
- **Apps:** drop on the desktop or in a folder to make a shortcut.
- **Text:** drop a calculator result, the time, the date or a path into any text box.
- **Links:** drop a URL or web search into a browser or a chat.

The launcher steps aside while you drag. Press <kbd>Esc</kbd> to cancel.

## Speed

Wisp 0.2.0 against PowerToys Run 0.94 on the same PC. Median of 30 runs.

| | Wisp | PowerToys Run |
| --- | --- | --- |
| Typing → first results | **24 ms** | 200 ms |
| Shortcut → launcher drawn | **15 ms** | 30 ms |
| Memory | **20 MB** | 302 MB |
| Size on disk | **1.5 MB** (0.4.0) | 794 MB |

The dock grows into the launcher in 155 ms. Each frame takes at most 8 ms. A hidden dock uses almost no CPU.

[`bench/`](bench) has the method and the full results.

## Install

1. Download the zip from the [latest release](https://github.com/SumirSeth/wisp/releases/latest).
2. Extract it to `%LOCALAPPDATA%\Programs\Wisp`.
3. Run `wisp.exe`.

The settings app opens on the first run. It sets your shortcut and **Start with Windows**.

The settings app also updates Wisp. Open it and select **Update** on the **About** page.

> [!NOTE]
> PowerToys Run also uses <kbd>Alt</kbd>+<kbd>Space</kbd>. Turn it off, or change Wisp's shortcut.

> [!NOTE]
> Wisp is not code-signed. If SmartScreen warns you, choose **More info → Run anyway**.

## Config

Open **Settings** from the tray icon. Or edit `%APPDATA%\wisp\config.toml`.

```toml
hotkey = "Alt+Space"
theme = "system"            # "system" | "dark" | "light"
backdrop = "acrylic"        # "acrylic" | "mica" | "solid"
position = "bottom"         # "bottom" | "center"
dock = true
dock_autohide = true        # false keeps the dock on screen
hide_taskbar = true
max_results = 8
show_recent = true
file_search = true
search_url = "https://www.google.com/search?q={query}"
```

Configs from Wisp 0.3 keep the taskbar. To switch, set `dock` and `hide_taskbar` to `true` and `position` to `"bottom"`.

To add your own panel, put a TOML file in `%APPDATA%\wisp\surfaces`. See [`src/surface/now.toml`](src/surface/now.toml).

## How it works

- **Tray icons.** Wisp creates its own hidden `Shell_TrayWnd`. Apps find it first. Wisp copies each icon and passes the message on to Explorer.
- **One window.** The dock and the launcher are the same window. That is how the dock can grow into the launcher.
- **Blur.** Windows turns its blur off for windows that are not active. The dock is never active, so it uses an older blur API that stays on.
- **Drawing.** Direct2D draws in software. It is fast at this size and saves about 30 MB.
- **Apps.** Wisp reads `shell:AppsFolder`, the same list as Start.
- **Files.** Wisp queries the Windows Search index on a worker thread.

## Build

Install [Rust](https://rustup.rs) and the Windows SDK.

```sh
git clone https://github.com/SumirSeth/wisp
cd wisp
cargo build --release --workspace
```

To try changes without touching your installed Wisp, run `scripts\install-nightly.ps1`. It installs **Wisp Nightly** with its own folder, config and shortcut (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd>).

For timings and screenshots, build with `--features perf-trace`. Then run `scripts\measure-perf.ps1` or `scripts\capture-screenshots.ps1`.

## Contributing

CI runs these checks. Run them before you open a pull request:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy --workspace --all-targets --features wisp/nightly,wisp-settings/nightly,wisp/perf-trace -- -D warnings
cargo test --workspace
```

## License

[MIT](LICENSE) © Sumir Seth
