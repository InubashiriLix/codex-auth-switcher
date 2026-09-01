# Codex Switcher

[English](README.md) · [简体中文](README.zh-CN.md) · [日本語](README.ja.md) · [Español](README.es.md) · [Deutsch](README.de.md) · [Italiano](README.it.md) · [Français](README.fr.md)

一个入口管理多个 Codex 账户。Codex Switcher 可以保存认证快照、检测额度健康度、安全切换账户，并通过只监听回环地址的流式代理保持活动会话稳定。

## 功能

- 导入、重命名、检查、启用和删除本地认证快照。
- 检查主要及次要额度窗口，不显示或保存认证信息。
- 将账户明确加入本地代理池，并按粘性身份路由请求。
- 只在安全请求边界切换，且不会覆盖无关的 Codex 配置。
- 仅保存脱敏元数据；提示词、代码、响应、Token 和认证头不会进入数据库。
- 支持普通 TUI、附着守护进程和长期运行的 systemd 用户服务。
- 内置本地 Doctor、账户故障时间线和脱敏支持包导出。
- 支持中文、英语、日语、西班牙语、德语、意大利语和法语。

## 安装与使用

```bash
cargo build --release
install -Dm755 target/release/codex-switcher ~/.local/bin/codex-switcher
codex-switcher
```

在账户界面按 `a` 导入当前 Codex 登录，按 `r` 检测，按 `Enter` 启用所选快照。运行代理界面：

```bash
codex-switcher --proxy
```

启动代理前，至少需要一个刚检测为健康并通过 `Space` 明确加入代理池的账户。后台运行可使用：

```bash
codex-switcher --daemon
codex-switcher daemon-status
codex-switcher daemon-reload
codex-switcher daemon-stop
```

## 可靠性工具

```bash
# 默认离线：检查配置、快照、接入状态、运行时数据库和代理端口。
codex-switcher doctor
# 只有显式指定才进行最小 DNS/TLS/上游连通性预检。
codex-switcher doctor --network --json

# 你自行完成官方 Codex 登录后，用当前登录更新原账户快照。
codex-switcher account update-current ACCOUNT_ID
codex-switcher account events ACCOUNT_ID --json

# 仅导出脱敏元数据、配置和诊断结果。
codex-switcher support-bundle --output ./codex-switcher-support.tar.gz
```

健康账户会保持高频探测；失败账户使用有上限的指数退避与抖动，真实代理请求会立即更新健康状态。

## 语言

在主界面或详情页按 `l`，选择“跟随系统”或七种内置语言之一，再按 `Enter` 保存。配置会原子写入并在重启后保留；`Esc` 取消修改。

```toml
# auto、zh-cn、en、ja、es、de、it、fr
language = "auto"
```

自动检测依次读取 `LC_ALL`、`LC_MESSAGES` 和 `LANG`，不支持的语言回退英语。页眉会显示当前实际语言，例如 `🌐 [l] 中文`。

## 常用快捷键

| 按键 | 功能 |
| --- | --- |
| `l` | 打开语言选择器 |
| `m`，然后 `1` / `2` | 切换 ACCOUNT / PROXY 工作区 |
| `j` / `k` | 移动选择 |
| `Enter` | 查看详情或启用所选快照 |
| `Space` / `x` | 加入或移出代理池 / 请求安全切换 |
| `a` / `i` | 导入当前认证 / JSON 或文件路径 |
| `u` | 用当前官方登录更新所选账户快照 |
| `r` / `R` | 检测所选 / 全部账户 |
| `t` | 切换主题 |
| `?` / `q` | 帮助 / 退出 |

Linux 配置位于 `$XDG_CONFIG_HOME/codex-switcher/config.toml`，账户和运行数据位于 `$XDG_DATA_HOME/codex-switcher/`。Windows 10/11 x86_64 使用 `%APPDATA%\CodexSwitcher` 与 `%LOCALAPPDATA%\CodexSwitcher`；可使用未签名 MSI 和 LocalService 服务。

## 设备身份收敛

代理默认复用 `codex_home/installation_id`；该文件不存在或无效时，才从本机稳定种子派生 UUID。它会在所有上游请求中统一 `x-codex-installation-id`、`x-codex-turn-metadata.installation_id` 和请求体 `client_metadata` 中的同名投影。它不会改写 `session_id`、`thread_id`、`window_id` 或 `prompt_cache_key`，因此不会把不同对话错误合并。zstd 压缩请求也会先解压、改写再重新压缩。这只是本机一致性控制，不能用于伪装不同用户或设备。

```toml
[proxy.device_identity]
enabled = true
# 可选：仅在迁移同一台机器时保留既有身份。
installation_id = "00000000-0000-0000-0000-000000000000"

[proxy.auth_policy]
refresh_before_expiry_seconds = 300
refresh_once_on_401 = true
cross_account_replay = false
rate_limit_fallback_seconds = 900
```

未填写 `installation_id` 时，当前 Codex 主目录及同一设备会自动获得稳定 ID。认证失败会在原账户内处理：仅明确的 `token_expired` 允许刷新一次；其余 401、403 以及额度类 429 会隔离账户，原请求不会切换到另一账户重放。临时 429 优先按 `Retry-After` 恢复。

## 安全边界与贡献

本项目仅适用于你拥有或获授权使用的账户，不绕过服务限制。代理仅监听本地回环地址，历史记录只包含脱敏元数据。分享截图和日志前请移除 Token、认证头、邮箱、主目录、提示词、代码和模型输出。

提交前运行 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test --all-targets` 和 release build。七种内置翻译位于 `locales/`，新增或修改文案时必须保持消息键和变量一致。

## License

MIT
