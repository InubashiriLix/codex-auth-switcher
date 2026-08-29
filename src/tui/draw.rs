use super::{ConfirmChoice, DetailPage, HelpPage, Modal, ProxyPanel, Ui, Workspace};
use crate::{
    proxy::RuntimeState,
    types::{Quota, StatusKind},
};
use chrono::Local;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, Borders, Clear, Gauge, List, ListItem, ListState, Paragraph, Sparkline, Wrap,
    },
};
use unicode_width::UnicodeWidthStr;

pub fn draw(frame: &mut Frame, ui: &Ui) {
    let theme = ui.config.theme.colors();
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        frame.area(),
    );
    let root = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(6),
        Constraint::Length(2),
    ])
    .split(frame.area());

    draw_header(frame, root[0], ui, theme);
    if let Some(detail) = &ui.detail {
        draw_detail(frame, root[1], ui, detail, theme);
    } else {
        match ui.workspace {
            Workspace::Accounts => draw_accounts(frame, root[1], ui, theme),
            Workspace::Proxy => draw_proxy(frame, root[1], ui, theme),
        }
    }
    draw_footer(frame, root[2], ui, theme);
    if ui.modal != Modal::None {
        draw_modal(frame, ui, theme);
    }
}

fn draw_header(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let daemon = if ui.attached_daemon {
        Span::styled(" ● daemon", Style::default().fg(theme.success))
    } else {
        Span::styled(" ○ daemon", Style::default().fg(theme.muted))
    };
    let badge_color = if ui.workspace == Workspace::Proxy {
        theme.warning
    } else {
        theme.focus
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " CODEX SWITCHER ",
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" {} ", ui.workspace.badge()),
                Style::default()
                    .fg(theme.background)
                    .bg(badge_color)
                    .add_modifier(Modifier::BOLD),
            ),
            daemon,
            Span::styled(
                format!("   {}   ", ui.config.theme.name()),
                Style::default().fg(theme.muted),
            ),
            Span::styled("[m] 切换工作区", Style::default().fg(theme.focus)),
        ]))
        .style(Style::default().bg(theme.surface))
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .style(Style::default().fg(theme.border).bg(theme.surface)),
        ),
        area,
    );
}

fn draw_footer(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let keys = match ui.workspace {
        Workspace::Accounts => " a 导入  r/R 检测  Enter 启用  / 过滤  m 工作区  ? 帮助",
        Workspace::Proxy => " Tab/1-4 面板  j/k 选择  Enter 详情  s 启停  p 暂停  c 接入  ? 帮助",
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(keys, Style::default().fg(theme.muted)),
            Span::styled(
                format!("  │  {}", ui.notice),
                Style::default().fg(theme.text),
            ),
        ]))
        .style(Style::default().bg(theme.background))
        .block(
            Block::default()
                .borders(Borders::TOP)
                .style(Style::default().fg(theme.border)),
        ),
        area,
    );
}

fn draw_accounts(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let rows = Layout::vertical([Constraint::Length(6), Constraint::Min(6)]).split(area);
    let cards = Layout::horizontal([
        Constraint::Ratio(1, 4),
        Constraint::Ratio(1, 4),
        Constraint::Ratio(1, 4),
        Constraint::Ratio(1, 4),
    ])
    .split(rows[0]);
    let live = ui
        .index
        .accounts
        .iter()
        .filter(|account| account.status.kind == StatusKind::Live)
        .count();
    let active = active_account_id(ui)
        .and_then(|id| ui.index.accounts.iter().find(|account| account.id == id))
        .map(|account| account.label.as_str())
        .unwrap_or("未设置");
    metric_card(
        frame,
        cards[0],
        "账户",
        &ui.index.accounts.len().to_string(),
        "已保存",
        theme.focus,
        theme,
    );
    metric_card(
        frame,
        cards[1],
        "健康",
        &live.to_string(),
        "可直接使用",
        theme.success,
        theme,
    );
    metric_card(
        frame,
        cards[2],
        "待处理",
        &ui.index.accounts.len().saturating_sub(live).to_string(),
        "需要检测或登录",
        if live == ui.index.accounts.len() {
            theme.muted
        } else {
            theme.warning
        },
        theme,
    );
    metric_card(
        frame,
        cards[3],
        "当前直连",
        active,
        "auth.json",
        theme.focus,
        theme,
    );

    let body =
        Layout::horizontal([Constraint::Percentage(42), Constraint::Percentage(58)]).split(rows[1]);
    draw_account_list(frame, body[0], ui, theme);
    draw_account_summary(frame, body[1], ui, theme);
}

fn draw_account_list(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let active = active_account_id(ui);
    let visible = ui.visible();
    let items = visible
        .iter()
        .enumerate()
        .map(|(position, index)| {
            let account = &ui.index.accounts[*index];
            let (color, status) = status_style(theme, &account.status.kind);
            let direct = if active == Some(account.id) {
                "  ● 直连"
            } else {
                ""
            };
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        if position == ui.selected {
                            "› "
                        } else {
                            "  "
                        },
                        Style::default().fg(theme.focus),
                    ),
                    Span::styled(
                        account.label.chars().take(28).collect::<String>(),
                        Style::default()
                            .fg(if position == ui.selected {
                                theme.selected_text
                            } else {
                                theme.text
                            })
                            .add_modifier(if position == ui.selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                    Span::styled(direct, Style::default().fg(theme.focus)),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(status, Style::default().fg(color)),
                    Span::styled(
                        format!("  {}", account.email.as_deref().unwrap_or("邮箱未知")),
                        Style::default().fg(theme.muted),
                    ),
                ]),
            ])
            .style(Style::default().bg(if position == ui.selected {
                theme.selected_bg
            } else {
                theme.surface
            }))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(panel_block(
            theme,
            if ui.filter.is_empty() {
                "账户列表"
            } else {
                "过滤结果"
            },
            true,
        )),
        area,
    );
}

fn draw_account_summary(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let Some(index) = ui.selected_id() else {
        frame.render_widget(
            Paragraph::new("暂无账户\n\n按 a 导入当前 Codex 登录，或按 i 导入 JSON/路径。")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted).bg(theme.surface))
                .block(panel_block(theme, "账户详情", false)),
            area,
        );
        return;
    };
    let account = &ui.index.accounts[index];
    let parts = Layout::vertical([
        Constraint::Length(7),
        Constraint::Length(6),
        Constraint::Length(6),
        Constraint::Min(4),
    ])
    .split(area);
    let (color, status) = status_style(theme, &account.status.kind);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                &account.label,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(status, Style::default().fg(color)),
                Span::styled(
                    format!("   {}", account.plan.as_deref().unwrap_or("套餐未知")),
                    Style::default().fg(theme.muted),
                ),
            ]),
            Line::from(format!(
                "邮箱  {}",
                account.email.as_deref().unwrap_or("未知")
            )),
            Line::from(format!("来源  {}", account.source)),
        ])
        .style(Style::default().fg(theme.text).bg(theme.surface))
        .block(panel_block(theme, "身份与状态", false)),
        parts[0],
    );
    draw_quota(
        frame,
        parts[1],
        "主要额度窗口",
        account.status.primary.as_ref(),
        theme,
    );
    draw_quota(
        frame,
        parts[2],
        "次要额度窗口",
        account.status.secondary.as_ref(),
        theme,
    );
    let checked = account
        .status
        .checked_at
        .map(|time| {
            time.with_timezone(&Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|| "从未检测".into());
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("最近检测  {checked}")),
            Line::from(format!("状态说明  {}", account.status.detail)),
        ])
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.muted).bg(theme.surface))
        .block(panel_block(theme, "诊断", false)),
        parts[3],
    );
}

