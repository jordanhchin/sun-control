# sun-control

A modern terminal UI for managing [Sunshine](https://github.com/LizardByte/Sunshine) game streaming servers — locally or on remote/headless hosts over SSH.

> **Status:** early development.

## Features (planned)

- Dashboard, apps, pairing/clients, full configuration editor, logs, troubleshooting
- Manages the local instance by default; connect to remote hosts via their admin API
- Schema-driven config editor that tolerates new/unknown Sunshine options
- Scriptable subcommands (`pin`, `restart`, `apps list`, `config get/set`)

## Building

```sh
cargo build --release
./target/release/sun-control
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
