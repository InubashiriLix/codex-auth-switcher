# Codex Switcher

## 多个账号，一个入口

把 Codex 的账号、额度与安全切换，收进一个安静运行在本机的终端控制台。

[![CI](https://github.com/InubashiriLix/codex-auth-switcher/actions/workflows/ci.yml/badge.svg)](https://github.com/InubashiriLix/codex-auth-switcher/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/Rust-2024-dea584?logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/platform-Linux-fcc624?logo=linux&logoColor=black)](#平台支持与跨平台招募)
[![License](https://img.shields.io/badge/license-MIT-2ea44f)](LICENSE)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)

`本地优先` · `流式代理` · `进程 / 会话粘性路由` · `元数据监控`

Codex Switcher 是一个面向 Codex CLI 的本地账号管理器与反向代理。它既能像传统切号工具一样保存、检测和启用认证快照，也能作为后台代理，根据额度、健康状态和会话边界为新请求选择合适的账号。

它不会替你突破限制，也不会把凭据上传到某个陌生服务。它做的事情更朴素：**让你自己获授权的账号更容易管理，让该切换的时候不再靠手工盯着。**

> [!IMPORTANT]
> 当前正式支持并实际验证的平台是 **Linux**。macOS 与 Windows 的基础兼容结构已经预留，但还不能被称为完整支持。我们正在认真寻找愿意一起完成移植和验证的贡献者。

## 先看一眼

<img width="2261" height="1280" alt="c9c233b9aacb14df47fe72a4e25aae18" src="https://github.com/user-attachments/assets/be40333a-842a-4407-8df4-a9c16d4ed39d" />

## 为什么会有这个项目

如果你同时维护几个自己的 Codex 登录，可能已经熟悉这套流程：

- 打开不同认证文件，确认现在到底在用哪个账号；
- 手动检查额度，猜测什么时候应该换号；
- 切换之后重启 Codex，还要担心正在运行的会话；
- 出现 `401`、`429` 或断流时，再回头翻日志找原因。

Codex Switcher 把这些事情合并成两个清晰的工作区：

- **ACCOUNT**：管理认证快照、检查额度、重命名、过滤和手动启用账号；
- **PROXY**：管理代理池、自动路由、活动实例、请求指标与脱敏事件。

```text
┌───────────┐      ┌──────────────────────────────┐      ┌─────────────────┐
│ Codex CLI │ ───▶ │ Codex Switcher · 127.0.0.1  │ ───▶ │ Codex upstream  │
└───────────┘      │                              │      └─────────────────┘
                   │  会话 / 进程粘性             │
                   │  额度与健康状态              │
                   │  安全请求边界切换            │
                   └──────────────┬───────────────┘
                                  │
                         ┌────────▼────────┐
                         │ TUI / Control API│
                         └─────────────────┘
```

已经开始输出的流式响应不会被偷偷迁移到另一个账号。需要切换时，代理会等到下一次尚未返回上游响应的安全请求边界；如果所有账号都不可用，它会快速返回明确错误，而不是把请求无限挂起。

## 它认真处理了什么

### 真正的流式代理

- 请求和 SSE 响应全程流式传输，不等待完整响应才交给 Codex；
- 记录首字节时间、总耗时、状态码和字节数，但不记录正文；
- 响应开始后发生断流，只会标记为 partial failure，绝不会伪装成一次成功重试；
- 仅在请求可安全重放且尚未向 Codex 提交响应时执行有限重试。

### 不打断上下文的路由

- 优先使用可信会话标识，其次使用 PID 与进程启动时间，最后退化到连接标识；
- 已绑定实例保持粘性，新实例才会避开超过阈值的账号；
- 支持 `smart`、`max_remaining` 和 `round_robin` 三种策略；
- `401` 尝试单次刷新，`403` / `429` 进入熔断，网络错误与 `5xx` 不会被误判为额度耗尽。

### 本地优先，而且真的不看你的内容

- Token、Authorization、提示词、代码和模型输出不会进入数据库或 TUI；
- SQLite WAL 只保留请求摘要、指标、切换和健康事件；
- 默认保留 7 天，最多 50,000 条请求摘要和 10,000 条事件；
- 数据代理和带随机 Bearer Token 的控制面均只监听回环地址。

### 可逆的 Codex 接入

- 使用 Codex 用户级配置将请求指向本地代理，不依赖 `HTTPS_PROXY`；
- 修改前创建完整备份，并尽量保留原 TOML 的格式、注释和未知字段；
- 停用时只恢复本工具管理的配置；发现外部修改会拒绝覆盖并展示漂移；
- 启用或停用接入后，需要重启对应的 Codex 进程。

## 安装

目前请在 Linux 上使用。你需要较新的 Rust toolchain，并已安装和登录 Codex CLI。

```bash
git clone https://github.com/InubashiriLix/codex-auth-switcher.git
cd codex-auth-switcher
cargo install --path . --locked
```

安装完成后确认命令可用：

```bash
command -v codex-switcher
codex-switcher --help
```

Cargo 默认把二进制安装到 `~/.cargo/bin`。如果 shell 找不到它，请把该目录加入 `PATH`。

仅在源码目录试运行：

```bash
cargo run --release
```

## 五分钟上手

### 只管理和手动切换账号

```bash
codex-switcher
```

1. 在 `ACCOUNT` 工作区按 `a` 导入当前 Codex 登录，或按 `i` 导入 JSON / 文件路径；
2. 按 `r` 检查当前账号，或按 `R` 检查全部账号；
3. 选中账号后按 `Enter` 启用认证快照；
4. 传统快照切换前请关闭正在运行的 Codex。

### 让代理长期在后台工作

第一次配置建议从普通 TUI 开始：

1. 导入账号并按 `R` 完成额度检测；
2. 按 `m`，再按 `2` 进入 `PROXY` 工作区；
3. 在账户池中用 `j/k` 选择账号，按 `Space` 明确加入代理池；
4. 按 `c` 启用 Codex 接入，按 `a` 明确开启自动切换；
5. 退出 TUI，然后启动无界面守护进程；
6. 重启 Codex，让它读取新的接入配置。

```bash
codex-switcher --daemon
```

守护进程运行后，TUI 可以随时打开或退出，不会影响后台代理：

```bash
codex-switcher                 # 默认打开 ACCOUNT，可附着已有 daemon
codex-switcher daemon-status   # 查看当前状态
codex-switcher daemon-reload   # 热重载安全配置
codex-switcher daemon-stop     # 排空并停止 daemon
```

如果你只想临时体验代理面板：

```bash
codex-switcher --proxy
```

`--proxy` 会启动或附着代理并直接打开 `PROXY` 工作区。它创建的内嵌代理由 TUI 管理，退出时会停止；长期挂机请使用 `--daemon`。

> [!NOTE]
> 至少要有一个“已加入代理池、认证有效、额度检测新鲜且未超过阈值”的账号，数据代理才会启动。旧版本升级后的账号默认不会自动入池。

## TUI 快捷键

| 按键                         | 功能                                        |
| ---------------------------- | ------------------------------------------- |
| `m`，然后 `1` / `2`          | 切换 `ACCOUNT` / `PROXY` 工作区             |
| `Tab` / `Shift-Tab`、`1`–`4` | 循环或直接聚焦代理面板                      |
| `j` / `k`                    | 在当前列表中移动                            |
| `Enter`                      | 查看详情；在 ACCOUNT 中启用所选快照         |
| `Space` / `x`                | 加入或移出代理池 / 在安全边界手动切换       |
| `s` / `p` / `c` / `a`        | 启停代理 / 暂停路由 / Codex 接入 / 自动切换 |
| `a` / `i`（ACCOUNT）         | 导入当前认证 / JSON 或文件路径              |
| `r` / `R`                    | 检查所选 / 全部账号                         |
| `n` / `d`                    | 重命名 / 删除账号                           |
| `t`                          | 切换 Midnight、Nord、Gruvbox、Paper 主题    |
| `/`                          | 按名称或邮箱过滤                            |
| `?` / `q`                    | 帮助 / 退出                                 |

## systemd 用户服务

仓库提供了一个 Linux `systemd --user` 服务模板：

```bash
mkdir -p ~/.config/systemd/user
cp codex-switcher.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now codex-switcher.service
```

配置位于 `$XDG_CONFIG_HOME/codex-switcher/config.toml`，默认回退到
`~/.config/codex-switcher/config.toml`。账号快照和运行数据位于
`$XDG_DATA_HOME/codex-switcher/`，默认回退到
`~/.local/share/codex-switcher/`。

## 平台支持与跨平台招募

| 平台    | 当前状态              | 我们需要什么                                                |
| ------- | --------------------- | ----------------------------------------------------------- |
| Linux   | ✅ 正式支持并实际验证 | Bug 报告、发行打包、桌面环境兼容性                          |
| macOS   | 🛠️ 尚未正式支持       | 进程识别、launchd、通知、权限与真实冒烟测试                 |
| Windows | 🛠️ 尚未正式支持       | 进程识别、Windows Service、通知、原子文件操作与真实冒烟测试 |

### 这不是一句客套话：我们真的需要 macOS / Windows 贡献者

你不必一次完成整个平台。下面任何一项都可以成为很有价值、范围清晰的 PR：

- 验证并修复 TCP 源端口到 PID、PID 到 cwd 的映射；
- 为 launchd 或 Windows Service 提供可靠的 daemon 生命周期；
- 验证私有文件权限、原子替换和系统通知；
- 增加平台专属测试、安装说明或发行包；
- 在真实 Codex 环境完成一次脱敏冒烟测试并记录差异。

如果你每天使用 macOS 或 Windows，你拥有这个项目目前最缺少的东西：
**真实的平台环境和对“怎样才算好用”的判断。** 欢迎先开 Issue
描述你准备认领的部分，也欢迎直接提交一个小而扎实的 PR。

## 参与贡献

不同规模的贡献都很欢迎：

- **第一次参与开源**：中文文案、错误提示、主题、文档和测试；
- **熟悉 Rust**：流式边界、路由策略、指标、TUI 组件和安全审计；
- **愿意维护一个平台**：macOS / Windows 支持、安装体验与持续验证。

开始之前请阅读 [CONTRIBUTING.md](CONTRIBUTING.md)，也可以使用仓库现有的
[Bug 报告](https://github.com/InubashiriLix/codex-auth-switcher/issues/new?template=bug_report.yml)
或
[功能建议](https://github.com/InubashiriLix/codex-auth-switcher/issues/new?template=feature_request.yml)
模板。

提交截图、日志或测试样本前，请务必移除 Token、Authorization、真实邮箱、主目录、提示词、代码与模型输出。

## 使用边界

本项目只适用于你本人拥有或明确获授权使用的账号。它不绕过服务限制、
不提供共享凭据，也不承诺任何账号可以持续可用。OAuth 刷新属于对当前
Codex 行为的兼容层；失败时，账号会被隔离并提示重新登录，而不会阻断其他
健康账号。

## License

[MIT](LICENSE) © 2026 InubashiriLix

## 如果它替你省下了一次手动切号，请留下一颗 ⭐

如果你希望它出现在自己的平台上，欢迎带着一个小 PR 来敲门。
