use super::{Modal, Ui};
use crate::types::{Quota, StatusKind};
use chrono::Local;
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap},
};
use std::sync::atomic::Ordering;

const HELP_TEXT: &str = "j/k 或 ↑/↓ 移动 · Ctrl-n/Ctrl-p 翻页 · Enter 启用
a 当前认证 · i 路径/JSON · n 重命名 · d 删除
r 单个检测 · R 全部检测 · / 过滤 · s 设置 · t 切换主题
Ctrl-C 退出程序
Use q or Esc to exit this helper window";

pub fn draw(f: &mut Frame, ui: &Ui) {
    let theme = ui.config.theme.colors();
    f.render_widget(
        Block::default().style(Style::default().bg(theme.background)),
        f.area(),
    );

    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .split(f.area());

    // 标题栏
    let active_id = active_account_id(ui);
    let active_label = active_id
        .and_then(|id| ui.index.accounts.iter().find(|a| a.id == id))
        .map(|a| a.label.as_str())
        .unwrap_or("没有受管理的活动账户");

    let proxy_status = if let Some(proxy_state) = &ui.proxy_state {
        if proxy_state.running.load(Ordering::Relaxed) {
            " [代理运行中]"
        } else {
            " [代理已停止]"
        }
    } else {
        ""
    };

    let title = format!(
        " Codex Switcher  ·  当前: {}  ·  {} 个账户{}",
        active_label,
        ui.index.accounts.len(),
        proxy_status
    );

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                title,
                Style::default()
                    .fg(theme.focus)
                    .bg(theme.surface)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "  [a]导入 [i]路径 [r/R]检测 [t]主题 [Enter]启用 [?]帮助",
                Style::default().fg(theme.text).bg(theme.surface),
            ),
        ]))
        .style(Style::default().fg(theme.text).bg(theme.surface))
        .block(block(theme, "")),
        areas[0],
    );

    // 主内容区
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(48), Constraint::Percentage(52)])
        .split(areas[1]);

    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(5)])
        .split(chunks[0]);

    // 账户列表
    let visible = ui.visible();
    let items: Vec<ListItem> = visible
        .iter()
        .enumerate()
        .map(|(pos, i)| {
            let a = &ui.index.accounts[*i];
            let (c, st) = status_style(theme, &a.status.kind);
            let marker = if pos == ui.selected { "›" } else { " " };
            let active = if active_id == Some(a.id) {
                " ● 当前"
            } else {
                ""
            };
            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} "), Style::default().fg(theme.focus)),
                Span::styled(
                    format!("{:<24}", a.label.chars().take(24).collect::<String>()),
                    Style::default()
                        .fg(if pos == ui.selected {
                            theme.selected_text
                        } else {
                            theme.text
                        })
                        .add_modifier(if pos == ui.selected {
                            Modifier::BOLD
                        } else {
                            Modifier::empty()
                        }),
                ),
                Span::styled(format!(" {st}{active}"), Style::default().fg(c)),
            ]))
            .style(Style::default().bg(if pos == ui.selected {
                theme.selected_bg
            } else {
                theme.surface
            }))
        })
        .collect();

    f.render_widget(
        List::new(items)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(block(
                theme,
                if ui.filter.is_empty() {
                    "账户"
                } else {
                    "过滤结果"
                },
            )),
        left[0],
    );

    draw_overview(f, left[1], ui, theme);

    // 右侧详情
    let right = chunks[1];
    if let Some(i) = ui.selected_id() {
        let a = &ui.index.accounts[i];
        let h = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),
                Constraint::Length(4),
                Constraint::Length(4),
                Constraint::Min(5),
            ])
            .split(right);

        let (c, st) = status_style(theme, &a.status.kind);
        let meta = vec![
            Line::from(vec![
                Span::styled(
                    &a.label,
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!(
                        "  {st}{}",
                        if active_id == Some(a.id) {
                            "  ● 当前生效"
                        } else {
                            ""
                        }
                    ),
                    Style::default().fg(c),
                ),
            ]),
            Line::from(format!(
                "邮箱：{}    套餐：{}",
                a.email.as_deref().unwrap_or("未知"),
                a.plan.as_deref().unwrap_or("未知")
            )),
            Line::from(format!(
                "{} · {}",
                a.status.detail,
                a.status
                    .checked_at
                    .map(|x| x.with_timezone(&Local).format("%H:%M:%S").to_string())
                    .unwrap_or_else(|| "未检查".into())
            )),
        ];

        f.render_widget(
            Paragraph::new(meta)
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(block(theme, "详情")),
            h[0],
        );

        draw_quota(f, h[1], "主要额度窗口", a.status.primary.as_ref(), theme);
        draw_quota(f, h[2], "次要额度窗口", a.status.secondary.as_ref(), theme);

        // 代理状态或概览
        if ui.proxy_state.is_some() {
            draw_proxy_status(f, h[3], ui, theme);
        } else {
            draw_overview(f, h[3], ui, theme);
        }
    } else {
        let h = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(8), Constraint::Length(5)])
            .split(right);

        f.render_widget(
            Paragraph::new("没有账户。按 a 导入当前 Codex 登录，或按 i 导入 JSON/路径。")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(block(theme, "详情")),
            h[0],
        );
        draw_overview(f, h[1], ui, theme);
    }

    // 状态栏
    f.render_widget(
        Paragraph::new(format!(" {}", ui.notice))
            .style(Style::default().fg(theme.muted).bg(theme.background))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .style(Style::default().fg(theme.border).bg(theme.background)),
            ),
        areas[2],
    );

    // 模态框
    if ui.modal != Modal::None {
        draw_modal(f, ui, theme);
    }
}