fn draw_proxy(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    if area.width < 120 {
        let rows = Layout::vertical([
            Constraint::Length(3),
            Constraint::Length(2),
            Constraint::Min(4),
        ])
        .split(area);
        draw_compact_proxy_status(frame, rows[0], ui, theme);
        draw_alert_strip(frame, rows[1], ui, theme);
        match ui.proxy_panel {
            ProxyPanel::Control => draw_control(frame, rows[2], ui, true, theme),
            ProxyPanel::Pool => draw_pool(frame, rows[2], ui, true, theme),
            ProxyPanel::Instances => draw_instances(frame, rows[2], ui, true, theme),
            ProxyPanel::Events => draw_events(frame, rows[2], ui, true, theme),
        }
        return;
    }
    let rows = Layout::vertical([Constraint::Length(7), Constraint::Min(8)]).split(area);
    draw_proxy_metrics(frame, rows[0], ui, theme);
    let columns = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(36),
        Constraint::Percentage(34),
    ])
    .split(rows[1]);
    draw_control(
        frame,
        columns[0],
        ui,
        ui.proxy_panel == ProxyPanel::Control,
        theme,
    );
    draw_pool(
        frame,
        columns[1],
        ui,
        ui.proxy_panel == ProxyPanel::Pool,
        theme,
    );
    let right = Layout::vertical([Constraint::Percentage(46), Constraint::Percentage(54)])
        .split(columns[2]);
    draw_instances(
        frame,
        right[0],
        ui,
        ui.proxy_panel == ProxyPanel::Instances,
        theme,
    );
    draw_events(
        frame,
        right[1],
        ui,
        ui.proxy_panel == ProxyPanel::Events,
        theme,
    );
}

fn draw_compact_proxy_status(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let (runtime, color) = runtime_label(&ui.runtime_state(), theme);
    let active = ui
        .snapshot
        .as_ref()
        .map_or(0, |snapshot| snapshot.active_requests.len());
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                format!(" {runtime} "),
                Style::default()
                    .fg(theme.background)
                    .bg(color)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    "  接入 {}  健康池 {}  活动 {}  ",
                    if ui.integration_enabled() {
                        "ON"
                    } else {
                        "OFF"
                    },
                    ui.eligible_accounts(),
                    active
                ),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("面板 {}", ui.proxy_panel.number()),
                Style::default().fg(theme.focus),
            ),
        ]))
        .style(Style::default().bg(theme.surface))
        .block(panel_block(theme, "代理状态", false)),
        area,
    );
}

fn draw_alert_strip(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let alert = ui
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.alerts.first());
    let (text, color) = alert.map_or_else(
        || (" ✓ 当前无告警".to_string(), theme.success),
        |alert| {
            (
                format!(" ! {} · {}", alert.title, alert.detail),
                theme.error,
            )
        },
    );
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(color).bg(theme.background)),
        area,
    );
}

fn draw_proxy_metrics(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let columns = Layout::horizontal([
        Constraint::Ratio(1, 6),
        Constraint::Ratio(1, 6),
        Constraint::Ratio(1, 6),
        Constraint::Ratio(1, 6),
        Constraint::Ratio(1, 6),
        Constraint::Ratio(1, 6),
    ])
    .split(area);
    let stats = ui.snapshot.as_ref().map(|snapshot| &snapshot.stats);
    let runtime = ui.runtime_state();
    let (runtime_text, runtime_color) = runtime_label(&runtime, theme);
    metric_card(
        frame,
        columns[0],
        "数据代理",
        runtime_text,
        &ui.config.proxy.listen_addr,
        runtime_color,
        theme,
    );
    metric_card(
        frame,
        columns[1],
        "Codex 接入",
        if ui.integration_enabled() {
            "ON"
        } else {
            "OFF"
        },
        if ui.config.proxy.auto_switch {
            "provider · AUTO ON"
        } else {
            "provider · AUTO OFF"
        },
        if ui.integration_enabled() {
            theme.success
        } else {
            theme.warning
        },
        theme,
    );
    metric_card(
        frame,
        columns[2],
        "健康池",
        &ui.eligible_accounts().to_string(),
        &format!(
            "可路由 · {} 活动请求",
            ui.snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.active_requests.len())
        ),
        if ui.eligible_accounts() > 0 {
            theme.success
        } else {
            theme.error
        },
        theme,
    );
    metric_spark(
        frame,
        columns[3],
        MetricSparkSpec {
            title: "RPS · 1 min",
            value: format!(
                "{:.2} req/s",
                ui.metrics_1m.as_ref().map_or(0.0, |metrics| metrics.rps)
            ),
            divisor: 1000.0,
            minimum_max: 1000,
            color: theme.focus,
        },
        &ui.request_history,
        theme,
    );
    let success = ui.metrics.as_ref().map_or_else(
        || {
            stats.map_or(100.0, |stats| {
                if stats.total_requests == 0 {
                    100.0
                } else {
                    100.0 * (stats.total_requests.saturating_sub(stats.failed_requests)) as f64
                        / stats.total_requests as f64
                }
            })
        },
        |metrics| metrics.success_rate,
    );
    metric_card(
        frame,
        columns[4],
        "成功率",
        &format!("{success:.1}%"),
        "累计",
        if success >= 95.0 {
            theme.success
        } else {
            theme.error
        },
        theme,
    );
    metric_spark(
        frame,
        columns[5],
        MetricSparkSpec {
            title: "TTFB p95",
            value: format!(
                "{} ms",
                ui.metrics
                    .as_ref()
                    .and_then(|metrics| metrics.ttfb_p95_ms)
                    .or_else(|| stats.and_then(|stats| stats.last_ttfb_ms))
                    .unwrap_or(0)
            ),
            divisor: 1.0,
            minimum_max: 100,
            color: theme.warning,
        },
        &ui.ttfb_history,
        theme,
    );
}

