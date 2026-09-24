# terminal_plugin_sicompass

*A real shell, in Sicompass.*

This plugin is part of [Sicompass](https://github.com/friendlyflow/sicompass), a
keyboard-first, accessibility-first way to use your entire computer.

The terminal starts on a folder listing. Walk to a folder and press : for a
shell there. Commands and their output become rows, i on the last row types a
command, and the rows under it are your earlier commands to reuse. Escape goes
back to the folders, where the shell ended up. Full screen programs like vim,
less and htop open in a dashboard that hands them the raw keys, and pressing
ctrl+c twice leaves it.

It runs your own login shell, or the one you name in Settings under terminal,
with your own environment.

The terminal asks for your whole disk, to pick a folder, and to run your
shells. The Store shows that before you install it, and installing it is your
approval.

## Install

In Sicompass, open store, then programs, and press Enter on install next to
terminal. The Store checks the release's signature before installing it, and
keeps it up to date.

## Building from source

```bash
nix develop          # the toolchain, with the wasm32-wasip2 target
cargo test           # natively, with real shells on a PTY
cargo build --release --target wasm32-wasip2
cp target/wasm32-wasip2/release/terminal_plugin.wasm plugin.wasm
```

`./scripts/release-plugin.sh --dry-run` does the build, checks the component
against `plugin.json`, and signs and verifies it with a throwaway key, the way
a release is made.

## Related repositories

- [sicompass](https://github.com/friendlyflow/sicompass), the application
- [sicompass-plugin-sdk](https://github.com/friendlyflow/sicompass-plugin-sdk),
  the SDK, the WASM plugin kit and the cloud backup library

## Community

Join the conversation on
[Discord](https://discord.com/channels/1464152138753249313/1464152139231137894).

## License

#### Open source license

If you are creating an open source application under a license compatible with
the GNU GPL license v3, you may use this project under the terms of the GPLv3.
See [LICENSE](LICENSE).

## Contributing

Contributions are welcome. Whether it is code, documentation, or feedback, your
input helps make computing more accessible for everyone.
