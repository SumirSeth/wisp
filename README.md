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

Wisp hides the taskbar and puts a dock in its place. Press <kbd>Alt</kbd>+<kbd>Space</kbd> and the dock grows into a launcher.

It is written in Rust with Win32 and Direct2D. It needs no .NET, web view or PowerToys.

## Install

1. Download the zip from the [latest release](https://github.com/SumirSeth/wisp/releases/latest).
2. Extract it to `%LOCALAPPDATA%\Programs\Wisp`.
3. Run `wisp.exe`. The settings app opens and sets your shortcut and **Start with Windows**.

To update, open the settings app and select **Update** on the **About** page.

> [!NOTE]
> PowerToys Run also uses <kbd>Alt</kbd>+<kbd>Space</kbd>. Turn it off, or change Wisp's shortcut.

> [!NOTE]
> Wisp is not code-signed. If SmartScreen warns you, select **More info → Run anyway**.

## Dock

The dock appears when the pointer touches the bottom edge of a screen, and hides when you move away. To keep it on screen, turn on **Keep the dock visible** in Settings. Maximized windows then stop above it.

| Do this | To |
| --- | --- |
| Click an app | Switch to it |
| Click the app in front | Minimize it |
| Middle-click an app | Open a new window of it |
| Right-click an app | Close its windows |
| Click a tray icon | Use it as you would on the taskbar |
| Click **⌄** | Show the hidden tray icons |
| Right-click **⌄** | Show the taskbar, or open taskbar settings |
| Click the volume | Mute or unmute |
| Scroll over the volume | Change the volume |
| Right-click the volume | Open sound settings |
| Click the clock | Open notifications and the calendar |
| Click the battery | Open battery settings |
| Scroll anywhere else | Switch virtual desktops |
| <kbd>Shift</kbd>+scroll | Scroll through the apps |

The dock hides over full-screen apps. Tray icons you pin in Windows show in the dock; the rest are behind **⌄**.

If Wisp quits or crashes, the taskbar comes back with every tray icon.

## Launcher

<p align="center">
  <img src="assets/screenshot-launcher.png" width="700" alt="The launcher searching for apps">
</p>

Type to search, then press <kbd>Enter</kbd>.

| Type | To |
| --- | --- |
| `code` | Find apps, then files. `vsc` finds Visual Studio Code. |
| `?invoice` | Find files only |
| `^vis` · `term$` · `'note` · `!edge` | Match the start · the end · exactly · exclude |
| `sqrt(2) * 10` | Calculate. <kbd>Enter</kbd> copies the result. |
| `> ipconfig` | Run a command in Command Prompt |
| `wifi off` · `bt` · `vol 40` · `mute` | Change a setting. <kbd>Tab</kbd> lists networks, devices or outputs. |
| `play` · `pause` · `next` | Control media |
| `w code` | Switch to an open window |
| `lock` · `sleep` · `restart` · `shut down` | Lock or power off. Some ask for a second <kbd>Enter</kbd>. |
| `time tokyo` · `date` | Copy a time or the date |
| `dark mode` · `display` · … | Open that Settings page |
| `now` · `sound` | Open live panels for status and audio |
| `quit` | Quit Wisp |
| Anything else | Search the web |

Apps you opened recently rank higher. With the box empty, the launcher shows your recent apps.

| Key | To |
| --- | --- |
| <kbd>Alt</kbd>+<kbd>Space</kbd> | Show or hide the launcher |
| <kbd>Enter</kbd> | Open the selected result |
| <kbd>Tab</kbd> | Show more actions, such as **Run as administrator** |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> | Run as administrator |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> | Show in File Explorer |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> | Copy the path |
| <kbd>Esc</kbd> | Close |

### Drag and drop

Drag a result out of the launcher and drop it anywhere:

- **Files and folders** into a chat, an email, an upload box or a folder. The original stays where it is.
- **Apps** onto the desktop or into a folder, to make a shortcut.
- **Text**, such as a result, the time or a path, into any text box.
- **Links and web searches** into a browser or a chat.

Press <kbd>Esc</kbd> to cancel a drag.

## Settings

Open **Settings** from Wisp's tray icon, or edit `%APPDATA%\wisp\config.toml`. The tray icon also has **Rescan apps**, **Start with Windows** and **Quit**.

| Setting | Default | What it does |
| --- | --- | --- |
| `hotkey` | `"Alt+Space"` | Shows or hides the launcher |
| `dock` | `true` | Shows the dock |
| `dock_autohide` | `true` | Hides the dock until the pointer touches the bottom edge. `false` keeps it visible. |
| `window_list` | `"multiple"` | Shows the app's window list for multiple windows, always, or never |
| `hide_taskbar` | `true` | Hides the Windows taskbar while Wisp runs |
| `position` | `"bottom"` | `"bottom"` grows the launcher from the dock; `"center"` opens it in the middle |
| `theme` | `"system"` | `"system"`, `"dark"` or `"light"` |
| `backdrop` | `"acrylic"` | `"acrylic"`, `"mica"` or `"solid"` |
| `max_results` | `8` | Results visible at once |
| `show_recent` | `true` | Shows recent apps when the box is empty |
| `file_search` | `true` | Searches files in the Windows Search index |
| `file_search_exclude` | `["node_modules", …]` | Folder names file search skips |
| `search_url` | Google | Web search URL. `{query}` is replaced by your search. |

Configs from Wisp 0.3 keep the taskbar. To switch, set `dock` and `hide_taskbar` to `true` and `position` to `"bottom"`.

To add your own panel, put a TOML file in `%APPDATA%\wisp\surfaces`. See [`src/surface/now.toml`](src/surface/now.toml) for an example.

## Speed

Wisp 0.2.0 against PowerToys Run 0.94 on the same PC, median of 30 runs:

| | Wisp | PowerToys Run |
| --- | --- | --- |
| Typing → first results | **24 ms** | 200 ms |
| Shortcut → launcher drawn | **15 ms** | 30 ms |
| Memory | **20 MB** | 302 MB |
| Size on disk | **1.5 MB** (0.4.0) | 794 MB |

The dock grows into the launcher in 155 ms. A hidden dock uses almost no CPU. [`bench/`](bench) has the method and full results.

## How it works

- **Tray icons:** Wisp creates its own hidden `Shell_TrayWnd`, which apps find first. Wisp copies each icon and passes the message on to Explorer.
- **One window:** the dock and the launcher are the same window, so the dock can grow into the launcher.
- **Blur:** Windows turns off blur for inactive windows. The dock is never active, so it uses an older blur API that stays on.
- **Drawing:** Direct2D draws in software. At this size it is fast and saves about 30 MB.
- **Apps:** Wisp reads `shell:AppsFolder`, the same list Start uses.
- **Files:** Wisp queries the Windows Search index on a worker thread.

## Build

Install [Rust](https://rustup.rs) and the Windows SDK, then:

```sh
git clone https://github.com/SumirSeth/wisp
cd wisp
cargo build --release --workspace
```

To try your changes without touching your installed Wisp, run `scripts\install-nightly.ps1`. It installs **Wisp Nightly** with its own folder, config and shortcut (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd>).

To contribute, see [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © Sumir Seth