fn draw_control(frame: &mut Frame, area: Rect, ui: &Ui, focused: bool, theme: super::ThemeColors) {
    let runtime = ui.runtime_state();
    let (runtime_text, runtime_color) = runtime_label(&runtime, theme);
    let database = ui
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.health.database.as_str())
        .unwrap_or("未连接");
    let mut lines = vec![
        Line::from(vec![
            Span::styled("状态     ", Style::default().fg(theme.muted)),
            Span::styled(
                runtime_text,
                Style::default()
                    .fg(runtime_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!("监听     {}", ui.config.proxy.listen_addr)),
        Line::from(format!(
            "接入     {}",
            if ui.integration_enabled() {
                "已配置"
            } else {
                "未配置"
            }
        )),
        Line::from(format!(
            "自动切换 {}",
            if ui.config.proxy.auto_switch {
                "开启"
            } else {
                "关闭"
            }
        )),
        Line::from(format!("策略     {:?}", ui.config.proxy.strategy)),
        Line::from(format!("阈值     {:.0}%", ui.config.proxy.threshold)),
        Line::from(format!("数据库   {database}")),
        Line::from(format!(
            "RPS 5m   {:.2}",
            ui.metrics.as_ref().map_or(0.0, |metrics| metrics.rps)
        )),
        Line::from(""),
        Line::from(Span::styled(
            "快捷控制",
            Style::default()
                .fg(theme.focus)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from("s 启动/停止   p 暂停/恢复"),
        Line::from("c Codex 接入  a 自动切换"),
    ];
    if let Some(snapshot) = &ui.snapshot {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            if snapshot.alerts.is_empty() {
                "✓ 当前无告警"
            } else {
                "最高优先级告警"
            },
            Style::default()
                .fg(if snapshot.alerts.is_empty() {
                    theme.success
                } else {
                    theme.error
                })
                .add_modifier(Modifier::BOLD),
        )));
        for alert in snapshot.alerts.iter().take(3) {
            lines.push(Line::from(format!("• {}", alert.title)));
            lines.push(Line::from(Span::styled(
                &alert.detail,
                Style::default().fg(theme.muted),
            )));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(panel_block(theme, "1  控制与告警", focused)),
        area,
    );
}

fn draw_pool(frame: &mut Frame, area: Rect, ui: &Ui, focused: bool, theme: super::ThemeColors) {
    let current = ui
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.current_account);
    let items = ui
        .index
        .accounts
        .iter()
        .enumerate()
        .map(|(position, account)| {
            let selected = position == ui.pool_selected;
            let (status_color, status) = status_style(theme, &account.status.kind);
            let runtime = ui.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .account_runtime
                    .iter()
                    .find(|runtime| runtime.account_id == account.id)
            });
            let primary = mini_quota(account.status.primary.as_ref());
            let secondary = mini_quota(account.status.secondary.as_ref());
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        if selected { "› " } else { "  " },
                        Style::default().fg(theme.focus),
                    ),
                    Span::styled(
                        if account.proxy_enabled {
                            "● "
                        } else {
                            "○ "
                        },
                        Style::default().fg(if account.proxy_enabled {
                            theme.success
                        } else {
                            theme.muted
                        }),
                    ),
                    Span::styled(
                        account.label.chars().take(22).collect::<String>(),
                        Style::default()
                            .fg(if selected {
                                theme.selected_text
                            } else {
                                theme.text
                            })
                            .add_modifier(if selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                    Span::styled(
                        if current == Some(account.id) {
                            "  ROUTING"
                        } else {
                            ""
                        },
                        Style::default().fg(theme.focus),
                    ),
                ]),
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(status, Style::default().fg(status_color)),
                    Span::styled(
                        format!(
                            "  P {primary}  S {secondary}  {}",
                            runtime.map_or_else(
                                || "0 实例".into(),
                                |runtime| runtime.circuit_reason.as_ref().map_or_else(
                                    || format!("{} 实例", runtime.bound_instances),
                                    |reason| format!("熔断 {reason}")
                                )
                            )
                        ),
                        Style::default().fg(theme.muted),
                    ),
                ]),
            ])
            .style(Style::default().bg(if selected {
                theme.selected_bg
            } else {
                theme.surface
            }))
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        List::new(items).block(panel_block(
            theme,
            "2  账户池  Space 加入/移出 · x 切换",
            focused,
        )),
        area,
    );
}

fn draw_instances(
    frame: &mut Frame,
    area: Rect,
    ui: &Ui,
    focused: bool,
    theme: super::ThemeColors,
) {
    let instances = ui
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.instances.as_slice())
        .unwrap_or(&[]);
    let items = instances
        .iter()
        .enumerate()
        .map(|(position, instance)| {
            let selected = position == ui.instance_selected;
            let pid = instance
                .pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "未知".into());
            let cwd = instance
                .working_directory
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| "未知目录".into());
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        if selected { "› " } else { "  " },
                        Style::default().fg(theme.focus),
                    ),
                    Span::styled(
                        format!("PID {pid}"),
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!("  {} 请求", instance.active_requests),
                        Style::default().fg(theme.warning),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        "  {cwd} · oldest {} ms · {}",
                        instance.oldest_request_ms,
                        instance
                            .current_account
                            .map(|id| id.to_string().chars().take(8).collect::<String>())
                            .unwrap_or_else(|| "未绑定".into())
                    ),
                    Style::default().fg(theme.muted),
                )),
            ])
            .style(Style::default().bg(if selected {
                theme.selected_bg
            } else {
                theme.surface
            }))
        })
        .collect::<Vec<_>>();
    let items = if items.is_empty() {
        vec![ListItem::new(vec![
            Line::from("  暂无活动实例"),
            Line::from(Span::styled(
                "  新请求会实时出现在这里",
                Style::default().fg(theme.muted),
            )),
        ])]
    } else {
        items
    };
    frame.render_widget(
        List::new(items).block(panel_block(theme, "3  活动实例", focused)),
        area,
    );
}

fn draw_events(frame: &mut Frame, area: Rect, ui: &Ui, focused: bool, theme: super::ThemeColors) {
    let mut items = ui
        .recent_requests
        .iter()
        .enumerate()
        .map(|(position, request)| {
            let selected = position == ui.event_selected;
            ListItem::new(vec![
                Line::from(vec![
                    Span::styled(
                        format!("{} {}", request.method, request.path),
                        Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(
                            "  {}",
                            request
                                .status
                                .map(|status| status.to_string())
                                .unwrap_or_else(|| "—".into())
                        ),
                        Style::default().fg(if request.partial_failure {
                            theme.error
                        } else {
                            theme.success
                        }),
                    ),
                ]),
                Line::from(Span::styled(
                    format!(
                        "  {} ms · {} · retry {}",
                        request.duration_ms.unwrap_or(0),
                        request.route_reason,
                        request.retries
                    ),
                    Style::default().fg(theme.muted),
                )),
            ])
            .style(Style::default().bg(if selected {
                theme.selected_bg
            } else {
                theme.surface
            }))
        })
        .collect::<Vec<_>>();
    let request_count = items.len();
    items.extend(ui.recent_events.iter().enumerate().map(|(index, event)| {
        let position = request_count + index;
        let selected = position == ui.event_selected;
        ListItem::new(vec![
            Line::from(vec![
                Span::styled(
                    &event.kind,
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "  {}",
                        event.occurred_at.with_timezone(&Local).format("%H:%M:%S")
                    ),
                    Style::default().fg(theme.muted),
                ),
            ]),
            Line::from(Span::styled(
                format!("  {}", event.detail),
                Style::default().fg(theme.muted),
            )),
        ])
        .style(Style::default().bg(if selected {
            theme.selected_bg
        } else {
            theme.surface
        }))
    }));
    let empty = items.is_empty();
    let items = if empty {
        vec![ListItem::new(vec![
            Line::from("  暂无近期事件"),
            Line::from(Span::styled(
                "  只记录脱敏元数据",
                Style::default().fg(theme.muted),
            )),
        ])]
    } else {
        items
    };
    let total = ui.recent_requests.len() + ui.recent_events.len();
    let title = if total == 0 {
        "4  近期事件".to_string()
    } else {
        format!("4  近期事件 · {}/{}", ui.event_selected + 1, total)
    };
    let mut state = ListState::default().with_selected((!empty).then_some(ui.event_selected));
    frame.render_stateful_widget(
        List::new(items)
            .highlight_symbol("› ")
            .highlight_style(Style::default().bg(theme.selected_bg))
            .block(panel_block(theme, &title, focused)),
        area,
        &mut state,
    );
}

