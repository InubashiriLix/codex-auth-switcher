use super::{ConfirmChoice, DetailPage, HelpPage, Modal, ProxyPanel, Ui, Workspace};
use crate::{
    i18n::{Language, LanguagePreference, translate_with},
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
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    let mut header = vec![
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
    ];
    let language = Span::styled(
        translate_with(
            ui.language(),
            "header-language",
            [("language", ui.language().native_name())],
        ),
        Style::default().fg(theme.focus),
    );
    if area.width < 72 {
        header.push(Span::raw(" "));
        header.push(language);
    } else {
        header.extend([
            daemon,
            Span::styled(
                format!("   {}   ", ui.config.theme.name()),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                format!("[m] {}   ", ui.tr("workspace-switch")),
                Style::default().fg(theme.focus),
            ),
            language,
        ]);
    }
    frame.render_widget(
        Paragraph::new(Line::from(header))
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
        Workspace::Accounts => ui.tr("footer-accounts"),
        Workspace::Proxy => ui.tr("footer-proxy"),
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
        .map(|account| account.label.clone())
        .unwrap_or_else(|| ui.tr("not-set"));
    metric_card(
        frame,
        cards[0],
        &ui.tr("accounts"),
        &ui.index.accounts.len().to_string(),
        &ui.tr("saved"),
        theme.focus,
        theme,
    );
    metric_card(
        frame,
        cards[1],
        &ui.tr("healthy"),
        &live.to_string(),
        &ui.tr("ready-to-use"),
        theme.success,
        theme,
    );
    metric_card(
        frame,
        cards[2],
        &ui.tr("needs-attention"),
        &ui.index.accounts.len().saturating_sub(live).to_string(),
        &ui.tr("check-or-login"),
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
        &ui.tr("current-direct"),
        &active,
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
            let (color, status) = status_style(ui, theme, &account.status.kind);
            let direct = if active == Some(account.id) {
                format!("  ● {}", ui.tr("direct"))
            } else {
                String::new()
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
                        format!(
                            "  {}",
                            account
                                .email
                                .clone()
                                .unwrap_or_else(|| ui.tr("email-unknown"))
                        ),
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
    let title = if ui.filter.is_empty() {
        ui.tr("account-list")
    } else {
        ui.tr("filter-results")
    };
    frame.render_widget(
        List::new(items).block(panel_block(theme, &title, true)),
        area,
    );
}

fn draw_account_summary(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let Some(index) = ui.selected_id() else {
        frame.render_widget(
            Paragraph::new(ui.tr("no-accounts").replace("\\n", "\n"))
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.muted).bg(theme.surface))
                .block(panel_block(theme, &ui.tr("account-details"), false)),
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
    let (color, status) = status_style(ui, theme, &account.status.kind);
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(Span::styled(
                &account.label,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(status, Style::default().fg(color)),
                Span::styled(
                    format!(
                        "   {}",
                        account
                            .plan
                            .clone()
                            .unwrap_or_else(|| ui.tr("plan-unknown"))
                    ),
                    Style::default().fg(theme.muted),
                ),
            ]),
            Line::from(format!(
                "{}  {}",
                ui.tr("email"),
                account.email.clone().unwrap_or_else(|| ui.tr("unknown"))
            )),
            Line::from(format!("{}  {}", ui.tr("source"), account.source)),
        ])
        .style(Style::default().fg(theme.text).bg(theme.surface))
        .block(panel_block(theme, &ui.tr("identity-status"), false)),
        parts[0],
    );
    draw_quota(
        frame,
        parts[1],
        &ui.tr("primary-quota"),
        account.status.primary.as_ref(),
        ui,
        theme,
    );
    draw_quota(
        frame,
        parts[2],
        &ui.tr("secondary-quota"),
        account.status.secondary.as_ref(),
        ui,
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
        .unwrap_or_else(|| ui.tr("never-checked"));
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("{}  {checked}", ui.tr("last-check"))),
            Line::from(format!(
                "{}  {}",
                ui.tr("status-detail"),
                account.status.detail
            )),
        ])
        .wrap(Wrap { trim: false })
        .style(Style::default().fg(theme.muted).bg(theme.surface))
        .block(panel_block(theme, &ui.tr("diagnostics"), false)),
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
    let (runtime, color) = runtime_label(ui, &ui.runtime_state(), theme);
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
                    "  {} {}  {} {}  {} {}  ",
                    ui.tr("integration"),
                    if ui.integration_enabled() {
                        "ON"
                    } else {
                        "OFF"
                    },
                    ui.tr("healthy-pool"),
                    ui.eligible_accounts(),
                    ui.tr("active"),
                    active
                ),
                Style::default().fg(theme.text),
            ),
            Span::styled(
                format!("{} {}", ui.tr("panel"), ui.proxy_panel.number()),
                Style::default().fg(theme.focus),
            ),
        ]))
        .style(Style::default().bg(theme.surface))
        .block(panel_block(theme, &ui.tr("proxy-status"), false)),
        area,
    );
}

