# Native Settings migration

## Current status

Phase 3 is implemented. `wisp.exe --settings [--page general|appearance|search|about]` and `--welcome` open native Settings; the former `--settings-native` alias is removed with the GPUI crate. The release is one application executable plus the temporary 0.8 updater compatibility ZIP. See the phase 3 implementation and validation notes at the end. The inventories below record the original migration plan and phase history.

## Scope and sequencing

Phase 1 adds an opt-in `wisp.exe --settings-native [--page general]` window and the complete General page. Requests reach the existing channel-specific launcher via `channel::NATIVE_SETTINGS_MESSAGE` (WM_APP+10, numeric page in WPARAM). Default tray, command, onboarding and update launch paths stay on GPUI until migration is complete. Phase 2 completes the other native pages and hidden welcome flow.

1. Foundation and General: retained DIP layout, Direct2D/DirectWrite rendering using `Renderer` and installed fonts, native popup menus, keyboard focus, error reporting, shared validated atomic writes, preview harness and tests.
2. Appearance, Search, About and onboarding: all native pages, color editor and debounce, folder names/system picker, saved update status, three welcome steps, keyboard and mouse feedback.
3. Move the updater into wisp.exe with verification, mutex, staged rename/rollback and restart. Rename the running executable to `.old`, put the verified executable at its original path, start it, then exit. Never overwrite the mapped image or wait for our own process to quit. Test failures and rollback offline. Switch launch::settings/update_settings, tray/commands, first-run and daily checks; delete settings/ and GPUI, assets/licenses/dependencies/profile overrides; update packaging/signing, winget, Scoop, README, CONTRIBUTING and MAINTAINING together. Ship one executable. UI Automation and mixed-monitor DPI review remain before enabling by default.

## Complete source inventory and native mapping

`settings/src/ui.rs` owns all views, state/subscriptions, input validation and save dispatch. `config.rs` reads defaults plus 0.3 missing-key compatibility, validates edits, preserves unknown keys/comments and rereads before every atomic edit through `src/config_write.rs`. Native phase 1 compiles this same Store, not a second serialization format. `hotkey.rs` matches the launcher grammar and restores shifted punctuation during GPUI recording. Native uses the launcher's parser and `Editor`. `system.rs` owns Run registry entry, Scoop-stable startup path, PowerToys JSON/process discovery, conflict predicate, config shell-open, titlebar styling and WM_APP+6 notification. Phase 1 reuses that module. `palette.rs` defines black/paper-white surfaces and cyan focus/selection; `assets.rs` supplies nine Lucide SVGs plus Settings' app SVG. Native uses shared theme/fonts and procedural shapes, with no GPUI dependency in wisp.exe. `main.rs` parses hidden update checks before GPUI, welcome and page; deletes old update backups, opens 820x580 or welcome 560x460, minimum 520x420, quits on last close.

### General (phase 1)

| Section/control | Existing behavior/config | Native implementation |
| --- | --- | --- |
| Shortcut text + Record | `hotkey`; validate on edit, save on Enter/blur; recording consumes a chord and Escape cancels, blur stops; parser accepts modifier aliases, letters, digits, punctuation, F1–F24 and named keys | Retained text + shared Editor/parser, record button; error shown without replacing invalid config |
| PowerToys warning | Alt+Space conflict if PowerToys Run enabled in local settings or running process; suppressed for Win-key mode | Shared discovery/predicate; warning row |
| Use Win key | `win_key`; replaces shortcut, keeps saved chord and hides its editor | Toggle, disables shortcut/record and explains why |
| Show dock | `dock` | Toggle |
| Keep dock visible | inverse `dock_autohide`; disabled without dock; reserves work area while taskbar hidden | Inverse toggle |
| Hide only when windows overlap | `dock_dodge_windows`; disabled without dock or with autohide off | Conditional toggle |
| Screen | `dock_screen`; pointer/main/connected screen IDs; disconnected choice retained, case insensitive; pointer uses main for persistent dock | Native popup rebuilt from shared screens; contextual hint |
| Search button | `dock_search_button`; disabled without dock | Toggle |
| App icon size | `dock_icon_size`, integer 24–40 DIP; saves each change | Slider, arrows, pointer positioning |
| Pinned apps / Unpin all | count and clear sibling `dock-pins.toml`, notify launcher; disabled at zero; refreshed on activation | Shared pins loader/writer and reload message |
| App window list | `window_list`: multiple/always/off | Native popup |
| Hide taskbar | `hide_taskbar`; restored at Wisp exit | Toggle |
| Launcher at bottom | `position`: bottom/center | Toggle |
| Group status icons | merges ordered group+own lists into selected placement, disabled if both empty | Toggle and atomic Store list transfer |
| Sound/Battery/Wi-Fi/Bluetooth | `dock_status_group` / `dock_status_icons`: In group, Own icon, Hidden; preserve order, normalize duplicate/unknown names and disjoint lists | Select per status. Existing GPUI has placement buttons but no reorder widget; dock normalizes names into fixed home-cell order. Reordering needs a separate dock layout/config design decision, not an ineffective Settings-only list move. |
| Check for updates | `check_for_updates`; daily/unmetered hint, nightly skips checks | Toggle; fixes missing key allowance in existing Store |
| Start with Windows | channel-specific Run value, quoted adjacent wisp.exe path, Scoop current junction | Shared Windows integration; errors remain visible |