fn draw_detail(
    frame: &mut Frame,
    area: Rect,
    ui: &Ui,
    detail: &DetailPage,
    theme: super::ThemeColors,
) {
    let (title, lines) = match detail {
        DetailPage::Account(id) => {
            let lines = ui
                .index
                .accounts
                .iter()
                .find(|account| account.id == *id)
                .map(|account| {
                    let runtime = ui.snapshot.as_ref().and_then(|snapshot| {
                        snapshot
                            .account_runtime
                            .iter()
                            .find(|runtime| runtime.account_id == account.id)
                    });
                    vec![
                        Line::from(format!("名称        {}", account.label)),
                        Line::from(format!(
                            "邮箱        {}",
                            account.email.as_deref().unwrap_or("未知")
                        )),
                        Line::from(format!(
                            "套餐        {}",
                            account.plan.as_deref().unwrap_or("未知")
                        )),
                        Line::from(format!(
                            "代理池      {}",
                            if account.proxy_enabled {
                                "已加入"
                            } else {
                                "未加入"
                            }
                        )),
                        Line::from(format!("状态        {}", account.status.detail)),
                        Line::from(format!(
                            "绑定实例    {}",
                            runtime.map_or(0, |runtime| runtime.bound_instances)
                        )),
                        Line::from(format!(
                            "熔断状态    {}",
                            runtime
                                .and_then(|runtime| runtime.circuit_reason.as_deref())
                                .unwrap_or("未熔断")
                        )),
                        Line::from(""),
                        Line::from("凭据、Authorization、提示词和响应正文不会显示在此处。"),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from("账户已不存在")]);
            ("账户详情", lines)
        }
        DetailPage::Control => (
            "代理控制详情",
            [
                format!("运行状态      {:?}", ui.runtime_state()),
                format!(
                    "Codex 接入    {}",
                    if ui.integration_enabled() {
                        "开启"
                    } else {
                        "关闭"
                    }
                ),
                format!("自动切换      {}", ui.config.proxy.auto_switch),
                format!("路由策略      {:?}", ui.config.proxy.strategy),
                format!("使用阈值      {:.0}%", ui.config.proxy.threshold),
            ]
            .into_iter()
            .enumerate()
            .map(|(index, text)| {
                Line::from(vec![
                    Span::styled(
                        if index == ui.control_selected {
                            "› "
                        } else {
                            "  "
                        },
                        Style::default().fg(theme.focus),
                    ),
                    Span::styled(
                        text,
                        Style::default()
                            .fg(if index == ui.control_selected {
                                theme.selected_text
                            } else {
                                theme.text
                            })
                            .add_modifier(if index == ui.control_selected {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                ])
            })
            .chain([
                Line::from(""),
                Line::from(Span::styled(
                    "j/k 选择 · Enter/Space 修改 · Esc 返回",
                    Style::default().fg(theme.muted),
                )),
                Line::from(format!("监听地址      {}", ui.config.proxy.listen_addr)),
                Line::from(format!("历史保留      {} 天", ui.config.retention.days)),
            ])
            .collect(),
        ),
        DetailPage::Instance(index) => {
            let lines = ui
                .snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.instances.get(*index))
                .map(|instance| {
                    vec![
                        Line::from(format!(
                            "PID         {}",
                            instance
                                .pid
                                .map(|pid| pid.to_string())
                                .unwrap_or_else(|| "未知".into())
                        )),
                        Line::from(format!(
                            "父 PID      {}",
                            instance
                                .parent_pid
                                .map(|pid| pid.to_string())
                                .unwrap_or_else(|| "未知".into())
                        )),
                        Line::from(format!(
                            "可执行文件  {}",
                            instance
                                .executable
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "未知".into())
                        )),
                        Line::from(format!(
                            "工作目录    {}",
                            instance
                                .working_directory
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "未知".into())
                        )),
                        Line::from(format!("实例标识    {}", instance.client_instance_id)),
                        Line::from(format!("设备        {}", instance.device_id)),
                        Line::from(format!("活动请求    {}", instance.active_requests)),
                        Line::from(format!("最长请求    {} ms", instance.oldest_request_ms)),
                        Line::from(format!(
                            "粘性账户    {}",
                            instance
                                .current_account
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "尚未绑定".into())
                        )),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from("实例已结束")]);
            ("Codex 实例详情", lines)
        }
        DetailPage::Request(index) => {
            let lines = ui
                .recent_requests
                .get(*index)
                .map(|request| {
                    vec![
                        Line::from(format!(
                            "时间        {}",
                            request
                                .started_at
                                .with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M:%S")
                        )),
                        Line::from(format!("请求        {} {}", request.method, request.path)),
                        Line::from(format!(
                            "状态        {}",
                            request
                                .status
                                .map(|status| status.to_string())
                                .unwrap_or_else(|| "未知".into())
                        )),
                        Line::from(format!("阶段        {}", request.stage)),
                        Line::from(format!("TTFB        {} ms", request.ttfb_ms.unwrap_or(0))),
                        Line::from(format!(
                            "总耗时      {} ms",
                            request.duration_ms.unwrap_or(0)
                        )),
                        Line::from(format!(
                            "上/下行     {} / {} bytes",
                            request.request_bytes, request.response_bytes
                        )),
                        Line::from(format!("路由原因    {}", request.route_reason)),
                        Line::from(format!("重试        {}", request.retries)),
                        Line::from(format!("中途断流    {}", request.partial_failure)),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from("请求摘要已过期")]);
            ("请求摘要详情", lines)
        }
        DetailPage::Event(index) => {
            let lines = ui
                .recent_events
                .get(*index)
                .map(|event| {
                    vec![
                        Line::from(format!(
                            "时间      {}",
                            event
                                .occurred_at
                                .with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M:%S")
                        )),
                        Line::from(format!("类型      {}", event.kind)),
                        Line::from(format!("租户      {}", event.tenant_id)),
                        Line::from(format!("设备      {}", event.device_id)),
                        Line::from(format!(
                            "账户      {}",
                            event
                                .account_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| "无".into())
                        )),
                        Line::from(""),
                        Line::from(event.detail.as_str()),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from("事件已过期")]);
            ("事件详情", lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(panel_block(theme, &format!("{title}  ·  Esc 返回"), true)),
        area,
    );
}

fn draw_modal(frame: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    match ui.modal {
        Modal::Help => draw_help_center(frame, ui, theme),
        modal if modal.is_text_editor() => draw_text_editor(frame, ui, theme),
        modal if modal.is_confirmation() => draw_confirmation(frame, ui, theme),
        Modal::ModeSelector => draw_choice_card(
            frame,
            theme,
            "切换工作区",
            &[
                ("1", "ACCOUNT", "认证快照、额度检测与账户切换"),
                ("2", "PROXY", "代理健康、账户池、实例与事件"),
            ],
            "按 1/2 立即进入  ·  Esc 返回",
        ),
        Modal::Onboarding => draw_choice_card(
            frame,
            theme,
            "把代理跑起来",
            &[
                ("1", "准备账户", "检测后按 Space 明确加入代理池"),
                ("2", "启动代理", "只监听本地回环地址"),
                ("3", "接入 Codex", "备份并安全更新 config.toml"),
            ],
            "建议顺序 1 → 2 → 3  ·  ? 打开完整指南  ·  Esc 稍后再说",
        ),
        Modal::None => {}
        _ => {}
    }
}

fn draw_help_center(frame: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    let popup = centered_fixed(104, 32, frame.area());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(Line::from(vec![
                Span::styled(" ◈ ", Style::default().fg(theme.focus)),
                Span::styled(
                    "使用指南 ",
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled("从第一个账户到稳定路由", Style::default().fg(theme.muted)),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.focus))
            .style(Style::default().bg(theme.surface)),
        popup,
    );
    let inner = Rect::new(
        popup.x + 2,
        popup.y + 2,
        popup.width.saturating_sub(4),
        popup.height.saturating_sub(4),
    );
    let wide = inner.width >= 76;
    let chunks = if wide {
        Layout::horizontal([
            Constraint::Length(22),
            Constraint::Length(1),
            Constraint::Min(20),
        ])
        .split(inner)
    } else {
        Layout::vertical([Constraint::Length(2), Constraint::Min(5)]).split(inner)
    };
    if wide {
        let items = HelpPage::ALL
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let active = *page == ui.help_page;
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!(" {} ", index + 1),
                        Style::default()
                            .fg(if active {
                                theme.background
                            } else {
                                theme.focus
                            })
                            .bg(if active { theme.focus } else { theme.surface })
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        format!(" {}", page.title()),
                        Style::default()
                            .fg(if active { theme.text } else { theme.muted })
                            .add_modifier(if active {
                                Modifier::BOLD
                            } else {
                                Modifier::empty()
                            }),
                    ),
                ]))
            })
            .collect::<Vec<_>>();
        frame.render_widget(
            List::new(items).block(
                Block::default()
                    .title("章 节")
                    .borders(Borders::RIGHT)
                    .border_style(Style::default().fg(theme.border)),
            ),
            chunks[0],
        );
    } else {
        let tabs = HelpPage::ALL
            .iter()
            .enumerate()
            .map(|(index, page)| {
                let active = *page == ui.help_page;
                Span::styled(
                    format!(" {} {} ", index + 1, page.title()),
                    Style::default()
                        .fg(if active {
                            theme.background
                        } else {
                            theme.muted
                        })
                        .bg(if active { theme.focus } else { theme.surface })
                        .add_modifier(if active {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                )
            })
            .collect::<Vec<_>>();
        frame.render_widget(Paragraph::new(Line::from(tabs)), chunks[0]);
    }
    let body_area = if wide { chunks[2] } else { chunks[1] };
    let body = Paragraph::new(help_lines(ui.help_page, theme))
        .scroll((ui.help_scroll, 0))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(format!(
                    "{}  ·  {}/4",
                    ui.help_page.title(),
                    ui.help_page as usize + 1
                ))
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(theme.border)),
        )
        .style(Style::default().fg(theme.text));
    frame.render_widget(body, body_area);
    let footer = Rect::new(
        inner.x,
        popup.y + popup.height.saturating_sub(2),
        inner.width,
        1,
    );
    frame.render_widget(
        Paragraph::new("Tab/Shift-Tab 翻页   1–4 直达   j/k 滚动   Home/End   Esc 关闭")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        footer,
    );
}

