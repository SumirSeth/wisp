# Maintaining Wisp

How Wisp's pull requests, issues and releases are handled. Contributors follow [CONTRIBUTING.md](CONTRIBUTING.md); this file covers what happens after.

## Reviewing pull requests

- **Read the diff before you approve CI** for a first-time contributor. GitHub holds their workflows until a maintainer approves them.
- **CI must pass.** Nothing merges with a failing or missing check.
- **A build with the change must be tested** on a real desktop before merging. The author's test counts if the pull request names the build and setup. Otherwise, test it yourself, for example on Nightly.
- **Check the template is filled in:** why, how it was verified, what wasn't tested, `Fixes #`, and a changelog entry or the `skip-changelog` label.
- **Ask for changes rather than making them.** Ask the author to rebase or fix things. For small fixes, you may push your own commit to their branch, without rewriting theirs.

## Merging

- **Rebase and merge only**, so history stays linear and every commit message survives.
- Delete the branch after merging.
- Close the linked issue through `Fixes #` in the pull request.

## Issues

- Every bug needs steps to reproduce it and a Windows version. Label it `needs-repro` until it has them.
- Close every issue with a reason: fixed by a pull request, won't fix, or duplicate.
- Long-term ideas outside Wisp's scope stay open with the `enhancement` label until they are planned or declined.

### Labels

| Label | For |
| --- | --- |
| `bug` | Something broken |
| `enhancement` | A new feature or an improvement |
| `performance` | Speed, memory or CPU |
| `documentation` | Docs only |
| `windows-10` | Specific to Windows 10 |
| `needs-repro` | A bug without steps to reproduce it yet |
| `good first issue` | Small and well understood |
| `release` | A version bump; left out of release notes |
| `skip-changelog` | Nothing users notice; left out of release notes |

Release notes group pull requests by these labels ([`.github/release.yml`](.github/release.yml)).

## Releases

Wisp uses [semantic versioning](https://semver.org). While the version is 0.x:

- **Patch** (0.5.1) for fixes only.
- **Minor** (0.6.0) for new features or changed behavior.
- **1.0** once the config and behavior are stable and Windows 10 is verified.

To release:

1. Open a release pull request that renames the `Unreleased` section of [CHANGELOG.md](CHANGELOG.md) to the version and date, adds a new empty `Unreleased` section, and bumps the version in `Cargo.toml` and `settings/Cargo.toml`. Label it `release`.
2. Merge it once CI passes.
3. Tag the merged commit on `main` and push the tag:

   ```sh
   git tag -a v0.6.0 -m "Wisp v0.6.0"
   git push origin v0.6.0
   ```

4. The release workflow tests, builds and publishes the release. Its notes are the version's changelog section, followed by the pull requests. The settings app's updater then offers it to users.

Never move or delete a published tag. If a release is broken, ship a patch release.

## Repository settings

Rulesets enforce these rules, and the maintainer may bypass them in an emergency:

- **`main`:** changes only through pull requests, CI must pass, no force pushes, no deletion.
- **`v*` tags:** no updates and no deletion.

## Housekeeping

- Delete merged branches.
- Close stale pull requests after asking the author.