fn draw_alert_strip(frame: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let alert = ui
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.alerts.first());
    let (text, color) = alert.map_or_else(
        || (format!(" ✓ {}", ui.tr("no-alerts")), theme.success),
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
    let (runtime_text, runtime_color) = runtime_label(ui, &runtime, theme);
    metric_card(
        frame,
        columns[0],
        &ui.tr("data-proxy"),
        &runtime_text,
        &ui.config.proxy.listen_addr,
        runtime_color,
        theme,
    );
    metric_card(
        frame,
        columns[1],
        &ui.tr("codex-integration"),
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
        &ui.tr("healthy-pool"),
        &ui.eligible_accounts().to_string(),
        &format!(
            "{} · {} {}",
            ui.tr("routable"),
            ui.snapshot
                .as_ref()
                .map_or(0, |snapshot| snapshot.active_requests.len()),
            ui.tr("active-requests")
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
        &ui.tr("success-rate"),
        &format!("{success:.1}%"),
        &ui.tr("cumulative"),
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
    let (runtime_text, runtime_color) = runtime_label(ui, &runtime, theme);
    let database = ui
        .snapshot
        .as_ref()
        .map(|snapshot| snapshot.health.database.clone())
        .unwrap_or_else(|| ui.tr("disconnected"));
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                format!("{}     ", ui.tr("status")),
                Style::default().fg(theme.muted),
            ),
            Span::styled(
                runtime_text,
                Style::default()
                    .fg(runtime_color)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(format!(
            "{}     {}",
            ui.tr("listening"),
            ui.config.proxy.listen_addr
        )),
        Line::from(format!(
            "{}     {}",
            ui.tr("integration"),
            if ui.integration_enabled() {
                ui.tr("configured")
            } else {
                ui.tr("not-configured")
            }
        )),
        Line::from(format!(
            "{} {}",
            ui.tr("auto-switch"),
            if ui.config.proxy.auto_switch {
                ui.tr("on")
            } else {
                ui.tr("off")
            }
        )),
        Line::from(format!(
            "{}     {:?}",
            ui.tr("strategy"),
            ui.config.proxy.strategy
        )),
        Line::from(format!(
            "{}     {:.0}%",
            ui.tr("threshold"),
            ui.config.proxy.threshold
        )),
        Line::from(format!("{}   {database}", ui.tr("database"))),
        Line::from(format!(
            "RPS 5m   {:.2}",
            ui.metrics.as_ref().map_or(0.0, |metrics| metrics.rps)
        )),
        Line::from(""),
        Line::from(Span::styled(
            ui.tr("quick-controls"),
            Style::default()
                .fg(theme.focus)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(ui.tr("quick-start-pause")),
        Line::from(ui.tr("quick-codex-auto")),
    ];
    if let Some(snapshot) = &ui.snapshot {
        lines.push(Line::from(""));
        let alert_heading = if snapshot.alerts.is_empty() {
            format!("✓ {}", ui.tr("no-alerts"))
        } else {
            ui.tr("highest-alert")
        };
        lines.push(Line::from(Span::styled(
            alert_heading,
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
            .block(panel_block(
                theme,
                &format!("1  {}", ui.tr("control-alerts")),
                focused,
            )),
        area,
    );
}

fn draw_pool(frame: &mut Frame, area: Rect, ui: &Ui, focused: bool, theme: super::ThemeColors) {
    let current = ui
        .snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.current_account);
    // Each pool row is intentionally laid out using terminal-cell widths, not
    // byte or character counts. Localized status names (for example French
    // "Disponible") otherwise shift the P/S quota columns on every row.
    let content_width = usize::from(area.width.saturating_sub(4));
    let quota_width = if content_width >= 62 {
        8
    } else if content_width >= 48 {
        6
    } else {
        4
    };
    let status_width = content_width
        .saturating_sub(quota_width * 2 + 8)
        .clamp(8, 12);
    let items = ui
        .index
        .accounts
        .iter()
        .enumerate()
        .map(|(position, account)| {
            let selected = position == ui.pool_selected;
            let (status_color, status) = status_style(ui, theme, &account.status.kind);
            let runtime = ui.snapshot.as_ref().and_then(|snapshot| {
                snapshot
                    .account_runtime
                    .iter()
                    .find(|runtime| runtime.account_id == account.id)
            });
            let primary = mini_quota(account.status.primary.as_ref(), quota_width);
            let secondary = mini_quota(account.status.secondary.as_ref(), quota_width);
            let runtime_label = runtime.map_or_else(
                || format!("0 {}", ui.tr("instances")),
                |runtime| {
                    runtime.circuit_reason.as_ref().map_or_else(
                        || format!("{} {}", runtime.bound_instances, ui.tr("instances")),
                        |reason| format!("{} {reason}", ui.tr("circuited")),
                    )
                },
            );
            let quota_line_width = 2 + status_width + 3 + quota_width + 3 + quota_width;
            let show_runtime =
                quota_line_width + 2 + display_width(&runtime_label) <= content_width;
            let label_width =
                content_width.saturating_sub(if current == Some(account.id) { 14 } else { 4 });
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
                        truncate_display(&account.label, label_width),
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
                    Span::styled(
                        pad_display(&status, status_width),
                        Style::default().fg(status_color),
                    ),
                    Span::styled(
                        format!("  P {primary}  S {secondary}"),
                        Style::default().fg(theme.muted),
                    ),
                    Span::styled(
                        if show_runtime {
                            format!("  {runtime_label}")
                        } else {
                            String::new()
                        },
                        Style::default().fg(theme.muted),
                    ),
                ]),
                Line::from(Span::styled(
                    if show_runtime {
                        String::new()
                    } else {
                        format!("  {runtime_label}")
                    },
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
    frame.render_widget(
        List::new(items).block(panel_block(
            theme,
            &truncate_display(
                &format!("2  {}  Space +/- · x", ui.tr("account-pool")),
                usize::from(area.width.saturating_sub(4)),
            ),
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
                .unwrap_or_else(|| ui.tr("unknown"));
            let cwd = instance
                .working_directory
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy())
                .unwrap_or_else(|| ui.tr("unknown-directory").into());
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
                        format!("  {} {}", instance.active_requests, ui.tr("requests")),
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
                            .unwrap_or_else(|| ui.tr("unbound"))
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
            Line::from(format!("  {}", ui.tr("no-active-instances"))),
            Line::from(Span::styled(
                format!("  {}", ui.tr("new-requests-here")),
                Style::default().fg(theme.muted),
            )),
        ])]
    } else {
        items
    };
    frame.render_widget(
        List::new(items).block(panel_block(
            theme,
            &format!("3  {}", ui.tr("active-instances")),
            focused,
        )),
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
                        localized_route_reason(ui, request),
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
                format!("  {}", localized_event_detail(ui, event)),
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
            Line::from(format!("  {}", ui.tr("no-recent-events"))),
            Line::from(Span::styled(
                format!("  {}", ui.tr("sanitized-metadata-only")),
                Style::default().fg(theme.muted),
            )),
        ])]
    } else {
        items
    };
    let total = ui.recent_requests.len() + ui.recent_events.len();
    let title = if total == 0 {
        format!("4  {}", ui.tr("recent-events"))
    } else {
        format!(
            "4  {} · {}/{}",
            ui.tr("recent-events"),
            ui.event_selected + 1,
            total
        )
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
                        Line::from(format!("{}        {}", ui.tr("name"), account.label)),
                        Line::from(format!(
                            "{}        {}",
                            ui.tr("email"),
                            account.email.clone().unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!(
                            "{}        {}",
                            ui.tr("plan"),
                            account.plan.clone().unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!(
                            "{}      {}",
                            ui.tr("account-pool"),
                            if account.proxy_enabled {
                                ui.tr("joined")
                            } else {
                                ui.tr("not-joined")
                            }
                        )),
                        Line::from(format!(
                            "{}        {}",
                            ui.tr("status"),
                            account.status.detail
                        )),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("bound-instances"),
                            runtime.map_or(0, |runtime| runtime.bound_instances)
                        )),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("circuit-state"),
                            runtime
                                .and_then(|runtime| runtime.circuit_reason.as_deref())
                                .map(str::to_owned)
                                .unwrap_or_else(|| ui.tr("not-circuited"))
                        )),
                        Line::from(""),
                        Line::from(ui.tr("privacy-detail-note")),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from(ui.tr("account-gone"))]);
            (ui.tr("account-details"), lines)
        }
        DetailPage::Control => (
            ui.tr("proxy-control-details"),
            [
                format!(
                    "{}      {}",
                    ui.tr("runtime-status"),
                    runtime_label(ui, &ui.runtime_state(), theme).0
                ),
                format!(
                    "{}    {}",
                    ui.tr("codex-integration"),
                    if ui.integration_enabled() {
                        ui.tr("on")
                    } else {
                        ui.tr("off")
                    }
                ),
                format!(
                    "{}      {}",
                    ui.tr("auto-switch"),
                    ui.config.proxy.auto_switch
                ),
                format!("{}      {:?}", ui.tr("strategy"), ui.config.proxy.strategy),
                format!(
                    "{}      {:.0}%",
                    ui.tr("threshold"),
                    ui.config.proxy.threshold
                ),
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
                    ui.tr("control-detail-controls"),
                    Style::default().fg(theme.muted),
                )),
                Line::from(format!(
                    "{}      {}",
                    ui.tr("listen-address"),
                    ui.config.proxy.listen_addr
                )),
                Line::from(format!(
                    "{}      {} {}",
                    ui.tr("history-retention"),
                    ui.config.retention.days,
                    ui.tr("days")
                )),
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
                                .unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!(
                            "{}      {}",
                            ui.tr("parent-pid"),
                            instance
                                .parent_pid
                                .map(|pid| pid.to_string())
                                .unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!(
                            "{}  {}",
                            ui.tr("executable"),
                            instance
                                .executable
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("working-directory"),
                            instance
                                .working_directory
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("instance-id"),
                            instance.client_instance_id
                        )),
                        Line::from(format!("{}        {}", ui.tr("device"), instance.device_id)),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("active-requests"),
                            instance.active_requests
                        )),
                        Line::from(format!(
                            "{}    {} ms",
                            ui.tr("oldest-request"),
                            instance.oldest_request_ms
                        )),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("sticky-account"),
                            instance
                                .current_account
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| ui.tr("not-bound"))
                        )),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from(ui.tr("instance-ended"))]);
            (ui.tr("instance-details"), lines)
        }
        DetailPage::Request(index) => {
            let lines = ui
                .recent_requests
                .get(*index)
                .map(|request| {
                    vec![
                        Line::from(format!(
                            "{}        {}",
                            ui.tr("time"),
                            request
                                .started_at
                                .with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M:%S")
                        )),
                        Line::from(format!(
                            "{}        {} {}",
                            ui.tr("request"),
                            request.method,
                            request.path
                        )),
                        Line::from(format!(
                            "{}        {}",
                            ui.tr("status"),
                            request
                                .status
                                .map(|status| status.to_string())
                                .unwrap_or_else(|| ui.tr("unknown"))
                        )),
                        Line::from(format!("{}        {}", ui.tr("stage"), request.stage)),
                        Line::from(format!("TTFB        {} ms", request.ttfb_ms.unwrap_or(0))),
                        Line::from(format!(
                            "{}      {} ms",
                            ui.tr("total-duration"),
                            request.duration_ms.unwrap_or(0)
                        )),
                        Line::from(format!(
                            "{}     {} / {} bytes",
                            ui.tr("traffic"),
                            request.request_bytes,
                            request.response_bytes
                        )),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("route-reason"),
                            localized_route_reason(ui, request)
                        )),
                        Line::from(format!("{}        {}", ui.tr("retries"), request.retries)),
                        Line::from(format!(
                            "{}    {}",
                            ui.tr("partial-failure"),
                            request.partial_failure
                        )),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from(ui.tr("request-expired"))]);
            (ui.tr("request-details"), lines)
        }
        DetailPage::Event(index) => {
            let lines = ui
                .recent_events
                .get(*index)
                .map(|event| {
                    vec![
                        Line::from(format!(
                            "{}      {}",
                            ui.tr("time"),
                            event
                                .occurred_at
                                .with_timezone(&Local)
                                .format("%Y-%m-%d %H:%M:%S")
                        )),
                        Line::from(format!("{}      {}", ui.tr("type"), event.kind)),
                        Line::from(format!("{}      {}", ui.tr("tenant"), event.tenant_id)),
                        Line::from(format!("{}      {}", ui.tr("device"), event.device_id)),
                        Line::from(format!(
                            "{}      {}",
                            ui.tr("accounts"),
                            event
                                .account_id
                                .map(|id| id.to_string())
                                .unwrap_or_else(|| ui.tr("none"))
                        )),
                        Line::from(""),
                        Line::from(localized_event_detail(ui, event)),
                    ]
                })
                .unwrap_or_else(|| vec![Line::from(ui.tr("event-expired"))]);
            (ui.tr("event-details"), lines)
        }
    };
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(panel_block(
                theme,
                &format!("{title}  ·  Esc {}", ui.tr("back")),
                true,
            )),
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
            &ui.tr("mode-title"),
            &[
                ("1".into(), "ACCOUNT".into(), ui.tr("mode-account-detail")),
                ("2".into(), "PROXY".into(), ui.tr("mode-proxy-detail")),
            ],
            &ui.tr("mode-controls"),
        ),
        Modal::Onboarding => draw_choice_card(
            frame,
            theme,
            &ui.tr("onboarding-title"),
            &[
                (
                    "1".into(),
                    ui.tr("onboarding-account"),
                    ui.tr("onboarding-account-detail"),
                ),
                (
                    "2".into(),
                    ui.tr("onboarding-proxy"),
                    ui.tr("onboarding-proxy-detail"),
                ),
                (
                    "3".into(),
                    ui.tr("onboarding-codex"),
                    ui.tr("onboarding-codex-detail"),
                ),
            ],
            &ui.tr("onboarding-controls"),
        ),
        Modal::LanguageSelector => draw_language_selector(frame, ui, theme),
        Modal::None => {}
        _ => {}
    }
}