fn help_lines(page: HelpPage, theme: super::ThemeColors) -> Vec<Line<'static>> {
    let heading = |text: &'static str| {
        Line::from(Span::styled(
            text,
            Style::default()
                .fg(theme.focus)
                .add_modifier(Modifier::BOLD),
        ))
    };
    let key = |keys: &'static str, text: &'static str| {
        Line::from(vec![
            Span::styled(
                format!(" {keys:^12} "),
                Style::default()
                    .fg(theme.background)
                    .bg(theme.focus)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(text, Style::default().fg(theme.text)),
        ])
    };
    match page {
        HelpPage::QuickStart => vec![
            heading("三步开始"),
            Line::from(""),
            key(
                "1  ACCOUNT",
                "a 导入当前 Codex 登录，或 i 从路径 / JSON 导入",
            ),
            Line::from("│   按 r 检测额度与认证状态"),
            Line::from("│"),
            key("2  PROXY", "m 切换工作区，在账户池按 Space 加入可路由账户"),
            Line::from("│   按 s 启动本地代理，a 明确开启自动切换"),
            Line::from("│"),
            key("3  CODEX", "按 c 启用 Codex 接入，然后重启正在运行的 Codex"),
            Line::from(""),
            heading("你会看到什么"),
            Line::from("• 顶栏始终显示工作区与 daemon 连接状态"),
            Line::from("• PROXY 监控健康、流量、延迟、实例、路由与安全事件"),
            Line::from("• 修改 Codex 接入后需重启 Codex，daemon 不受 TUI 退出影响"),
        ],
        HelpPage::Account => vec![
            heading("ACCOUNT · 账户快照"),
            Line::from("导入、检测、命名并切换 Codex 认证快照。"),
            Line::from(""),
            key("j / k", "下一个 / 上一个账户"),
            key("Enter", "将选中快照设为当前 Codex 认证"),
            key("a / i", "导入当前认证 / 导入 JSON 或文件路径"),
            key("r / R", "检测选中账户 / 检测全部"),
            key("n / d", "重命名 / 删除快照"),
            key("/", "按名称或邮箱过滤；Esc 清除过滤"),
            Line::from(""),
            heading("输入框"),
            Line::from("Tab 接受补全 · ↑/↓ 或 Ctrl-P/N 选补全 · Ctrl-A/E 到行首/行尾"),
            Line::from("Ctrl-W 删除上一个词 · Ctrl-U/K 删除光标前/后内容"),
        ],
        HelpPage::Proxy => vec![
            heading("PROXY · 本地路由控制台"),
            Line::from(""),
            key("Tab / S-Tab", "在控制、账户池、实例、事件之间循环"),
            key("1 / 2 / 3 / 4", "直达对应面板"),
            key("j / k", "选择条目；Enter 打开详情"),
            key("Space", "在账户池中加入 / 移出代理资格"),
            key("s / p", "启停代理 / 暂停或恢复路由"),
            key("c / a", "Codex 接入 / 自动切换"),
            key("x", "将选中账户设为下一安全边界的路由目标"),
            Line::from(""),
            heading("不会偷偷发生的事"),
            Line::from("自动切换首次必须确认；未入池、未检测或已熔断的账户不参与路由。"),
        ],
        HelpPage::Safety => vec![
            heading("路由与安全边界"),
            Line::from(""),
            key("粘性优先", "会话 → 进程与启动时间 → 连接，尽量保持账户稳定"),
            key("安全切换", "只在尚未向 Codex 返回上游响应的请求边界切换"),
            key(
                "SSE",
                "流已开始后绝不跨账户重放；中途断流记为 partial failure",
            ),
            key("无可用账户", "立即返回明确错误与最早恢复时间，不无限等待"),
            Line::from(""),
            heading("隐私红线"),
            Line::from("只保留脱敏元数据；不记录提示词、代码、模型输出、Authorization 或 token。"),
            Line::from("控制面仅绑定随机回环端口，并使用私有 runtime 文件中的 bearer token。"),
        ],
    }
}

