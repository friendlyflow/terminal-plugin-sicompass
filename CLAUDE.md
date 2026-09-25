# Project Instructions

terminal-plugin-sicompass was split out of the
[sicompass](https://github.com/friendlyflow/sicompass) workspace, and its git
history before that point is the history of `lib/lib_terminal` (and `lib/lib_shell`, now `src/shell.rs`` there. Work on it is
usually driven from a sicompass checkout next to this one (`../sicompass`),
whose `/commit-and-push`, `/release`, `/sync` and `/update-cargo` take this
repo's name as their first argument and then follow the skills in this repo's
`.claude/skills/`.

It is a sicompass **WASM plugin**: a `cdylib` built for `wasm32-wasip2` with
`sicompass-pdk`, installed by the sicompass Store from this repo's GitHub
releases. The plugin platform is described in
`../sicompass/docs/plugin-platform.md` and `../sicompass/docs/wasm-plugins.md`.

- `plugin.json` is the manifest. Its `name` is `terminal` (the app gives
  `:` and the live input slot special treatment by that name, so it must not
  change) and its `displayName` `terminal` is the settings section
  (`shellProgram`, `commandHistorySize`, `scrollbackSize`,
  `autoEnterDashboard`, the keys the built-in had). It asks for `storage` (the
  recall history), `"filesystem": ["/"]` and the shells by name (`$SHELL` is
  the login shell), which the user approves at install.
- `locales/<lang>.ftl`, every id prefixed `terminal-`, in all four
  languages.

## The sandbox, and what it changes

- **The shell** is `src/shell.rs`: the host's `process` with a PTY in the
  sandbox, portable-pty natively. `Shell::cwd` and `Shell::foreground_busy` are
  the host's `child.cwd` and `child.foreground-busy` (Linux, from `/proc`).
  A process can no longer be renamed for process monitors.
- **`shellProgram` is a name** the manifest lists. A path saved by the
  built-in is taken by its file name (`program_name`).
- **The prompt** needs the user, the host and the home folder, and the
  plugin's own environment is empty, so `src/sys.rs` asks `sh` once.
- **The recall history** is `/storage/history`, the plugin's storage folder.
  No test may reach it: `TEST_NO_HISTORY` defaults on under `cfg(test)`.
- **The interactive dashboard** is the SDK's `DashboardFrame` from the vte
  emulator, handed to the host with `.into()` (the pdk converts, and keys the
  other way).
- A typed `cd` moves the plugin without a navigation call. The pdk's
  `export_plugin!` tells the host (`host.moved-to`), so nothing here has to.

## Environment (Nix)

The toolchain comes from the flake dev shell in [flake.nix](flake.nix): Rust
from rust-overlay with the `wasm32-wasip2` target (nixpkgs' rustc has no `std`
for it), `wasm-tools` and `jq`. Nothing is installed system-wide.

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
  `cargo test` (natively), and `./scripts/release-plugin.sh --dry-run`, which
  also builds the component and audits its imports.
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

The SDK and the pdk come from crates.io (the source is
`../sicompass-plugin-sdk`). The commented-out `[patch]` in `Cargo.toml` is for
working on them together, and stays commented on main.
