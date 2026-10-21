# sun-control

A terminal UI for managing [Sunshine](https://github.com/LizardByte/Sunshine) game streaming servers, made for headless machines and SSH sessions. It covers what Sunshine's web admin page does: apps, pairing, configuration, logs and troubleshooting. It manages the local Sunshine by default and can connect to Sunshine on other machines.

## Install

On Linux (x86_64 or aarch64), the installer downloads a static binary, verifies its checksum and puts it in `~/.local/bin`:

```sh
curl -fsSL https://github.com/jordanhchin/sun-control/releases/latest/download/install.sh | sh
```

Update it later with:

```sh
sun-control update           # install the latest release
sun-control update --check   # only check
```

The installer reads these optional settings from the environment: `SUN_CONTROL_INSTALL_DIR` (where to install), `SUN_CONTROL_VERSION` (a release tag such as `v0.2.0`). For a private fork, set `GITHUB_TOKEN` so `sun-control update` can reach the releases; the installer falls back to `gh` if you're logged in.

With Rust installed, you can build it instead, on any platform:

```sh
cargo install --git https://github.com/jordanhchin/sun-control
```

## Use

Run `sun-control` with no arguments to open the TUI. It connects to Sunshine on this machine (`https://localhost:47990`) and asks for the admin login if needed.

| Screen | What it does |
|---|---|
| 1 Dashboard | Version, platform, counts of apps, paired devices and custom settings; restart and force-close |
| 2 Apps | Add, edit, duplicate, delete and reorder apps, including prep and detached commands |
| 3 Pairing | Pair devices waiting with a PIN; enable, disable or unpair devices, with when each device's Moonlight identity was created |
| 4 Config | Every Sunshine setting by section, with descriptions and defaults; review changes before saving |
| 5 Logs | Follow the log live, filter by level, search |
| 6 System | Restart, force-close, reset portal capture or display settings, export logs, change the admin login, check permissions |
| 7 Hosts | Saved Sunshine hosts: add, remove, set the default, re-trust certificates |

Keys: `Tab`/`Shift-Tab` move focus between the sidebar and panes, and arrow keys act on the focused pane. `1`–`7` jump to a screen, `Ctrl-P` or `:` opens a command palette to find any screen or action, and `?` lists every key.

### Command line

Everything important also works without the TUI, for scripts and quick SSH checks:

```sh
sun-control status
sun-control pair 1234 --name "Living room TV"
sun-control clients list
sun-control apps list
sun-control config get min_log_level
sun-control config set min_log_level=3 --restart
sun-control logs -n 50
sun-control permissions
sun-control --help             # everything else
```

## Remote hosts

Save a host once, then use it by name:

```sh
sun-control hosts add den den.lan --username admin --password-cmd "pass show sunshine/den"
sun-control -H den status
sun-control -H den            # TUI
```

`-H` also accepts a one-off address (`box`, `box:47990`, `https://box:47990`). Sunshine's admin UI only accepts LAN connections by default (Config → Network → "Origin Web UI Allowed"). Over the internet, prefer an SSH tunnel: `ssh -L 47990:localhost:47990 box`, then `sun-control -H localhost`.

**Certificates.** Sunshine uses a self-signed certificate. The first time you connect to a remote host, sun-control shows its SHA-256 fingerprint for you to confirm, then pins it. Compare it on the host with:

```sh
openssl x509 -noout -fingerprint -sha256 -in ~/.config/sunshine/credentials/cacert.pem
```

If the certificate later changes (for example after reinstalling Sunshine), connections are refused until you re-trust it with `sun-control hosts trust <name>` or `t` on the Hosts screen.

**Passwords** are never stored. Give a host a `password_cmd` (any command that prints the password), use `SUN_CONTROL_PASSWORD`, or type it when asked.

Hosts live in `~/.config/sun-control/hosts.toml`.

## Newer Sunshine versions

sun-control talks to the same API as Sunshine's web UI and is built to cope with changes:

- Settings it doesn't know appear under **Config → Other**, editable as text, and are always saved back unchanged. The same goes for unknown fields on apps.
- Features missing from an older or newer Sunshine show "not supported by this Sunshine version" instead of failing. Pairing falls back to PIN-only pairing on versions without the waiting-device list.
- The settings list (`src/schema/sunshine.json`) is generated from Sunshine's web UI. Regenerate it after a Sunshine update with:

  ```sh
  python3 -I tools/gen_schema.py /usr/share/sunshine/web > src/schema/sunshine.json
  ```

- Without rebuilding, you can add or override setting definitions in `~/.config/sun-control/schema.json`, using the same format.

## Development

```sh
cargo run              # TUI against local Sunshine
cargo test
cargo clippy --all-targets -- -D warnings
```

To publish a release, set `version` in `Cargo.toml`, commit, then tag it with the matching version:

```sh
git tag v0.2.0 && git push origin v0.2.0
```

The release workflow checks that the tag matches, runs the tests, builds static musl binaries for x86_64 and aarch64, and publishes them with checksums and `install.sh`.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.