fn draw_text_editor(frame: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    let (title, hint, width) = match ui.modal {
        Modal::Import => ("导入认证", "粘贴 JSON，或输入 auth.json 路径", 74),
        Modal::Filter => ("过滤账户", "按名称或邮箱即时过滤", 58),
        Modal::Rename => ("重命名账户", "使用一个容易辨认的名称", 58),
        Modal::Settings => (
            "Codex 目录",
            "输入包含 config.toml 与 auth.json 的绝对路径",
            74,
        ),
        _ => return,
    };
    let visible_suggestions = ui.editor.suggestions.len().min(5) as u16;
    let height = 9 + visible_suggestions + u16::from(ui.editor.error.is_some());
    let popup = centered_fixed(width, height, frame.area());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(format!(" ✎  {title} "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.focus))
            .style(Style::default().bg(theme.surface)),
        popup,
    );
    let inner = Rect::new(
        popup.x + 2,
        popup.y + 2,
        popup.width.saturating_sub(4),
        popup.height.saturating_sub(4),
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(theme.muted)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let input_area = Rect::new(inner.x, inner.y + 2, inner.width, 3);
    let available = input_area.width.saturating_sub(3) as usize;
    let (input, cursor_column) = visible_input(&ui.editor.value, ui.editor.cursor, available);
    frame.render_widget(
        Paragraph::new(input)
            .style(Style::default().fg(theme.text))
            .block(
                Block::default()
                    .title(" 输入 ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(theme.focus)),
            ),
        input_area,
    );
    frame.set_cursor_position(Position::new(
        input_area.x + 1 + cursor_column,
        input_area.y + 1,
    ));
    let mut y = input_area.y + 3;
    if visible_suggestions > 0 {
        let start = ui.editor.suggestion_index.saturating_sub(4).min(
            ui.editor
                .suggestions
                .len()
                .saturating_sub(visible_suggestions as usize),
        );
        for (index, suggestion) in ui
            .editor
            .suggestions
            .iter()
            .enumerate()
            .skip(start)
            .take(visible_suggestions as usize)
        {
            let active = index == ui.editor.suggestion_index;
            let marker = if suggestion.directory { "◇" } else { "•" };
            frame.render_widget(
                Paragraph::new(format!(
                    " {} {marker} {}",
                    if active { "›" } else { " " },
                    suggestion.display
                ))
                .style(
                    Style::default()
                        .fg(if active {
                            theme.selected_text
                        } else {
                            theme.muted
                        })
                        .bg(if active {
                            theme.selected_bg
                        } else {
                            theme.surface
                        }),
                ),
                Rect::new(inner.x, y, inner.width, 1),
            );
            y += 1;
        }
    }
    if let Some(error) = &ui.editor.error {
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(
                    "✕ ",
                    Style::default()
                        .fg(theme.error)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(error, Style::default().fg(theme.error)),
            ]))
            .wrap(Wrap { trim: true }),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
    let footer = Rect::new(
        inner.x,
        popup.y + popup.height.saturating_sub(2),
        inner.width,
        1,
    );
    frame.render_widget(
        Paragraph::new("Enter 确认   Tab 补全   ↑/↓ 选择   Ctrl-A/E/W/U/K 编辑   Esc 取消")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        footer,
    );
}

fn visible_input(value: &str, cursor: usize, max_width: usize) -> (&str, u16) {
    if max_width == 0 {
        return ("", 0);
    }
    let mut start = cursor;
    while start > 0 {
        let previous = value[..start]
            .char_indices()
            .next_back()
            .map_or(0, |(index, _)| index);
        if UnicodeWidthStr::width(&value[previous..cursor]) >= max_width {
            break;
        }
        start = previous;
    }
    let mut end = cursor;
    for (offset, character) in value[cursor..].char_indices() {
        let next = cursor + offset + character.len_utf8();
        if UnicodeWidthStr::width(&value[start..next]) > max_width {
            break;
        }
        end = next;
    }
    (
        &value[start..end],
        UnicodeWidthStr::width(&value[start..cursor]) as u16,
    )
}

fn draw_confirmation(frame: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    let (title, detail, dangerous, cancel_label, confirm_label) = match ui.modal {
        Modal::ConfirmUseEmail => (
            "使用检测到的邮箱？",
            "邮箱通常是最易辨认的账户名。",
            false,
            "自定义",
            "使用邮箱",
        ),
        Modal::ConfirmDelete => (
            "删除账户快照？",
            "这会删除本地快照与索引记录，不会注销 OpenAI 账户。",
            true,
            "取消",
            "删除",
        ),
        Modal::ConfirmIntegrationEnable => (
            "启用 Codex 接入？",
            "将先备份，再安全更新 $CODEX_HOME/config.toml。修改后需重启 Codex。",
            false,
            "取消",
            "启用",
        ),
        Modal::ConfirmIntegrationDisable => (
            "停用 Codex 接入？",
            "只恢复本工具管理的键；发现外部配置漂移时会拒绝覆盖。",
            false,
            "取消",
            "停用",
        ),
        Modal::ConfirmProxyStart => (
            "启动本地代理？",
            "将在 127.0.0.1 上启动流式路由，只使用已入池且健康的账户。",
            false,
            "取消",
            "启动",
        ),
        Modal::ConfirmProxyStop => (
            "停止代理并恢复 Codex？",
            "停止接收新请求，活动请求最多排空 30 秒，然后恢复 Codex 配置。",
            true,
            "继续运行",
            "停止",
        ),
        Modal::ConfirmAutoSwitch => (
            "启用自动切换？",
            "只在安全请求边界切换账户；已开始输出的 SSE 永不迁移。",
            false,
            "取消",
            "启用",
        ),
        Modal::ConfirmExit => (
            "退出并停止内嵌代理？",
            "仍有活动请求。将停止接收新请求，并最多排空 30 秒。",
            true,
            "留在这里",
            "退出",
        ),
        _ => return,
    };
    let accent = if dangerous {
        theme.error
    } else {
        theme.warning
    };
    let popup = centered_fixed(66, 12, frame.area());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(Line::from(vec![
                Span::styled(
                    if dangerous { " ⚠  " } else { " ◇  " },
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    title,
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ]))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent))
            .style(Style::default().bg(theme.surface)),
        popup,
    );
    let compact = popup.height < 10;
    let body = Rect::new(
        popup.x + 3,
        popup.y + if compact { 2 } else { 3 },
        popup.width.saturating_sub(6),
        if compact { 2 } else { 3 },
    );
    frame.render_widget(
        Paragraph::new(detail)
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true })
            .style(Style::default().fg(theme.muted)),
        body,
    );
    let button = |label: &str, selected: bool, color: Color| {
        Span::styled(
            format!("  {label}  "),
            Style::default()
                .fg(if selected { theme.background } else { color })
                .bg(if selected { color } else { theme.surface })
                .add_modifier(Modifier::BOLD),
        )
    };
    let button_area = Rect::new(
        popup.x + 3,
        popup.y + popup.height.saturating_sub(4),
        popup.width.saturating_sub(6),
        1,
    );
    let button_areas = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(button_area);
    frame.render_widget(
        Paragraph::new(button(
            cancel_label,
            ui.confirm_choice == ConfirmChoice::Cancel,
            theme.focus,
        ))
        .alignment(Alignment::Center),
        button_areas[0],
    );
    frame.render_widget(
        Paragraph::new(button(
            confirm_label,
            ui.confirm_choice == ConfirmChoice::Confirm,
            accent,
        ))
        .alignment(Alignment::Center),
        button_areas[1],
    );
    frame.render_widget(
        Paragraph::new("←/→ 或 Tab 选择   Enter 执行   y/n 快捷键   Esc 取消")
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        Rect::new(
            popup.x + 2,
            popup.y + popup.height.saturating_sub(2),
            popup.width.saturating_sub(4),
            1,
        ),
    );
}

