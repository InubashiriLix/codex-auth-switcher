# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

One local entry point for multiple Codex accounts. Codex Switcher stores authentication snapshots, checks quota health, switches accounts safely, and can run a loopback streaming proxy that preserves active sessions.

## Features

- Import, rename, inspect, activate, and remove local authentication snapshots.
- Check primary and secondary quota windows without exposing credentials.
- Opt accounts into a local proxy pool and route requests with sticky identities.
- Switch only at safe request boundaries and preserve unrelated Codex configuration.
- Keep metadata-only history; prompts, source code, responses, tokens, and authorization headers are never stored.
- Use the TUI, an attached daemon, or a long-running systemd user service.
- Display the application in Chinese, English, Japanese, Spanish, German, Italian, or French.

## Install

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
```

The program reads `$XDG_CONFIG_HOME/codex-switcher/config.toml` and falls back to `~/.config/codex-switcher/config.toml`. Account snapshots and runtime metadata live under `$XDG_DATA_HOME/codex-switcher/`.

## Quick start

Run the account manager:

```bash
codex-switcher
```

Press `a` to import the current Codex login, `r` to check it, and `Enter` to activate the selected snapshot.

Run or attach to the proxy workspace:

```bash
codex-switcher --proxy
```

At least one recently checked, healthy account must be explicitly added to the pool with `Space` before the data proxy can start.

Run a background daemon:

```bash
codex-switcher --daemon
codex-switcher daemon-status
codex-switcher daemon-reload
codex-switcher daemon-stop
```

## Language

Press `l` from the main screen or a detail screen. Choose “Follow system” or one of the seven built-in languages, then press `Enter`. The choice is written atomically and survives restarts; `Esc` cancels without changing the configuration.

```toml
# auto, zh-cn, en, ja, es, de, it, fr
language = "auto"
```

Automatic detection checks `LC_ALL`, `LC_MESSAGES`, then `LANG`. Unsupported locales fall back to English. The header always shows the effective language as `🌐 [l] English`.

## TUI keys

| Key                          | Action                                                          |
| ---------------------------- | --------------------------------------------------------------- |
| `l`                          | Open the language selector                                      |
| `m`, then `1` / `2`          | Switch between ACCOUNT and PROXY workspaces                     |
| `Tab` / `Shift-Tab`, `1`–`4` | Cycle or focus proxy panels                                     |
| `j` / `k`                    | Move in the current list                                        |
| `Enter`                      | Open details or activate the selected snapshot                  |
| `Space` / `x`                | Toggle proxy-pool membership / request a safe route change      |
| `s` / `p` / `c` / `a`        | Start or stop / pause / Codex integration / automatic switching |
| `a` / `i` in ACCOUNT         | Import current authentication / JSON or file path               |
| `r` / `R`                    | Check the selected account / all accounts                       |
| `n` / `d`                    | Rename / delete an account                                      |
| `t`                          | Cycle the Midnight, Nord, Gruvbox, and Paper themes             |
| `/`                          | Filter by name or email                                         |
| `?` / `q`                    | Help / quit                                                     |

## systemd user service

```bash
mkdir -p ~/.config/systemd/user
cp codex-switcher.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now codex-switcher.service
```

Linux is the currently verified platform. macOS and Windows code paths exist but still need real-world testing and platform-specific service integration.

## Security boundary

Use Codex Switcher only with accounts you own or are authorized to use. It does not bypass service limits. The proxy listens on loopback only, configuration changes are reversible, and stored request history contains sanitized metadata rather than request or response bodies.

Before sharing screenshots, logs, or test data, remove tokens, authorization values, real email addresses, home-directory paths, prompts, code, and model output.

## Contributing

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, and a release build before submitting changes. Translation fixes are welcome; the seven built-in catalogs are in `locales/` and must keep matching message keys and variables.

## License

MIT