### Appearance (phase 2)

Look group: Theme (`theme`: system/dark/light, changes Settings immediately and native titlebar), Backdrop (`backdrop`: acrylic/mica/solid; description explains fade/blur). Advanced disclosure starts closed. Gap below dock (`dock_margin`, 0–24 DIP, zero "On the edge"). Custom dock color switch (`dock_color`, empty disables; remembers last color, initial #1E3A8ABF). Color picker with nine RGBA presets, palette/hex, alpha and Red/Green/Blue digit-only fields (0–255, blank restored on blur/Enter). Valid colors #RRGGBB/#RRGGBBAA. Keep current alpha for new full-strength palette/six-digit colors unless preset or alpha edit; always serialize 8 digits. Debounce picker writes 250 ms, cancel pending task on reload/custom switch, compare against persisted value. Acrylic shows alpha, Mica/Solid opaque. Results group: Visible results (`max_results`, slider 1–20). Map to retained select/disclosure/slider/text plus a native popup color surface and one-shot debounce killed on close.

### Search (phase 2)

Recent apps (`show_recent`, empty-query hint), Web search URL (`search_url`, must contain literal `{query}`, inline error while invalid; save each valid change), File search (`file_search`), Use Everything (`file_search_everything`, running Everything searches drives). Skipped folders (`file_search_exclude`): Enter/Add trim text, reject empty/slashes/backslashes and case-insensitive duplicates; Remove per row; dot-prefixed names always skipped. Preserve array comments. Map to toggles, validated text and retained list with keyboard remove/add actions.

### About and updates (phase 2 status; phase 3 updater)

Wisp icon/name/version, config path Open (ensure defaults then shell-open/Notepad fallback), Source code Open, error panel with Open config file and Reload. Reload refreshes inputs, screens, sliders, pins and theme and cancels color debounce. Updates: automatic quiet check on Settings open; manual Check/Check again/Try again; Checking and Installing busy disabled controls; Latest/Available/Installed/Failed states; What's new link; Update only for portable installs; package-manager instructions for winget/Scoop; Restart Settings starts updated About and quits old process; sidebar update link opens About. Map to retained buttons/links and background jobs posting completion messages, no persistent polling timer.

`settings/src/update.rs`: nightly returns without network. Hidden check reads opt-in and unrestricted network cost, writes `update.toml` and posts shared notice WM_APP+9. GitHub latest release JSON, numeric version comparison, WinHTTP HTTPS and system proxy/redirects. Prefer release ZIP, verify SHA256SUMS before extracting exact root or release-folder executable names (both slash forms); standalone assets only when ZIP absent, never fallback after corrupt/missing/download failures. Stage both `.new`, lock named mutex, ask same-install launcher to quit and wait, rename old files to `.old`, rollback partial swaps, restart launcher; clean `.old` on background retries at start. Moving this into the launcher requires redesigning shutdown/self-replacement, not copying stop_launcher unchanged.

### Onboarding (phase 2)

`--welcome`: Welcome to Wisp; explanation and shortcut/Win row; Check the shortcut with refreshed PowerToys warning and chord controls; Start with Windows and tray-settings hint. Three progress bars, Back except first, Next then Done; invalid shortcut disables advance, commit before next, stop recording on page change, Done closes. Native normal window uses 560x460 onboarding layout, explicit focus order and same shared saves.

## All integration and distribution paths

- `src/launch.rs`: adjacent wisp-settings.exe for normal/--welcome; missing-exe dialog offers latest release; updates use `--page about`.
- `src/app.rs`: first-run welcome, Command::WispSettings and tray CMD_SETTINGS, Action::Update; WM_APP_RELOAD (+6) reapplies config, refreshes dock pins/search/hotkeys; daily timer spawns `--check-updates`; WM_APP+9 reloads update notice. `src/commands/mod.rs` and search expose command names/actions; preserve those routes when switching.
- `src/channel.rs`: nightly has independent data directory, launcher class, mutex and Run entry. Native Settings adds an independent channel-specific class and uses existing launcher discovery for forwarding. `src/update_notice.rs` shared update.toml state and message; `src/install.rs` lexical winget/Scoop detection and startup junction path.
- `scripts/install-nightly.ps1`: builds both crates/features into target/nightly, copies both executables to LOCALAPPDATA Programs/Wisp Nightly, initializes nightly data from stable on first install with Ctrl+Shift+Space, creates shortcut. Must eventually build/copy one exe and retain data isolation.
- `.github/workflows/release.yml`: workspace build/test, unsigned two-exe upload, SignPath two-exe signing, ZIP with wisp.exe, wisp-settings.exe, README.md, LICENSE, legacy standalone assets, SHA256SUMS, generated manifests and release notes. `.github/workflows/sign-test.yml` and `.signpath` signing configuration also reference both binaries; CI checks both nightly features. Remove only after updater compatibility decision.
- `packaging/winget/generate.ps1` validates both ZIP executables and emits both portable aliases; checked-in `packaging/scoop/wisp.json` has both bins and shortcut integration. Generated manifest ZIP hashes must be regenerated. No additional checked-in winget version manifests.
- README, CONTRIBUTING and MAINTAINING describe the pair/workspace build, GPUI settings, screenshot/package/update procedures; rewrite in final migration phase. Shared src/screens.rs, dock/status_options.rs, dock/pins.rs and config_write.rs must remain after deleting settings/; move shared config/system code out before deletion.

## Lifecycle and risks

Settings shares the dock process and release panic=abort: never use panics for user input or OS failures; propagate errors and keep the last config. Existing `panic_log::install` remains the last-resort evidence hook, not crash isolation. No catch_unwind can recover release aborts. Review reentrant Win32 callbacks and avoid RefCell panics; menus enter a nested message loop. Closing destroys the window and drops its state, renderer/target, edit buffer and retained controls; no Settings timers/threads exist while closed.

DPI: process already per-monitor-v2; window responds to WM_DPICHANGED with suggested bounds and renderer DPI, uses DIP layout/hit testing, scales min size. Test mixed-DPI monitors and work-area centering. Keyboard: Tab/Shift+Tab skip disabled rows and section labels, wrap navigation, focus scrolls into view, Space/Enter invoke, selects use native popup keyboard navigation/Escape, arrows adjust selects/sliders, Page Up/Down scroll. Focus ring is visible. Custom text must retain Editor selection/caret semantics and later expose UI Automation for screen readers. Theme follows config/system, DWM backdrop/titlebar update on theme broadcast; compare both dark/light with GPUI's black/paper white and cyan accent. Font fallback uses Renderer::Fonts. Visual parity, text wrapping at minimum width, automation accessibility and mixed-DPI behavior require review before enabling by default.

## Safe local preview

`wisp.exe --settings-native-preview --config <absolute-temp-config> --page general` starts only the Settings window. It bypasses launcher startup/mutex, indexing, taskbar, global shortcuts and updates, and suppresses external reload/autostart mutations. For old Settings set APPDATA only in the child process to a temporary root with a seeded wisp/config.toml; do not redirect or modify the real profile. Screenshots must capture the window alone, with no synthetic desktop clicks or keys.

## Phase 1 verification and remaining decisions

The implementation is on `feat/native-settings`. Normal tray/command/update/onboarding paths still open the existing Settings executable. Native General includes every existing General control, status placement, pins, screen choices, shortcut editing/recording/conflict warning, conditional availability, shared saves/reload notification and autostart. The toolkit uses the shared renderer/fonts/theme/Editor and on-demand redraws. Its state and render target are dropped on WM_NCDESTROY, with no background workers; phase 2 adds a one-shot color debounce timer that is destroyed with the window.

Checks passed: `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`; the same Clippy invocation with `--features 'wisp/nightly,wisp-settings/nightly'`; and `cargo test --workspace` (263 launcher tests plus 40 Settings tests passed; four existing launcher tests ignored). New tests cover retained layout/focus skipping/wrapping, page argument wire values, General writes/inversions/disabled rows, preservation of external edits/comments, status transfers, screen selection and invalid shortcut rejection against temporary configs.

`scripts/capture-native-settings.ps1` launches Settings with a seeded temporary APPDATA only in the child, and captures the process's own visible window through PrintWindow. Native capture uses the isolated explicit-config preview. No real desktop clicks or keystrokes are sent. Captures under `target/native-settings-shots/`: `old-dark.png`, `old-light.png`, `native-dark.png`, `native-light.png`, `comparison-dark.png`, `comparison-light.png`. The comparisons preserve the original pixels, old on the left and native on the right. Reviewed both themes at the current display DPI. Real multi-monitor DPI transitions, screen-reader UI Automation, and interactive keyboard traversal on the user's desktop remain manual review items; no synthetic desktop input was used to exercise those.

Owner decision: the requested status-list reordering has no existing Settings control or ordered dock representation. Current `StatusPlacement` reduces arrays to boolean membership and restores canonical Sound/Battery/Wi-Fi/Bluetooth order, with fixed matrix home cells. This phase preserves that behavior and exposes all three placement choices. Adding reorder requires deciding whether the group keeps home-cell gaps, packs visible items, or follows an arbitrary 2x2 order, and how that interacts with the dock's separate section order. No nonfunctional Move up control is shipped. This is the one requested behavior not implemented in phase 1.

Release measurement: pristine `origin/main` (`38c1d4b619ffdbf4cbcd26fcf69c7bdfc0fbe830`) built from an archive under target/ with `cargo build --release -p wisp` and the requested CARGO_TARGET_DIR was **2,301,440 bytes**. The final branch built with the same command/target was **2,425,344 bytes**: **+123,904 bytes (121 KiB, 5.38%)**. The archived source is not a Git checkout and no other checkout was modified. Both native themes also closed naturally through WM_CLOSE in the capture harness, exercising the normal Settings teardown path.

## Phase 2 completion

Appearance implements Theme, Backdrop, Advanced disclosure, gap 0-24 DIP, custom color enable/remember, nine RGBA presets, hex, RGB and opacity 0-255 entry, and visible results 1-20. The inline color editor reuses the polished field/select/button drawing; RGB and opacity share a compact row. Six-digit colors retain alpha; presets bring their own alpha. Valid colors debounce 250 ms and compare against persisted state. Reload and custom-color changes cancel pending edits; close flushes valid pending color and window destruction removes its timer. Blank channel entries restore on commit; invalid input leaves the saved config intact. Theme applies immediately.

Search includes Recent apps, live validated Web search URL, File search, Everything and skipped-folder names. Enter/Add trims names, rejects paths/empty values/case-insensitive duplicates, and Remove uses the shared Store to retain array comments. Browse opens an owned system folder picker and adds the selected folder's basename, preserving the existing name-based exclusion semantics. About shows version, config/Open, source/Open, saved update notice/What's new and package-manager guidance. Update checking/downloading/installing remains in the old app for phase 3; native Settings makes no network requests.

`wisp.exe --settings-native --welcome` uses hidden wire page 4. The isolated preview also accepts `--welcome`. Three steps retain welcome copy, shortcut validation and recording, Win key, refreshed PowerToys warning, autostart, Back/Next/Done and progress bars. It opens at 560x460 and scrolls when needed. Existing default launch/first-run paths are unchanged.

All controls have Tab/Shift+Tab focus with visible rings and scroll-to-focus. Selects/sliders accept arrows, folder rows accept Up/Down, menus accept Escape, and Escape collapses the color editor. Buttons/links/navigation/actionable rows have hover/press feedback. Temporary-config tests exercise all writable pages, invalid input, alpha retention, debounce flush/cancel, explicit expanded Appearance focus order, page wrapping, read-only About and welcome progression. The existing General tests pass.

Capture script additions: `-Page general|appearance|search|about`, `-Welcome`, `-Advanced` and `-Picker`; `-Wheel` remains supported. Appearance fixtures enable custom color. Only temporary APPDATA/configs are used; synthetic input is exclusively PostMessage to the verified child window. Artifacts under `target/native-settings-shots/`:

- `old-{page}-{top|scrolled}-{dark|light}.png`
- `native-{page}-{top|scrolled}-{dark|light}.png`
- `comparison-{page}-{top|scrolled}-{dark|light}.png` (old left, native right)
- `old-welcome-{dark|light}.png`, `native-welcome-{dark|light}.png`, `comparison-welcome-{dark|light}.png`
- `native-appearance-picker-{dark|light}.png`
- `phase2-paths.txt` lists every artifact's absolute path.

Owner review: phase 3 must settle updater compatibility with older two-executable installs before switching paths/deleting GPUI. Browse deliberately retains basename semantics; per-path exclusions require a separate config/search change. Existing status reordering, UI Automation and mixed-DPI review remain open.

Phase 2 checks: `cargo fmt` and `cargo fmt --check`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo clippy --workspace --all-targets --features 'wisp/nightly,wisp-settings/nightly' -- -D warnings`; `cargo test --workspace`. All passed: 264 launcher tests and 40 Settings tests, four existing launcher tests ignored. Captures use the final debug build from the same source; release size is measured with `cargo build --release -p wisp` and `CARGO_TARGET_DIR=C:\Users\clash\.t3\projects\wisp-screen\target`. No real profile configuration, other checkout, push or PR was touched.

Phase 2 release wisp.exe: **2463744 bytes**, against the post-polish baseline **2,452,992 bytes**: **+10752 bytes (0.44%)**.


## Phase 3 implementation

Settings and updating now live in wisp.exe. Public entry points are `--settings`, `--page` and `--welcome`; all launcher/dock/tray routes post the native window request. The GPUI crate, assets, workspace dependency and release profile overrides are removed. Shared config and Windows integration live in src/settings_config.rs and src/settings_system.rs.

The portable updater resolves the single executable plus SHA256SUMS.txt, verifies before staging wisp.exe.new, then rename-swaps with rollback. A worker holds the update lock through shutdown; main releases the single-instance handle and hotkeys before allowing restart. Failed restart restores and starts the old binary. About checks on a worker and receives completion messages. Hidden daily checks, unrestricted-network checks, nightly skip, notices, package-manager guidance and What's new remain.

0.9 packaging produces the main standalone executable and a temporary compatibility ZIP with a versioned folder containing two byte-identical copies of that executable named wisp.exe and wisp-settings.exe. The old updater verifies the ZIP checksum and installs both. A process started under the old Settings name only forwards Settings to the same-folder launcher or starts the adjacent wisp.exe with Settings flags, then exits. Cleanup waits while any Settings or Settings.old process from this install folder exists, preserving the old About page's Restart Settings action; it retries briefly and again on future starts. Remove this ZIP after a couple of releases. No 0.8.3 bridge release is needed.

The welcome navigation is below the card at bottom right. Search uses the existing text field plus Add; the redundant Browse row and folder picker are removed.

Validation: scripts/test-update-migration.py builds the tagged 0.8.2 updater with only a local asset download seam, in its own target dir. Fixture hosts compile current Settings/migration/updater code with a hidden launcher window instead of dock startup and isolated class names. They use temporary APPDATA and install folders, never global hotkeys or taskbar changes. This checks both swaps, restart, --page about forwarding, deferred cleanup, and a subsequent single-exe update/restart. scripts/capture-native-settings.ps1 uses --settings with an explicit temporary config and local release fixtures for About states; all messages target only the owned child window. Production downloads still require HTTPS; file:// is the explicit offline fixture seam.

Before release: update the SignPath project artifact configuration to one exe, review winget's migration from nested ZIP to portable EXE, move Unreleased notes into the release section, and release 0.9 directly. Keep the compatibility ZIP for the agreed transition period. UI Automation and mixed-DPI review remain the existing manual review items.


Phase 3 verification completed on 2026-11-22: formatting, stable and nightly workspace/all-target Clippy with warnings denied, and all 268 active workspace tests pass (four existing tests ignored). The release workspace build passes with package version 0.9.0. The migration harness passes with the actual v0.8.2 updater implementation, including About/welcome/hidden-check alias forwarding and next-start cleanup, followed by the single-exe update to fake 0.9.1. Packaging manifest generation was tested with a local executable. Window-only dark/light captures of General, Appearance, Search, About, welcome and About latest/available/error states are listed in target/native-settings-shots/phase3-paths.txt. The harness evidence location is printed in target/migration-final.log. Real profile configuration and installed stable/nightly copies were not changed.

Final 0.9.0 release wisp.exe: 2,573,312 bytes. No wisp-settings crate or separate Settings executable is built. The compatibility ZIP alone carries a byte-identical copy named wisp-settings.exe.