fn draw_choice_card(
    frame: &mut Frame,
    theme: super::ThemeColors,
    title: &str,
    choices: &[(&str, &str, &str)],
    footer: &str,
) {
    let height = 7 + choices.len() as u16 * 2;
    let popup = centered_fixed(72, height, frame.area());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(format!(" ◈  {title} "))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.focus))
            .style(Style::default().bg(theme.surface)),
        popup,
    );
    let mut lines = Vec::new();
    for (key, name, detail) in choices {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {key} "),
                Style::default()
                    .fg(theme.background)
                    .bg(theme.focus)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  {name:<12}"),
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::styled(*detail, Style::default().fg(theme.muted)),
        ]));
        lines.push(Line::from(""));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }),
        Rect::new(
            popup.x + 3,
            popup.y + 2,
            popup.width.saturating_sub(6),
            popup.height.saturating_sub(4),
        ),
    );
    frame.render_widget(
        Paragraph::new(footer)
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        Rect::new(
            popup.x + 2,
            popup.y + popup.height.saturating_sub(2),
            popup.width.saturating_sub(4),
            1,
        ),
    );
}

fn metric_card(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    value: &str,
    subtitle: &str,
    color: Color,
    theme: super::ThemeColors,
) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                value,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )),
            Line::from(Span::styled(subtitle, Style::default().fg(theme.muted))),
        ])
        .alignment(Alignment::Center)
        .style(Style::default().bg(theme.surface))
        .block(panel_block(theme, title, false)),
        area,
    );
}

struct MetricSparkSpec<'a> {
    title: &'a str,
    value: String,
    divisor: f64,
    minimum_max: u64,
    color: Color,
}

fn metric_spark(
    frame: &mut Frame,
    area: Rect,
    spec: MetricSparkSpec<'_>,
    history: &std::collections::VecDeque<u64>,
    theme: super::ThemeColors,
) {
    let outer = panel_block(theme, spec.title, false);
    let inner = outer.inner(area);
    frame.render_widget(outer, area);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);
    frame.render_widget(
        Paragraph::new(spec.value)
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(spec.color)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            ),
        rows[0],
    );
    let data = history.iter().copied().collect::<Vec<_>>();
    let axis_max = nice_axis_max(
        data.iter()
            .copied()
            .max()
            .unwrap_or(0)
            .max(spec.minimum_max),
    );
    let plot = Layout::horizontal([Constraint::Length(6), Constraint::Min(1)]).split(rows[1]);
    let midpoint = axis_max / 2;
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format_axis(axis_max, spec.divisor)),
            Line::from(format_axis(midpoint, spec.divisor)),
            Line::from(""),
            Line::from(format_axis(0, spec.divisor)),
        ])
        .alignment(Alignment::Right)
        .style(Style::default().fg(theme.muted).bg(theme.surface)),
        plot[0],
    );
    frame.render_widget(
        Sparkline::default()
            .data(&data)
            .max(axis_max)
            .style(Style::default().fg(spec.color).bg(theme.surface)),
        plot[1],
    );
}

fn nice_axis_max(value: u64) -> u64 {
    if value <= 1 {
        return 1;
    }
    let magnitude = 10u64.pow(value.ilog10());
    let normalized = value.div_ceil(magnitude);
    let step = match normalized {
        0..=1 => 1,
        2 => 2,
        3..=5 => 5,
        _ => 10,
    };
    step * magnitude
}

fn format_axis(value: u64, divisor: f64) -> String {
    let scaled = value as f64 / divisor;
    if scaled >= 100.0 || scaled.fract() == 0.0 {
        format!("{scaled:.0} ┤")
    } else if scaled >= 10.0 {
        format!("{scaled:.1} ┤")
    } else {
        format!("{scaled:.2} ┤")
    }
}

fn draw_quota(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    quota: Option<&Quota>,
    theme: super::ThemeColors,
) {
    let Some(quota) = quota else {
        frame.render_widget(
            Paragraph::new("尚无数据 · 按 r 检测")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.warning).bg(theme.surface))
                .block(panel_block(theme, title, false)),
            area,
        );
        return;
    };
    let remaining = (100.0 - quota.used_percent).clamp(0.0, 100.0);
    let reset = quota
        .resets_at
        .and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
        .map(|time| time.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "--".into());
    let block = panel_block(theme, title, false);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(inner);
    frame.render_widget(
        Paragraph::new(format!("{remaining:.0}% 剩余 · 重置 {reset}"))
            .style(Style::default().fg(theme.progress_text).bg(theme.surface)),
        rows[0],
    );
    frame.render_widget(
        Gauge::default().ratio(remaining / 100.0).gauge_style(
            Style::default()
                .fg(theme.progress_fill)
                .bg(theme.progress_track),
        ),
        rows[1],
    );
}

fn mini_quota(quota: Option<&Quota>) -> String {
    let Some(quota) = quota else {
        return "········".into();
    };
    let filled = (((100.0 - quota.used_percent).clamp(0.0, 100.0) / 100.0) * 8.0).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(8 - filled))
}