fn draw_quota(f: &mut Frame, area: Rect, title: &str, q: Option<&Quota>, theme: super::ThemeColors) {
    let Some(q) = q else {
        f.render_widget(
            Paragraph::new("尚无额度数据\n按 r 检测此账户 · R 检测全部")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.warning).bg(theme.surface))
                .block(block(theme, title)),
            area,
        );
        return;
    };

    let reset = q
        .resets_at
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
        .map(|x| x.with_timezone(&Local).format("%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "--".into());

    let window = q
        .window_minutes
        .map(|minutes| format!("{} 分钟", minutes))
        .unwrap_or_else(|| "未知窗口".into());

    let ratio = ((100. - q.used_percent).clamp(0., 100.) / 100.) as f64;
    let label = format!(
        "{:.0}% 剩余 · {window} · 重置 {reset}",
        (100. - q.used_percent).max(0.)
    );

    let outer = block(theme, title);
    let inner = outer.inner(area);
    f.render_widget(outer, area);

    let quota_lines = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(inner);

    f.render_widget(
        Paragraph::new(label).style(Style::default().fg(theme.progress_text).bg(theme.surface)),
        quota_lines[0],
    );

    f.render_widget(
        Gauge::default()
            .gauge_style(
                Style::default()
                    .fg(theme.progress_fill)
                    .bg(theme.progress_track),
            )
            .ratio(ratio),
        quota_lines[1],
    );
}

fn draw_overview(f: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    let live = ui
        .index
        .accounts
        .iter()
        .filter(|a| a.status.kind == StatusKind::Live)
        .count();
    let needs_attention = ui.index.accounts.len().saturating_sub(live);

    let activity = if let Some(checking) = &ui.checking {
        let spinner = ["|", "/", "-", "\\"][(ui.tick as usize) % 4];
        Line::from(vec![
            Span::styled(format!(" {spinner} "), Style::default().fg(theme.focus)),
            Span::styled(
                format!(
                    "正在检测 {}（{}/{}）",
                    checking.current, checking.completed, checking.total
                ),
                Style::default().fg(theme.warning),
            ),
        ])
    } else {
        Line::from(Span::styled(
            " 就绪 · r 检测所选 · R 检测全部",
            Style::default().fg(theme.muted),
        ))
    };

    let lines = vec![
        Line::from(vec![
            Span::styled(
                format!(" {} 可用", live),
                Style::default().fg(theme.success),
            ),
            Span::styled(
                format!("  ·  {} 待关注", needs_attention),
                Style::default().fg(if needs_attention == 0 {
                    theme.muted
                } else {
                    theme.warning
                }),
            ),
            Span::styled("  ·  n 重命名", Style::default().fg(theme.muted)),
        ]),
        activity,
    ];

    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(block(theme, "账户概览")),
        area,
    );
}

