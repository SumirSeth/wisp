<p align="center">
  <img src="assets/icon.svg" width="96" height="96" alt="Wisp logo">
</p>

<h1 align="center">Wisp</h1>

<p align="center">
  <b>A fast launcher for Windows, with a dock that can replace the taskbar.</b>
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

Wisp is a keyboard launcher for Windows 11: press <kbd>Alt</kbd>+<kbd>Space</kbd>, type, press <kbd>Enter</kbd>. Its optional dock hides the taskbar and takes its place, and grows into the launcher when you open it.

The dock and launcher are one 2.2 MB executable, written in Rust with Win32 and Direct2D. Wisp needs no .NET, web view or PowerToys.

## Features

- **Launcher**: finds apps, files and open windows, calculates, runs commands, and searches the web.
- **File search**: file names from the Windows Search index, or from [Everything](https://www.voidtools.com/) on every drive when it's running.
- **System commands**: Wi-Fi, Bluetooth, volume, audio output, media, Night Light, dark mode, power, and the window you opened the launcher over.
- **Dock**: running and pinned apps, with window lists on hover and Jump Lists on right-click. Autohides, or stays visible.
- **Status**: sound, battery, Wi-Fi, Bluetooth and the clock, grouped or as separate icons, or hidden.
- **Tray icons**: the same icons and clicks as on the taskbar.
- **Arrange it**: drag apps and whole sections of the dock into any order.
- **Now**: a panel for time, battery, Wi-Fi networks, Bluetooth devices, volume and media.
- **Look**: light and dark themes, acrylic, mica or solid backdrops, and your own dock color.
- **Win key**: optionally open Wisp with the Windows key instead of Start.
- **Updates**: a daily check, and one-click updates from Settings.

## Install

1. Download `wisp-vX-x86_64-windows.zip` from the [latest release](https://github.com/SumirSeth/wisp/releases/latest).
2. Extract it to `%LOCALAPPDATA%\Programs\Wisp`.
3. Run `wisp.exe`. The settings app opens and sets your shortcut and **Start with Windows**.

Keep `wisp-settings.exe` beside `wisp.exe` for Settings and updates. To update, select **Update** on the **About** page in Settings.

Once available, you can also install with winget:

```powershell
winget install SumirSeth.Wisp
```

Update that install with `winget upgrade SumirSeth.Wisp`.

> [!NOTE]
> Wisp is not code-signed. If SmartScreen warns you, select **More info → Run anyway**.

> [!NOTE]
> PowerToys Run also uses <kbd>Alt</kbd>+<kbd>Space</kbd>. Turn it off, or change Wisp's shortcut.

## Usage

### Launcher

<p align="center">
  <img src="assets/screenshot-launcher.png" width="700" alt="The launcher searching for apps">
</p>

Press <kbd>Alt</kbd>+<kbd>Space</kbd>, type, then press <kbd>Enter</kbd>. Apps you opened recently rank higher, and with the box empty the launcher shows them.

| Type | To |
| --- | --- |
| `code` | Find apps, then files. `vsc` finds Visual Studio Code. |
| `cmd` · `taskmgr` · `regedit` | Find Windows tools by program name, in any Windows language |
| `?invoice` | Find files only |
| `^vis` · `term$` · `'note` · `!edge` | Match the start · the end · exactly · exclude |
| `w code` | Switch to an open window |
| `sqrt(2) * 10` | Calculate. <kbd>Enter</kbd> copies the result. |
| `> ipconfig` | Run a command in Command Prompt |
| `wifi off` · `bt` · `vol 40` · `mute` | Change a setting. <kbd>Tab</kbd> lists networks, devices or outputs. |
| `night light` · `night light off` | Turn Night Light on or off |
| `theme` · `dark mode` · `light mode` | Switch Windows and apps between dark and light mode |
| `minimize` · `maximize` · `restore` · `close window` · `always on top` | Control the window you opened the launcher over |
| `play` · `pause` · `next` | Control media |
| `lock` · `sleep` · `hibernate` · `restart` · `shut down` | Lock or power off. Hibernate appears when Windows allows it. Some ask for a second <kbd>Enter</kbd>. |
| `time tokyo` · `date` | Copy a time or the date |
| `display` · `colors` · … | Open that Windows Settings page |
| `now` · `sound` | Open the status and audio panels |
| `wisp` · `settings` | Open Wisp's Settings |
| `quit` | Quit Wisp |
| Anything else | Search the web |

| Key | To |
| --- | --- |
| <kbd>Alt</kbd>+<kbd>Space</kbd> | Show or hide the launcher |
| <kbd>Enter</kbd> | Open the selected result |
| <kbd>Tab</kbd> or right-click a result | Show more actions, such as **Run as administrator** |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Enter</kbd> | Run as administrator |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>E</kbd> | Show in File Explorer |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>C</kbd> | Copy the path |
| <kbd>Esc</kbd> | Close |

To pin an app without opening it, select its result, press <kbd>Tab</kbd>, then choose **Pin to dock**. The same menu offers **Unpin from dock** for pinned apps.

Drag a result out of the launcher to drop it elsewhere: a file into a chat or a folder, an app onto the desktop as a shortcut, text or a link into any text box.

To open Wisp with the Windows key instead, turn on **Use Win key** in Settings. It replaces your shortcut; Windows shortcuts such as <kbd>Win</kbd>+<kbd>E</kbd> still work. While an elevated app is in front, the key may open Start.

### Now

Type `now`, or click the status icons in the dock. **Now** puts Wi-Fi, Bluetooth, Night light, Dark mode, Mute and Power saver in tiles beside what's playing, with time and battery above and volume below. Power saver is unavailable while plugged in. Lock sits in the footer beside Sleep, Restart, Shut down and Wisp Settings.

Click a radio tile to turn it on or off. Its **›** opens networks or devices. The output name beside volume opens **Sound** for outputs and app volumes. Press <kbd>Esc</kbd> or choose **Back to Now** to return. Use arrow keys to move, and <kbd>Enter</kbd> or <kbd>Space</kbd> to activate.

Custom `now.toml` files keep their list layout. The built-in uses `kind = "quick"` to opt into tiles.

Type `wifi` or `bluetooth` to go straight to a list.

### Dock

The dock appears when the pointer touches the bottom edge of a screen, and hides when you move away. To keep it on screen, turn on **Keep the dock visible** in Settings; maximized windows then stop above it. Full-screen apps hide it; touch the bottom edge to reveal it until you move away, in either mode. This works with borderless fullscreen apps; exclusive fullscreen games may block overlays. With autohide on, turn on **Hide only when windows overlap** to keep the dock shown on an empty desktop.

| Do this | To |
| --- | --- |
| Click an app | Switch to it, or start it if it is pinned and closed |
| Click the app in front | Minimize it |
| Middle-click an app | Open a new window of it |
| Hover an app with several windows | See its windows. Click one to switch to it, middle-click to close it. |
| Right-click an app | Pin or unpin it, close its windows, or open its Jump List |
| Drag an app sideways | Rearrange it; drop among pins to pin it, or among running apps to unpin it |
| Drag the search button, a gap beside the apps, **⌄**, a status icon or the clock | Move that whole section |
| Click a tray icon | Use it as you would on the taskbar |
| Click **⌄** | Show the hidden tray icons |
| Right-click **⌄** | Show the taskbar, or open taskbar settings |
| Click a status icon | Open Now |
| Scroll over sound | Change the volume by 2% a notch |
| Right-click a status icon | Open its Windows settings page |
| Click the clock | Open notifications and the calendar |
| Scroll anywhere else | Switch virtual desktops |
| <kbd>Shift</kbd>+scroll | Scroll through the apps |

Pinned apps sit before running apps. Up to five tray icons show in the dock: the ones turned on in Windows Settings, under **Personalization > Taskbar > Other system tray icons**. Windows turns new icons off, so at first they're all behind **⌄**.

Sound, battery, Wi-Fi and Bluetooth sit in one group. On a wired connection, the Wi-Fi icon shows Ethernet instead. In Settings, under **Status**, give any of them its own icon or hide it.

If Wisp quits or crashes, the taskbar comes back with every tray icon.

## Configuration

Open **Settings** by typing `settings` in the launcher or from Wisp's tray icon, or edit `%APPDATA%\wisp\config.toml`. The tray icon also has **Rescan apps**, **Start with Windows** and **Quit**.

| Setting | Default | What it does |
| --- | --- | --- |
| `hotkey` | `"Alt+Space"` | Shows or hides the launcher |
| `win_key` | `false` | Tap either Windows key alone to show or hide Wisp instead of Start. Replaces `hotkey` while on. |
| `max_results` | `8` | Results visible at once |
| `theme` | `"system"` | `"system"`, `"dark"` or `"light"` |
| `backdrop` | `"acrylic"` | `"acrylic"`, `"mica"` or `"solid"` |
| `position` | `"bottom"` | `"bottom"` grows the launcher from the dock; `"center"` opens it in the middle |
| `show_recent` | `true` | Shows recent apps when the box is empty |
| `search_url` | Google | Web search URL. `{query}` is replaced by your search. |
| `file_search` | `true` | Searches file names |
| `file_search_everything` | `true` | Searches with [Everything](https://www.voidtools.com/) when it's running, on every drive. Otherwise Wisp uses the Windows Search index. |
| `file_search_exclude` | `["node_modules", …]` | Folder names file search skips. Names that start with a dot are always skipped. |
| `check_for_updates` | `true` | Checks daily for new stable releases, except on metered connections |
| `hide_taskbar` | `true` | Hides the Windows taskbar while Wisp runs |
| `dock` | `true` | Shows the dock |
| `dock_autohide` | `true` | Hides the dock until the pointer touches the bottom edge. `false` keeps it visible. |
| `dock_dodge_windows` | `false` | With autohide on, stays visible until a window overlaps the dock. |
| `dock_margin` | `9` | The gap below the dock, in DIPs, from 0 to 24. `0` puts the dock on the screen edge. |
| `dock_screen` | `"pointer"` | `"pointer"` shows the dock on the screen the pointer touches, or the main screen while it stays visible; `"main"`; or a screen chosen in Settings |
| `dock_search_button` | `true` | Shows the search button at the start of the dock |
| `window_list` | `"multiple"` | `"multiple"` lists an app's windows on hover when it has more than one; `"always"` or `"off"` |
| `dock_icon_size` | `32` | App icon size in the dock, 24–40 DIP |
| `dock_order` | `["launcher", "apps", "tray", "status", "sound", "battery", "wifi", "bluetooth", "clock"]` | Sections from left to right |
| `dock_status_group` | `["sound", "battery", "wifi", "bluetooth"]` | Items in the status group |
| `dock_status_icons` | `[]` | Items with their own icon. Items in neither list are hidden. |
| `dock_color` | `""` | The color of the dock and the launcher, as `"#RRGGBB"`, or `"#RRGGBBAA"` where `AA` sets how strongly it covers the theme's acrylic. Mica and solid show it at full strength. Empty uses the theme's colors. |

Configs from Wisp 0.3 keep the taskbar. To switch, set `dock` and `hide_taskbar` to `true` and `position` to `"bottom"`.

To add your own panel, put a TOML file in `%APPDATA%\wisp\surfaces`. See [`src/surface/now.toml`](src/surface/now.toml) for an example.

## Speed

Wisp 0.2.0 against PowerToys Run 0.94 on the same PC, median of 30 runs:

| | Wisp | PowerToys Run |
| --- | --- | --- |
| Typing → first results | **24 ms** | 200 ms |
| Shortcut → launcher drawn | **15 ms** | 30 ms |
| Memory | **20 MB** | 302 MB |
| Size on disk | **2.2 MB** (0.8.0) | 794 MB |

The dock grows into the launcher in 155 ms. A hidden dock uses almost no CPU. [`bench/`](bench) has the method and full results.

## How it works

- **Tray icons:** Wisp creates its own hidden `Shell_TrayWnd`, which apps find first. Wisp copies each icon and passes the message on to Explorer.
- **One window:** the dock and the launcher are the same window, so the dock can grow into the launcher.
- **Blur:** Windows turns off blur for inactive windows. The dock is never active, so it uses an older blur API that stays on.
- **Drawing:** Direct2D draws in software. At this size it is fast and saves about 30 MB.
- **Apps:** Wisp reads `shell:AppsFolder`, the same list Start uses.
- **Files:** Wisp asks Everything over window messages when it's running, and the Windows Search index otherwise, on a worker thread.

## Build from source

Install [Rust](https://rustup.rs) and the Windows SDK, then:

```sh
git clone https://github.com/SumirSeth/wisp
cd wisp
cargo build --release --workspace
```

To try your changes without touching your installed Wisp, run `scripts\install-nightly.ps1`. It installs **Wisp Nightly** with its own folder, config and shortcut (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd>).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## Code signing policy

Free code signing provided by [SignPath.io](https://about.signpath.io), certificate by [SignPath Foundation](https://signpath.org).

- Committers and approvers: [@SumirSeth](https://github.com/SumirSeth).
- Reviewers: [@SumirSeth](https://github.com/SumirSeth) reviews every pull request from other [contributors](https://github.com/SumirSeth/wisp/graphs/contributors).

Releases are built from this repository by GitHub Actions, and each one is approved before it's signed.

## Privacy

Wisp sends nothing about you or your PC. It connects to the internet only to:

- Check GitHub for a new release once a day. Turn this off in Settings, under Updates.
- Download an update when you select **Update**.
- Connect to Wi-Fi networks and Bluetooth devices you pick.
- Open web searches and links you choose, in your browser.

## License

[MIT](LICENSE) © Sumir Seth