fn panel_block<'a>(theme: super::ThemeColors, title: &'a str, focused: bool) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(
            Style::default()
                .fg(if focused { theme.focus } else { theme.border })
                .add_modifier(if focused {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
        )
        .style(Style::default().fg(theme.text).bg(theme.surface))
}

fn runtime_label(runtime: &RuntimeState, theme: super::ThemeColors) -> (&'static str, Color) {
    match runtime {
        RuntimeState::Stopped => ("STOPPED", theme.muted),
        RuntimeState::Starting => ("STARTING", theme.warning),
        RuntimeState::Running => ("RUNNING", theme.success),
        RuntimeState::Paused => ("PAUSED", theme.warning),
        RuntimeState::Draining => ("DRAINING", theme.warning),
        RuntimeState::Blocked => ("BLOCKED", theme.error),
        RuntimeState::Error => ("ERROR", theme.error),
    }
}

fn status_style(theme: super::ThemeColors, kind: &StatusKind) -> (Color, &'static str) {
    match kind {
        StatusKind::Live => (theme.success, "✓ 可用"),
        StatusKind::Exhausted => (theme.error, "✗ 耗尽"),
        StatusKind::Reauth => (theme.warning, "⚠ 需登录"),
        StatusKind::AccessDenied => (theme.error, "✗ 拒绝"),
        StatusKind::Invalid => (theme.error, "✗ 无效"),
        StatusKind::Unknown => (theme.unknown, "? 未知"),
    }
}

fn active_account_id(ui: &Ui) -> Option<uuid::Uuid> {
    let active_path = ui.config.codex_home.join("auth.json");
    let active_content = std::fs::read(active_path).ok()?;
    ui.index
        .accounts
        .iter()
        .find(|account| {
            std::fs::read(crate::account::snapshot_path(&ui.config, account.id))
                .ok()
                .is_some_and(|content| content == active_content)
        })
        .map(|account| account.id)
}

fn centered_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.saturating_sub(2)).max(1);
    let height = height.min(area.height.saturating_sub(2)).max(1);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, types::AccountIndex};
    use ratatui::{Terminal, backend::TestBackend};

    fn rendered(width: u16, height: u16, workspace: Workspace, panel: ProxyPanel) -> String {
        let mut ui = Ui::new(Config::defaults(), AccountIndex::default(), None, workspace);
        ui.proxy_panel = panel;
        ui.modal = Modal::None;
        rendered_ui(width, height, &ui)
    }

    fn rendered_ui(width: u16, height: u16, ui: &Ui) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, ui)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn account_workspace_has_single_mode_badge_and_summary() {
        let screen = rendered(120, 34, Workspace::Accounts, ProxyPanel::Control);
        assert!(screen.contains("ACCOUNT"), "{screen}");
        assert!(screen.contains("账 户 列 表"), "{screen}");
        assert!(!screen.contains("总 览 实 例 账 户 池"), "{screen}");
    }

    #[test]
    fn wide_proxy_workspace_is_a_three_column_wall() {
        let screen = rendered(120, 30, Workspace::Proxy, ProxyPanel::Pool);
        assert!(screen.contains("控 制 与 告 警"), "{screen}");
        assert!(screen.contains("账 户 池"), "{screen}");
        assert!(screen.contains("活 动 实 例"), "{screen}");
        assert!(screen.contains("近 期 事 件"), "{screen}");
    }

    #[test]
    fn small_proxy_workspace_keeps_focused_panel_and_status() {
        let screen = rendered(60, 18, Workspace::Proxy, ProxyPanel::Events);
        assert!(screen.contains("PROXY"), "{screen}");
        assert!(screen.contains("近 期 事 件"), "{screen}");
    }

    #[test]
    fn minimum_proxy_terminal_does_not_panic_or_hide_mode() {
        let screen = rendered(40, 10, Workspace::Proxy, ProxyPanel::Control);
        assert!(screen.contains("PROXY"), "{screen}");
        assert!(screen.contains("代 理 状 态"), "{screen}");
        assert!(screen.contains("告 警"), "{screen}");
    }

    #[test]
    fn onboarding_confirmation_and_detail_have_distinct_surfaces() {
        let mut ui = Ui::new(
            Config::defaults(),
            AccountIndex::default(),
            None,
            Workspace::Proxy,
        );
        // Do not let a real daemon in the developer's home directory decide
        // whether this isolated renderer test shows onboarding.
        ui.modal = Modal::Onboarding;
        let onboarding = rendered_ui(120, 34, &ui);
        assert!(onboarding.contains("把 代 理 跑 起 来"), "{onboarding}");

        ui.modal = Modal::ConfirmProxyStop;
        let confirmation = rendered_ui(120, 34, &ui);
        assert!(
            confirmation.contains("停 止 代 理 并 恢 复  Codex"),
            "{confirmation}"
        );

        ui.modal = Modal::None;
        ui.detail = Some(DetailPage::Control);
        let detail = rendered_ui(120, 34, &ui);
        assert!(detail.contains("代 理 控 制 详 情"), "{detail}");
        assert!(detail.contains("Esc 返 回"), "{detail}");
    }

    #[test]
    fn help_center_has_four_real_pages_in_wide_and_small_terminals() {
        let mut ui = Ui::new(
            Config::defaults(),
            AccountIndex::default(),
            None,
            Workspace::Accounts,
        );
        ui.modal = Modal::Help;
        for (page, expected) in [
            (HelpPage::QuickStart, "三 步 开 始"),
            (HelpPage::Account, "账 户 快 照"),
            (HelpPage::Proxy, "本 地 路 由 控 制 台"),
            (HelpPage::Safety, "安 全 边 界"),
        ] {
            ui.help_page = page;
            let screen = rendered_ui(110, 34, &ui);
            assert!(screen.contains(expected), "{screen}");
        }

        ui.help_page = HelpPage::QuickStart;
        let small = rendered_ui(48, 16, &ui);
        assert!(small.contains("快 速 开 始"), "{small}");
    }

    #[test]
    fn editor_and_confirmation_fit_small_terminals() {
        let mut ui = Ui::new(
            Config::defaults(),
            AccountIndex::default(),
            None,
            Workspace::Accounts,
        );
        ui.open_text_editor(Modal::Settings, "/tmp/中文/.codex");
        ui.editor.suggestions.push(super::super::InputSuggestion {
            display: "codex/".into(),
            value: "/tmp/codex/".into(),
            directory: true,
        });
        ui.editor.error = Some("路径必须是绝对路径".into());
        let editor = rendered_ui(48, 14, &ui);
        assert!(editor.contains("Codex 目 录"), "{editor}");
        assert!(editor.contains("绝 对 路 径"), "{editor}");

        ui.open_confirmation(Modal::ConfirmDelete);
        let confirmation = rendered_ui(40, 10, &ui);
        assert!(confirmation.contains("取 消"), "{confirmation}");
        assert!(confirmation.contains("删 除"), "{confirmation}");
    }

    #[test]
    fn recent_events_scrolls_to_keep_the_selection_visible() {
        let mut ui = Ui::new(
            Config::defaults(),
            AccountIndex::default(),
            None,
            Workspace::Proxy,
        );
        ui.modal = Modal::None;
        ui.proxy_panel = ProxyPanel::Events;
        ui.recent_events = (0..12)
            .map(|index| crate::storage::RuntimeEvent {
                id: index.to_string(),
                occurred_at: chrono::Utc::now(),
                tenant_id: "local".into(),
                device_id: "test".into(),
                client_instance_id: None,
                kind: format!("event-{index}"),
                account_id: None,
                detail: "safe metadata".into(),
            })
            .collect();
        ui.event_selected = 11;

        let screen = rendered_ui(120, 24, &ui);
        assert!(screen.contains("12/12"), "{screen}");
        assert!(screen.contains("event-11"), "{screen}");
        assert!(!screen.contains("event-0"), "{screen}");
    }

    #[test]
    fn metric_axis_uses_stable_nice_bounds() {
        assert_eq!(nice_axis_max(0), 1);
        assert_eq!(nice_axis_max(37), 50);
        assert_eq!(nice_axis_max(101), 200);
        assert_eq!(format_axis(500, 1000.0), "0.50 ┤");
    }
}
