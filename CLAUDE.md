# Project Instructions

terminal-plugin-sicompass was split out of the
[sicompass](https://github.com/friendlyflow/sicompass) workspace, and its git
history before that point is the history of `lib/lib_terminal` (and `lib/lib_shell`, now `src/shell.rs`` there. Work on it is
usually driven from a sicompass checkout next to this one (`../sicompass`),
whose `/commit-and-push`, `/release`, `/sync` and `/update-cargo` take this
repo's name as their first argument and then follow the skills in this repo's
`.claude/skills/`.

It is a sicompass **plugin process**: a program (`src/main.rs`) built with the
SDK's `plugin` feature, which sicompass starts and talks to over its stdin and
stdout. It runs with the user's rights. The Store installs it from this repo's
GitHub releases, one build per platform. The plugin platform is described in
`../sicompass/docs/plugin-platform.md`.

- `plugin.json` is the manifest. Its `name` is `terminal` (the app gives
  `:` and the live input slot special treatment by that name, so it must not
  change) and its `displayName` `terminal` is the settings section
  (`shellProgram`, `commandHistorySize`, `scrollbackSize`,
  `autoEnterDashboard`, the keys the built-in had). It asks for `storage` (the
  recall history), `"filesystem": ["/"]` and the shells by name (`$SHELL` is
  the login shell). They are what the plugin declares it does, shown to the
  user before install.
- `locales/<lang>.ftl`, every id prefixed `terminal-`, in all four
  languages.

## How it runs

- **The shell** is `src/shell.rs`: portable-pty (ConPTY on Windows), a child
  of the plugin's process, read by a thread so no call from the app waits on
  it. Its pid goes to the app in `PollResult::child_pid`, which the tab
  switcher names the tab after. `Shell::cwd` is `/proc/<pid>/cwd` on Linux and
  `proc_pidinfo` on macOS (none on Windows). `Shell::foreground_busy` compares
  the PTY's foreground process group (`tcgetpgrp` on the master) with the
  shell's pid (never busy on Windows).
- **`shellProgram`** is `$SHELL` (the login shell: `$SHELL` from the
  environment, then the account's entry, then `/bin/sh`, and on Windows
  `%ComSpec%`, then PowerShell), a name (found on `PATH`, then in
  `~/.local/bin`), or a path.
- **The prompt** needs the user, the host and the home folder: `src/sys.rs`
  reads the environment, and the host name from `gethostname`.
- **The recall history** is `history` in the plugin's storage folder
  (`sicompass_sdk::plugin::storage_dir`). No test may reach it:
  `TEST_NO_HISTORY` defaults on under `cfg(test)`.
- **The interactive dashboard** is the SDK's `DashboardFrame` from the vte
  emulator, handed to the app with `.into()` (the SDK converts, and keys the
  other way).
- **Strings** come from the app (`host::translate`). The unit tests run
  outside sicompass and read the English bundle instead (`src/localize.rs`).
- A typed `cd` moves the plugin without a navigation call. The SDK's runtime
  tells the app after every call that moved it, so nothing here has to.
- stdout is the channel to the app. `println!` lands in stderr, the app's log.
  The app waits for every call to answer, drawing nothing meanwhile.

## Environment (Nix)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix): Rust
from rust-overlay with this computer's plugin target (static musl on Linux,
which nixpkgs' rustc has no `std` for) and `jq`. Nothing is installed
system-wide.

- **Check once per session**, then stick with the answer: `command -v cargo`.
  - Non-empty: the shell is inside `nix develop`, so run `cargo ...` directly.
  - Empty: prefix every toolchain command with `nix develop -c`.
- `nix develop -c <cmd>` prints a `warning: Git tree ... is dirty` line on
  stderr first. That warning is noise, not a failure.
- Evaluate the flake through `git+file://$PWD`, never a plain path (a plain path
  copies `target/` into the store and hangs), and always under `timeout`.
- The version lives in `plugin.json` and in `[package] version` in `Cargo.toml`.
  Bump both together.

## Generated files that are committed

- `THIRD-PARTY-LICENSES.html`: `cargo about generate about.hbs -o
  THIRD-PARTY-LICENSES.html` (cargo-about 0.9.2, the version the `licenses.yml`
  workflow pins). Regenerate and commit it with any dependency change. The
  workflow fails if it drifts.

## Code Style

Follow standard Rust idioms. Use `#[allow(...)]` sparingly and only when
justified. In `README.md`, do not use em dashes or semicolons. Use commas
instead, or split into separate sentences.

## Testing

- After implementing changes, always run the tests before finishing:
  `cargo test`, and `./scripts/release-plugin.sh --dry-run`, which also builds
  this computer's release and verifies it the way the Store will.
- When adding new code, write or update tests.
- If tests fail, fix the code. Never leave a task with failing tests.

## Test Integrity

- Never remove or weaken test assertions to make a failing test pass. Fix the
  code instead.
- If a test itself is genuinely wrong and needs changing, **ask the user
  first** before modifying it.

## Releasing

A release is a `vX.Y.Z` tag on `main`, equal to `plugin.json`'s version. See
`.claude/skills/release/SKILL.md`. Before tagging, run
`nix develop -c ./scripts/release-plugin.sh --dry-run` (needs the
`sicompass-plugin` tool: `cargo install --git
https://github.com/friendlyflow/sicompass-plugin-sdk sicompass-plugin`). The
release workflow signs with the `PLUGIN_SIGNING_KEY` secret and checks it
against the `PLUGIN_PUBLIC_KEY` variable, the key the sicompass store list
names. The secret key file is `~/.config/sicompass/plugin-keys/terminal.key`
on the maintainer's machine. Never print, copy or commit it.

The SDK comes from crates.io (the source is `../sicompass-plugin-sdk`). The
commented-out `[patch]` in `Cargo.toml` is for working on them together, and
stays commented on main.

A release has one archive per platform. The release workflow builds them on
five runners (Linux x86_64 and arm64 as static musl, macOS arm64 and x86_64,
Windows x86_64), then packs, signs and verifies them in one job.
