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
- Run a local Doctor, inspect account fault timelines, and export a sanitized support bundle.
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

## Reliability tools

```bash
# Offline by default: checks configuration, snapshots, integration, runtime DB and proxy port.
codex-switcher doctor
# Explicitly add a minimal DNS/TLS/upstream reachability check.
codex-switcher doctor --network --json

# After you complete the official Codex login yourself, update the affected snapshot.
codex-switcher account update-current ACCOUNT_ID
codex-switcher account events ACCOUNT_ID --json

# The archive contains only sanitized metadata, configuration and diagnosis.
codex-switcher support-bundle --output ./codex-switcher-support.tar.gz
```

The monitor keeps healthy accounts fresh at a high cadence. Failed accounts are retried with a bounded exponential backoff and jitter; real proxy requests update health immediately.

## Language

Press `l` from the main screen or a detail screen. Choose “Follow system” or one of the seven built-in languages, then press `Enter`. The choice is written atomically and survives restarts; `Esc` cancels without changing the configuration.

```toml
# auto, zh-cn, en, ja, es, de, it, fr
language = "auto"
```

Automatic detection checks `LC_ALL`, `LC_MESSAGES`, then `LANG`. Unsupported locales fall back to English. The header always shows the effective language as `🌐 [l] English`.

### Legacy and ASCII terminals

The TUI normally uses Unicode box drawing and high-resolution charts. It automatically switches to ASCII when `TERM=dumb` or the active locale is non-UTF-8. Override that choice persistently with `terminal_mode = "auto" | "unicode" | "ascii"`, or for one invocation use:

```bash
codex-switcher --force-ascii
codex-switcher --proxy --force-ascii
```

ASCII mode uses English and emits only ASCII characters, including live charts, so it remains usable over constrained TTY and serial connections.

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
systemctl --user status codex-switcher.service
journalctl --user -u codex-switcher.service -e
```

## Windows (x86_64)

Windows 10/11 x86_64 is supported through the portable `codex-switcher.exe` and the unsigned MSI release. Interactive state uses `%APPDATA%\CodexSwitcher` and `%LOCALAPPDATA%\CodexSwitcher`; the default Codex directory is `%USERPROFILE%\.codex`.

The MSI installs the executable, registers a demand-start LocalService daemon, and prepares `C:\ProgramData\CodexSwitcher`. To configure and start that daemon, open an elevated PowerShell and provide the exact Codex directory it may access:

```powershell
& 'C:\Program Files\Codex Switcher\codex-switcher.exe' service install `
  --service-root 'C:\ProgramData\CodexSwitcher' `
  --codex-home "$env:USERPROFILE\.codex"
```

Use `service status`, `service stop`, and `service uninstall` to manage it. The service cannot display desktop notifications; switch events remain available through the TUI and runtime history. The installer is not Authenticode-signed yet, so Windows will show an unknown-publisher warning.

## Device identity convergence

The proxy reuses `codex_home/installation_id` by default and derives a UUID from a stable local-machine seed only when that file is missing or invalid. It consistently rewrites `x-codex-installation-id`, `x-codex-turn-metadata.installation_id`, and the matching `client_metadata` projection. It deliberately preserves `session_id`, `thread_id`, `window_id`, and `prompt_cache_key`, so separate conversations do not collapse into one session. Zstd-compressed requests are decoded, normalized, and recompressed. This is local consistency only; it does not impersonate other users or devices.

```toml
[proxy.device_identity]
enabled = true
# Optional: retain an existing identity while migrating this same machine.
installation_id = "00000000-0000-0000-0000-000000000000"

[proxy.auth_policy]
refresh_before_expiry_seconds = 300
refresh_once_on_401 = true
cross_account_replay = false
rate_limit_fallback_seconds = 900
```

Without an explicit `installation_id`, the configured Codex home and the same machine retain a stable ID automatically. `user_agent` and `originator` always retain the client values; legacy override fields are ignored.

## Security boundary

Use Codex Switcher only with accounts you own or are authorized to use. It does not bypass service limits. The proxy listens on loopback only, configuration changes are reversible, and stored request history contains sanitized metadata rather than request or response bodies.

Before sharing screenshots, logs, or test data, remove tokens, authorization values, real email addresses, home-directory paths, prompts, code, and model output.

## Contributing

Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all-targets`, and a release build before submitting changes. Translation fixes are welcome; the seven built-in catalogs are in `locales/` and must keep matching message keys and variables.

## License

MIT