fn draw_proxy_status(f: &mut Frame, area: Rect, ui: &Ui, theme: super::ThemeColors) {
    if let Some(proxy_state) = &ui.proxy_state {
        let stats = proxy_state.stats.read();
        let running = proxy_state.running.load(Ordering::Relaxed);

        let status_text = if running { "运行中" } else { "已停止" };
        let status_color = if running { theme.success } else { theme.error };

        let lines = vec![
            Line::from(vec![
                Span::styled("状态: ", Style::default().fg(theme.muted)),
                Span::styled(status_text, Style::default().fg(status_color)),
            ]),
            Line::from(format!("总请求: {}", stats.total_requests)),
            Line::from(format!("失败: {}", stats.failed_requests)),
            Line::from(format!("自动切换: {}", stats.auto_switches)),
        ];

        f.render_widget(
            Paragraph::new(lines)
                .style(Style::default().fg(theme.text).bg(theme.surface))
                .block(block(theme, "代理状态")),
            area,
        );
    }
}

fn draw_modal(f: &mut Frame, ui: &Ui, theme: super::ThemeColors) {
    let popup = centered(70, 30, f.area());
    f.render_widget(Clear, popup);

    let (title, text) = match ui.modal {
        Modal::Import => ("导入：输入 JSON 或本地文件路径", ui.input.as_str()),
        Modal::Filter => ("过滤账户", ui.input.as_str()),
        Modal::Rename => ("重命名账户", ui.input.as_str()),
        Modal::ConfirmUseEmail => (
            "使用检测到的邮箱？",
            "按 y 使用邮箱作为名称；按 n 输入自定义名称；Esc 取消",
        ),
        Modal::Settings => ("Codex 目录（保存后生效）", ui.input.as_str()),
        Modal::ConfirmDelete => ("确认删除", "按 y 永久删除快照；Esc 取消"),
        Modal::Help => ("键位", HELP_TEXT),
        Modal::ModeSelector => (
            "模式选择",
            "1. 交互模式（当前）\n2. 代理模式（手动）\n3. 代理模式（自动）\n\nEsc 取消",
        ),
        Modal::ProxySettings => ("代理设置", ui.input.as_str()),
        Modal::None => unreachable!(),
    };

    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(theme.text).bg(theme.surface))
            .block(block(theme, title)),
        popup,
    );
}

fn block<'a>(theme: super::ThemeColors, title: &'a str) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        .title(title)
        .style(Style::default().fg(theme.border).bg(theme.surface))
}

fn centered(x: u16, y: u16, area: Rect) -> Rect {
    let v = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - y) / 2),
            Constraint::Percentage(y),
            Constraint::Percentage((100 - y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - x) / 2),
            Constraint::Percentage(x),
            Constraint::Percentage((100 - x) / 2),
        ])
        .split(v[1])[1]
}

fn status_style(theme: super::ThemeColors, kind: &StatusKind) -> (ratatui::style::Color, &'static str) {
    match kind {
        StatusKind::Live => (theme.success, "✓ 可用"),
        StatusKind::Exhausted => (theme.error, "✗ 耗尽"),
        StatusKind::Reauth => (theme.warning, "⚠ 需重新认证"),
        StatusKind::AccessDenied => (theme.error, "✗ 拒绝访问"),
        StatusKind::Invalid => (theme.error, "✗ 无效"),
        StatusKind::Unknown => (theme.unknown, "? 未知"),
    }
}

fn active_account_id(ui: &Ui) -> Option<uuid::Uuid> {
    let active_path = ui.config.codex_home.join("auth.json");
    if !active_path.exists() {
        return None;
    }

    let active_content = std::fs::read(&active_path).ok()?;

    ui.index.accounts.iter().find(|a| {
        let snapshot_path = crate::account::snapshot_path(&ui.config, a.id);
        std::fs::read(&snapshot_path)
            .ok()
            .map(|content| content == active_content)
            .unwrap_or(false)
    }).map(|a| a.id)
}