fn draw_language_selector(frame: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    let popup = centered_fixed(64, 14, frame.area());
    frame.render_widget(Clear, popup);
    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(8),
        Constraint::Length(2),
    ])
    .split(popup.inner(ratatui::layout::Margin {
        horizontal: 2,
        vertical: 1,
    }));
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(format!(" 🌐 {} ", ui.tr("language-title")))
            .border_style(Style::default().fg(theme.focus))
            .style(Style::default().bg(theme.surface)),
        popup,
    );
    let items = LanguagePreference::ALL
        .iter()
        .enumerate()
        .map(|(index, preference)| {
            let name = if *preference == LanguagePreference::Auto {
                translate_with(
                    ui.language(),
                    "language-auto-current",
                    [("language", preference.resolve().native_name())],
                )
            } else {
                preference.resolve().native_name().to_owned()
            };
            let selected = index == ui.language_selected;
            ListItem::new(format!(" {}  {name}", index + 1)).style(
                Style::default()
                    .fg(if selected {
                        theme.selected_text
                    } else {
                        theme.text
                    })
                    .bg(if selected {
                        theme.selected_bg
                    } else {
                        theme.surface
                    })
                    .add_modifier(if selected {
                        Modifier::BOLD
                    } else {
                        Modifier::empty()
                    }),
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(List::new(items), rows[1]);
    frame.render_widget(
        Paragraph::new(ui.tr("language-help"))
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        rows[2],
    );
}

fn draw_help_center(frame: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    let popup = centered_fixed(104, 32, frame.area());
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Block::default()
            .title(Line::from(vec![
                Span::styled(" ◈ ", Style::default().fg(theme.focus)),
                Span::styled(
                    format!("{} ", ui.tr("help-guide-title")),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    ui.tr("help-guide-subtitle"),
                    Style::default().fg(theme.muted),
                ),
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
                        format!(" {}", page.title(ui.language())),
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
                    .title(ui.tr("help-sections"))
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
                    format!(" {} {} ", index + 1, page.title(ui.language())),
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
    let body = Paragraph::new(help_lines(ui.help_page, theme, ui))
        .scroll((ui.help_scroll, 0))
        .wrap(Wrap { trim: false })
        .block(
            Block::default()
                .title(format!(
                    "{}  ·  {}/4",
                    ui.help_page.title(ui.language()),
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
        Paragraph::new(ui.tr("help-controls"))
            .alignment(Alignment::Center)
            .style(Style::default().fg(theme.muted)),
        footer,
    );
}

fn help_lines(page: HelpPage, theme: super::ThemeColors, ui: &Ui) -> Vec<Line<'static>> {
    if ui.language() != Language::ZhCn {
        let key = match page {
            HelpPage::QuickStart => "help-body-quick",
            HelpPage::Account => "help-body-account",
            HelpPage::Proxy => "help-body-proxy",
            HelpPage::Safety => "help-body-safety",
        };
        return ui
            .tr(key)
            .split("\\n")
            .map(|line| Line::from(line.to_owned()))
            .collect();
    }
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
            key(
                "l / 🌐",
                "Language · 语言 · 言語 · Idioma · Sprache · Lingua · Langue",
            ),
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
        Modal::Import => (ui.tr("editor-import"), ui.tr("editor-import-hint"), 74),
        Modal::Filter => (ui.tr("editor-filter"), ui.tr("editor-filter-hint"), 58),
        Modal::Rename => (ui.tr("editor-rename"), ui.tr("editor-rename-hint"), 58),
        Modal::Settings => (
            ui.tr("editor-codex-home"),
            ui.tr("editor-codex-home-hint"),
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
                    .title(format!(" {} ", ui.tr("input")))
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
        Paragraph::new(ui.tr("editor-controls"))
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
            ui.tr("confirm-email-title"),
            ui.tr("confirm-email-detail"),
            false,
            ui.tr("custom"),
            ui.tr("use-email"),
        ),
        Modal::ConfirmDelete => (
            ui.tr("confirm-delete-title"),
            ui.tr("confirm-delete-detail"),
            true,
            ui.tr("cancel"),
            ui.tr("delete"),
        ),
        Modal::ConfirmIntegrationEnable => (
            ui.tr("confirm-integration-on-title"),
            ui.tr("confirm-integration-on-detail"),
            false,
            ui.tr("cancel"),
            ui.tr("enable"),
        ),
        Modal::ConfirmIntegrationDisable => (
            ui.tr("confirm-integration-off-title"),
            ui.tr("confirm-integration-off-detail"),
            false,
            ui.tr("cancel"),
            ui.tr("disable"),
        ),
        Modal::ConfirmProxyStart => (
            ui.tr("confirm-proxy-start-title"),
            ui.tr("confirm-proxy-start-detail"),
            false,
            ui.tr("cancel"),
            ui.tr("start"),
        ),
        Modal::ConfirmProxyStop => (
            ui.tr("confirm-proxy-stop-title"),
            ui.tr("confirm-proxy-stop-detail"),
            true,
            ui.tr("keep-running"),
            ui.tr("stop"),
        ),
        Modal::ConfirmAutoSwitch => (
            ui.tr("confirm-auto-title"),
            ui.tr("confirm-auto-detail"),
            false,
            ui.tr("cancel"),
            ui.tr("enable"),
        ),
        Modal::ConfirmExit => (
            ui.tr("confirm-exit-title"),
            ui.tr("confirm-exit-detail"),
            true,
            ui.tr("stay"),
            ui.tr("exit"),
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
            &cancel_label,
            ui.confirm_choice == ConfirmChoice::Cancel,
            theme.focus,
        ))
        .alignment(Alignment::Center),
        button_areas[0],
    );
    frame.render_widget(
        Paragraph::new(button(
            &confirm_label,
            ui.confirm_choice == ConfirmChoice::Confirm,
            accent,
        ))
        .alignment(Alignment::Center),
        button_areas[1],
    );
    frame.render_widget(
        Paragraph::new(ui.tr("confirm-controls"))
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
    choices: &[(String, String, String)],
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
            Span::styled(detail.clone(), Style::default().fg(theme.muted)),
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
    ui: &Ui,
    theme: super::ThemeColors,
) {
    let Some(quota) = quota else {
        frame.render_widget(
            Paragraph::new(ui.tr("quota-no-data"))
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
        Paragraph::new(translate_with(
            ui.language(),
            "quota-remaining",
            [("remaining", format!("{remaining:.0}")), ("reset", reset)],
        ))
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

fn mini_quota(quota: Option<&Quota>, width: usize) -> String {
    let Some(quota) = quota else {
        return "·".repeat(width);
    };
    let filled =
        (((100.0 - quota.used_percent).clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

fn display_width(value: &str) -> usize {
    UnicodeWidthStr::width(value)
}

/// Restrict text by the number of terminal cells it occupies, preserving UTF-8
/// boundaries and leaving room for an ellipsis when truncation is needed.
fn truncate_display(value: &str, width: usize) -> String {
    if display_width(value) <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".into();
    }
    let mut result = String::new();
    let mut used = 0;
    for character in value.chars() {
        let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
        if used + character_width > width - 1 {
            break;
        }
        result.push(character);
        used += character_width;
    }
    result.push('…');
    result
}

fn pad_display(value: &str, width: usize) -> String {
    let mut result = truncate_display(value, width);
    result.push_str(&" ".repeat(width.saturating_sub(display_width(&result))));
    result
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

fn runtime_label(ui: &Ui, runtime: &RuntimeState, theme: super::ThemeColors) -> (String, Color) {
    match runtime {
        RuntimeState::Stopped => (ui.tr("runtime-stopped"), theme.muted),
        RuntimeState::Starting => (ui.tr("runtime-starting"), theme.warning),
        RuntimeState::Running => (ui.tr("runtime-running"), theme.success),
        RuntimeState::Paused => (ui.tr("runtime-paused"), theme.warning),
        RuntimeState::Draining => (ui.tr("runtime-draining"), theme.warning),
        RuntimeState::Blocked => (ui.tr("runtime-blocked"), theme.error),
        RuntimeState::Error => (ui.tr("runtime-error"), theme.error),
    }
}

fn status_style(ui: &Ui, theme: super::ThemeColors, kind: &StatusKind) -> (Color, String) {
    match kind {
        StatusKind::Live => (theme.success, ui.tr("status-live")),
        StatusKind::Exhausted => (theme.error, ui.tr("status-exhausted")),
        StatusKind::Reauth => (theme.warning, ui.tr("status-reauth")),
        StatusKind::AccessDenied => (theme.error, ui.tr("status-denied")),
        StatusKind::Invalid => (theme.error, ui.tr("status-invalid")),
        StatusKind::Unknown => (theme.unknown, ui.tr("status-unknown")),
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

fn localized_event_detail(ui: &Ui, event: &crate::storage::RuntimeEvent) -> String {
    event.message.as_ref().map_or_else(
        || event.detail.clone(),
        |message| message.render(ui.language(), &event.detail),
    )
}

fn localized_route_reason(ui: &Ui, request: &crate::storage::RequestSummary) -> String {
    request.route_message.as_ref().map_or_else(
        || request.route_reason.clone(),
        |message| message.render(ui.language(), &request.route_reason),
    )
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
    use crate::{
        config::Config,
        types::{Account, AccountIndex, CheckStatus},
    };
    use ratatui::{Terminal, backend::TestBackend};

    fn test_config() -> Config {
        let mut config = Config::defaults();
        config.language = LanguagePreference::ZhCn;
        config
    }

    fn rendered(width: u16, height: u16, workspace: Workspace, panel: ProxyPanel) -> String {
        let mut ui = Ui::new(test_config(), AccountIndex::default(), None, workspace);
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
    fn localized_pool_statuses_keep_quota_columns_aligned() {
        let mut config = Config::defaults();
        config.language = LanguagePreference::Fr;
        let quota = || {
            Some(Quota {
                used_percent: 25.0,
                window_minutes: None,
                resets_at: None,
            })
        };
        let account = |label: &str, kind: StatusKind| Account {
            id: uuid::Uuid::new_v4(),
            label: label.into(),
            source: "test".into(),
            imported_at: chrono::Utc::now(),
            email: None,
            plan: None,
            account_id: None,
            status: CheckStatus {
                kind,
                checked_at: None,
                detail: String::new(),
                primary: quota(),
                secondary: quota(),
            },
            tenant_id: "local".into(),
            proxy_enabled: true,
        };
        let mut ui = Ui::new(
            config,
            AccountIndex {
                accounts: vec![
                    account("very-long-account-name@example.com", StatusKind::Live),
                    account(
                        "another-long-account-name@example.com",
                        StatusKind::Exhausted,
                    ),
                ],
            },
            None,
            Workspace::Proxy,
        );
        ui.modal = Modal::None;
        ui.proxy_panel = ProxyPanel::Pool;
        let screen = rendered_ui(72, 18, &ui);
        let rows = screen
            .lines()
            .filter(|line| line.contains("Disponible") || line.contains("Épuisé"))
            .collect::<Vec<_>>();
        assert_eq!(rows.len(), 2, "{screen}");
        let column = |row: &str, marker| {
            let index = row.find(marker).expect("marker in rendered row");
            display_width(&row[..index])
        };
        assert_eq!(column(rows[0], "P "), column(rows[1], "P "), "{screen}");
        assert_eq!(column(rows[0], "S "), column(rows[1], "S "), "{screen}");
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
            test_config(),
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
            test_config(),
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
            test_config(),
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
            test_config(),
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
                message: None,
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

    #[test]
    fn every_language_renders_the_header_and_selector_in_small_terminals() {
        for preference in LanguagePreference::ALL.into_iter().skip(1) {
            let mut config = test_config();
            config.language = preference;
            let mut ui = Ui::new(config, AccountIndex::default(), None, Workspace::Accounts);
            ui.modal = Modal::LanguageSelector;
            let screen = rendered_ui(48, 16, &ui);
            assert!(screen.contains("[l]"), "{preference:?}: {screen}");
            assert!(screen.contains("1"), "{preference:?}: {screen}");
        }
    }

    #[test]
    fn non_chinese_help_and_confirmation_use_the_selected_catalog() {
        let mut config = test_config();
        config.language = LanguagePreference::Es;
        let mut ui = Ui::new(config, AccountIndex::default(), None, Workspace::Accounts);
        ui.modal = Modal::Help;
        ui.help_page = HelpPage::QuickStart;
        let help = rendered_ui(100, 30, &ui);
        assert!(help.contains("TRES PASOS"), "{help}");
        assert!(!help.contains("三 步 开 始"), "{help}");

        ui.open_confirmation(Modal::ConfirmDelete);
        let confirmation = rendered_ui(72, 16, &ui);
        assert!(confirmation.contains("Eliminar"), "{confirmation}");
        assert!(!confirmation.contains("删 除 账 户"), "{confirmation}");
    }
}
