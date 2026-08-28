# Codex Switcher

> A fast, local-first TUI for managing multiple Codex CLI authentication snapshots, with intelligent proxy mode for automatic token management.

[![Rust](https://img.shields.io/badge/built_with-Rust-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Codex Switcher lets you keep several **your own** Codex CLI logins locally, inspect quota windows, and activate a snapshot without copying tokens by hand. **NEW**: Proxy mode enables automatic token monitoring and smart account switching for uninterrupted Codex CLI usage.

## ✨ New: Proxy Mode

**Intelligent proxy between Codex CLI and ChatGPT API:**
- 🔍 **Auto-monitoring**: Checks token usage every 30 seconds
- 🤖 **Smart recommendations**: Three strategies (Smart, MaxRemaining, RoundRobin)
- 🔄 **Hot reload**: Update accounts/config without restarting
- 🎯 **Zero-downtime**: Graceful connection handling
- 📊 **Statistics**: Track requests, failures, auto-switches
- 🔧 **Systemd ready**: Run as a system service

## Why it is useful

- **One keyboard-driven workflow** — import, inspect, rename, test, and activate accounts without leaving the terminal.
- **Quota visibility** — primary and secondary windows show remaining capacity, window length, and reset time.
- **Safe by default** — local snapshots, atomic writes, private permissions, symlink checks, and refusal to switch while Codex is running.
- **Readable anywhere** — four built-in themes, including a high-contrast light theme; press `t` to switch instantly.
- **No credential service** — tokens are not uploaded, logged, or displayed by this tool.

> This project is for accounts you are authorized to use. It does not bypass limits or provide shared credentials. Accounts never join the proxy pool until you explicitly opt them in.

## Screenshots

<!-- Add a repository-hosted, redacted screenshot here once published. Never commit live emails, home paths, tokens, or shell prompts. -->

## 安装 / Install

需要较新的 Rust toolchain，以及本机已安装的 Codex CLI。Requires a recent Rust toolchain and a local Codex CLI installation.

```bash
cargo install --path . --locked
```

Cargo installs the binary to `~/.cargo/bin`。如果提示找不到命令，请将其加入 `PATH`：

```bash
export PATH="$HOME/.cargo/bin:$PATH"
```

将上一行加入 `~/.bashrc`、`~/.zshrc` 或 Fish 配置，重新打开终端后验证：

```bash
command -v codex-switcher
codex-switcher
```

For a checkout-only run / 仅试运行：`cargo run --release`。

## Quick start / 快速开始

### Traditional TUI Mode / 传统TUI模式
1. Close running Codex sessions / 关闭正在运行的 Codex。
2. Press `a` to import the current login, or `i` to import JSON / 按 `a` 导入当前登录，或按 `i` 导入 JSON。
3. Press `r` or `R` to check one or all accounts / 按 `r` 或 `R` 检测额度。
4. Select an account and press `Enter` to activate it / 选择账户后按 `Enter` 启用。

### 🆕 Proxy Mode / 代理模式
```bash
# Default launch opens ACCOUNT. Press m, then 2 for the PROXY workspace.
codex-switcher

# Open PROXY directly and start/attach the data plane
codex-switcher --proxy

# Or start a headless daemon
codex-switcher --daemon

# In PROXY: Space manages the pool, s starts/stops, c configures Codex.
# This safely writes a model_provider to $CODEX_HOME/config.toml.
# Restart Codex after enabling or disabling it; HTTPS_PROXY is not used.

# Check status
codex-switcher daemon-status

# Hot reload config
codex-switcher daemon-reload

# Stop daemon
codex-switcher daemon-stop
```

**Systemd service:**
```bash
cp codex-switcher.service ~/.config/systemd/user/
systemctl --user enable codex-switcher.service
systemctl --user start codex-switcher.service
```

Snapshots live under `$XDG_DATA_HOME/codex-switcher/` (fallback `~/.local/share/codex-switcher/`). Configuration lives under `$XDG_CONFIG_HOME/codex-switcher/config.toml` (fallback `~/.config/codex-switcher/`). Press `s` to change the Codex home directory.

## Keymap / 快捷键

| Key | Action |
| --- | --- |
| `m`, then `1` / `2` | Switch between the ACCOUNT and PROXY workspaces |
| `Tab` / `Shift-Tab`, `1`–`4` | Cycle or directly focus proxy dashboard panels |
| `j` / `k` | Move inside the focused list (Vim navigation; arrow keys are not required) |
| `Space` / `x` | Add/remove a proxy-pool account / switch at a safe request boundary |
| `s` / `p` / `c` / `a` in PROXY | Start-stop proxy / pause / Codex integration / auto-switch |
| `a` / `i` in ACCOUNT | Import current auth / JSON or path · 导入认证 |
| `r` / `R` | Check selected / all accounts · 检测额度 |
| `Enter` | Activate selected snapshot · 启用账户 |
| `n` / `d` | Rename / delete · 重命名 / 删除 |
| `t` | Cycle `midnight`, `nord`, `gruvbox`, `paper` themes · 切换主题 |
| `/` | Filter by label or email · 过滤 |
| `?` / `q` | Help / quit · 帮助 / 退出 |

Checks run in the background with a progress indicator. When an email has been discovered, `n` first asks whether to use it as the account name; choose `n` to enter a custom label.

## Themes / 主题

`midnight` is the default high-contrast dark theme. The selection is persisted to `config.toml`; older configurations automatically use `midnight`。默认主题为高对比深色 `midnight`，按 `t` 即时切换。

## Security / 安全

Snapshots and daemon runtime descriptors use private permissions and atomic replacement. The data plane and authenticated control plane bind only to loopback in this release. Monitoring stores metadata summaries in SQLite WAL, never prompts, code, model output, request/response bodies, authorization headers, or tokens. Traditional snapshot activation is blocked while Codex is running; proxy routing changes only at a request boundary and never migrates an SSE stream. Use only accounts and credentials you are authorized to manage. 请勿提交 token、邮箱、路径或真实账户快照。

## Contributing / 参与贡献

Bug reports, theme ideas, documentation fixes, and pull requests are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) before opening a change.

## License / 许可证

MIT — see [LICENSE](LICENSE)。

<div align="center">If this saves you a few context switches, consider leaving a ⭐。</div>
