# Contributing to Wisp

Thanks for helping. Bug reports, fixes and features are all welcome.

Every change on `main` should be easy to trace: what changed, why, who owns it, how it was checked and which release shipped it. The rules below serve that. They apply to everyone, the maintainer included.

## Building, not just coding

> Building something has never been about writing code. When I say I built it, it means I have put effort, thought and intention behind every single thing, and got Claude to write the code because it writes better code than almost all humans do.
>
> — Sumir Seth, maintainer

Wisp is built in that spirit. A good change starts with a reason: a bug you hit, something that felt wrong, an idea you thought through. The code serves that intention.

## AI agents

AI agents are welcome. Use them as code writers, tinkerers and partners for ideas. Much of Wisp is written this way.

**You own what you submit.** The person who opens a pull request or makes a commit owns that change, whoever or whatever typed it. Be ready to explain it, defend it and fix it.

- **Understand every line.** If you can't explain why a line is there, it isn't ready.
- **Verify it.** Test a build with your change on a real desktop, write tests where they fit, and run the [benchmarks](#performance).
- **Think it through.** Consider other monitors, scale settings, virtual desktops, autohide on and off, and Windows 10.

Changes made end to end by an agent with no verifiable check, no real testing and no deliberate thought behind them are closed.

## Before you start

- **Bugs:** search the issues first. If it's new, open a bug report with your Windows version and the steps to reproduce it.
- **Features:** open a feature request first. Wisp aims to be a slim, fast dock and launcher, not a full taskbar replacement, so agree on the idea before you write code.

## Branches

- Fork the repository and branch from `main`. Everyone works on branches; nobody pushes to `main`.
- Name the branch after the change: `fix/…`, `feat/…`, `docs/…` or `perf/…`.
- Keep it short-lived and rebased on `main`. Rebase rather than merging `main` into it.

## Commits

- **Summary line:** imperative, plain language, about 50 to 72 characters, written for users. "Keep dock icons in place when clicked", not "fix bug" or "update dock.rs".
- **Body:** why the change is needed and anything surprising about it. The diff already shows what changed.
- **One logical change per commit.** Squash fixups such as "fix typo" or "address review" before the pull request is merged.
- **Honest authorship.** The author is the person who owns the change. Credit an AI that helped with a `Co-Authored-By:` line.

## Testing your change

1. Install your build with `scripts\install-nightly.ps1`. It installs **Wisp Nightly** beside your normal Wisp, with its own folder, config and shortcut (<kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Space</kbd>). Any build of your branch works, as long as you test the code you submit.
2. Try the change on your desktop, including the edge cases above.
3. If it affects anything the user sees or waits on, [measure it](#performance).
4. Run the checks CI runs:

   ```sh
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo clippy --workspace --all-targets --features wisp/nightly,wisp-settings/nightly,wisp/perf-trace -- -D warnings
   cargo test --workspace
   ```

## Performance

Speed is Wisp's main feature, so a change must not make it slower. Measure every change to the dock, the launcher, search, drawing or startup **before and after**, on the same machine.

1. **Frame timings.** Build the demo with tracing, then capture on `main` and on your branch:

   ```powershell
   cargo build --release --example launcher_demo --features perf-trace
   scripts\measure-perf.ps1 -Phase before   # on main
   scripts\measure-perf.ps1 -Phase after    # on your branch
   ```

   It records the morph, typing, slide and idle scenarios at 100% and 125% scale, and saves traces and CPU and memory figures in `target\perf`.

2. **Real latency and memory.** Run [`bench/bench.ps1`](bench/bench.ps1) against your installed build. It measures shortcut to launcher, typing to results, memory and idle CPU from outside the process. Add `-SkipPowerToys` if you don't have PowerToys. Don't touch the keyboard or mouse while it runs.

3. **Stress it.** Open and close the launcher fast, sweep the pointer across the dock, and switch virtual desktops quickly, with autohide on and off. Watch for dropped frames, stalls and CPU that stays up.

Frames should stay within the display's frame budget (6.9 ms at 144 Hz), and the dock should use almost no CPU when idle. A regression needs a reason worth its cost; look for another way first.

## Pull requests

Every change reaches `main` through a pull request. The template asks for the following:

- **One change** per pull request.
- **A title that reads as a changelog line**, in plain language, such as "Minimize an app when its dock icon is clicked in front". Release notes list pull requests by title.
- **What changed, why, and how you verified it.** Name the build you tested and the setup: Windows version, screens, scale and dock mode. Include before and after numbers for anything that could affect speed.
- **What you didn't test**, so the reviewer knows where to look.
- **The issue it fixes**, as `Fixes #123`.
- **A changelog entry** under `Unreleased` in [CHANGELOG.md](CHANGELOG.md) for anything users notice. Internal changes get the `skip-changelog` label instead.

A pull request merges once CI passes and a build with the change has been tested. See [MAINTAINING.md](MAINTAINING.md) for how pull requests are reviewed and released.

## Writing style

Write code comments, docs, commit messages and UI text in plain, active language. Use short sentences. Name what the user sees and does. For example, write "The dock hides over full-screen apps", not "Fullscreen detection suppresses dock visibility".

## License

By contributing, you agree that your contributions are licensed under the [MIT License](LICENSE).
