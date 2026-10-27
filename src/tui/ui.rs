use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};

use crate::tui::app::{App, DeleteKind, Focus, Phase};
use crate::util::{ellipsize, format_age, format_elapsed, format_size, truncate_right};

const ACCENT: Color = Color::Rgb(120, 200, 255);
const DIM: Color = Color::Rgb(120, 120, 130);
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let footer_h = if app.phase == Phase::Deleting { 2 } else { 1 };
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(footer_h),
    ])
    .split(area);

    header(frame, rows[0], app);
    match app.phase {
        Phase::Scanning => scanning(frame, rows[1], app),
        Phase::Browse | Phase::Deleting => browse(frame, rows[1], app),
        Phase::Empty => empty(frame, rows[1], app),
    }
    footer(frame, rows[2], app);

    if app.help {
        help_overlay(frame, area);
    }
    if app.confirm.is_some() {
        confirm_overlay(frame, area, app);
    }
}

fn header(frame: &mut Frame, area: Rect, app: &App) {
    let width = area.width as usize;
    let roots = app
        .opts
        .roots
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join("  ");
    let left = format!(" kembar  {}", ellipsize(&roots, width.saturating_sub(34)));
    let right = match (&app.report, app.phase) {
        (Some(report), _) if !report.groups.is_empty() => format!(
            "{} groups · {} freeable ",
            report.groups.len(),
            format_size(report.reclaimable)
        ),
        _ => String::new(),
    };
    let pad = width.saturating_sub(left.chars().count() + right.chars().count());
    let line = Line::from(vec![
        Span::styled(left, Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::new().fg(Color::Yellow)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn scanning(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(DIM))
        .title(" scanning ");
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(1),
    ])
    .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            app.progress_label.clone(),
            Style::new().fg(Color::White),
        )))
        .wrap(Wrap { trim: true }),
        rows[0],
    );

    let pct = app.progress.unwrap_or(0).min(100);
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(Color::Green).bg(Color::Rgb(30, 30, 36)))
            .percent(u16::from(pct))
            .label(format!("{pct}%")),
        rows[1],
    );

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "size grouping → 4 KB prehash → full BLAKE3 → byte confirm",
            Style::new().fg(DIM),
        ))),
        rows[2],
    );
}

fn browse(frame: &mut Frame, area: Rect, app: &mut App) {
    let panes =
        Layout::horizontal([Constraint::Percentage(44), Constraint::Percentage(56)]).split(area);
    groups_panel(frame, panes[0], app);

    let right = Layout::vertical([Constraint::Min(3), Constraint::Length(9)]).split(panes[1]);
    files_panel(frame, right[0], app);
    details_panel(frame, right[1], app);
}

fn groups_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let visible = app.visible_groups();
    let items = group_items(app, &visible);
    let title = if app.filter.is_empty() {
        format!(" Groups ({}) ", visible.len())
    } else {
        format!(" Groups ({}) filter: {} ", visible.len(), app.filter)
    };
    let focused = app.focus == Focus::Groups;
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
                .title(title),
        )
        .highlight_style(
            Style::new()
                .bg(Color::Rgb(48, 48, 62))
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut app.groups_state);
}

fn group_items(app: &App, visible: &[usize]) -> Vec<ListItem<'static>> {
    let Some(report) = &app.report else {
        return vec![ListItem::new("no results")];
    };
    visible
        .iter()
        .map(|&gi| {
            let group = &report.groups[gi];
            let marked = app
                .marked
                .get(gi)
                .map(|row| row.iter().filter(|m| **m).count())
                .unwrap_or(0);
            let flag = if marked == 0 {
                "  "
            } else if marked + 1 >= group.files.len() {
                "! "
            } else {
                "• "
            };
            let name = group.files.first().map(|f| f.name()).unwrap_or_default();
            let line = Line::from(vec![
                Span::styled(
                    format!("{:>9}", format_size(group.size)),
                    Style::new().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{flag}{:>2}×  ", group.files.len()),
                    Style::new().fg(if marked == 0 { DIM } else { Color::LightRed }),
                ),
                Span::styled(ellipsize(&name, 34), Style::new().fg(Color::White)),
            ]);
            ListItem::new(line)
        })
        .collect()
}

fn files_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let title = match app.current_group() {
        Some(group) => format!(
            " Copies ({}) · {} each ",
            group.files.len(),
            format_size(group.size)
        ),
        None => " Copies ".to_string(),
    };
    let items = file_items(app);
    let focused = app.focus == Focus::Files;
    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(if focused { ACCENT } else { DIM }))
                .title(title),
        )
        .highlight_style(
            Style::new()
                .bg(Color::Rgb(48, 48, 62))
                .add_modifier(Modifier::BOLD),
        );
    frame.render_stateful_widget(list, area, &mut app.files_state);
}

fn file_items(app: &App) -> Vec<ListItem<'static>> {
    let Some(gi) = app.current_group_index() else {
        return vec![ListItem::new(Line::from(Span::styled(
            " no group selected ",
            Style::new().fg(DIM),
        )))];
    };
    let Some(group) = app.report.as_ref().and_then(|r| r.groups.get(gi)) else {
        return Vec::new();
    };
    let keeper = app.keeper_index(gi);
    group
        .files
        .iter()
        .enumerate()
        .map(|(fi, file)| {
            let marked = app
                .marked
                .get(gi)
                .and_then(|row| row.get(fi))
                .copied()
                .unwrap_or(false);
            let is_keeper = keeper == Some(fi);
            let style = if marked {
                Style::new().fg(Color::LightRed)
            } else if is_keeper {
                Style::new().fg(Color::LightGreen)
            } else {
                Style::new().fg(Color::White)
            };
            let box_mark = if marked { "[x]" } else { "[ ]" };
            let star = if is_keeper { "★" } else { " " };
            let line = Line::from(vec![
                Span::styled(format!("{box_mark}{star} "), style),
                Span::styled(ellipsize(&file.name(), 32), style),
                Span::raw("  "),
                Span::styled(
                    format!("{:>8}", format_size(file.size)),
                    Style::new().fg(Color::Yellow),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("{:>8}", format_age(file.modified)),
                    Style::new().fg(DIM),
                ),
            ]);
            ListItem::new(line)
        })
        .collect()
}

fn details_panel(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(DIM))
        .title(" Details ");
    let lines = detail_lines(app, area.width);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn detail_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let Some(group) = app.current_group() else {
        return vec![Line::from("")];
    };
    let mut lines = Vec::new();
    if let Some(file) = group.files.get(app.file_idx) {
        lines.push(Line::from(Span::styled(
            ellipsize(
                &file.path.to_string_lossy(),
                width.saturating_sub(2) as usize,
            ),
            Style::new().fg(ACCENT),
        )));
    }
    lines.push(Line::from(vec![
        Span::styled(format_size(group.size), Style::new().fg(Color::Yellow)),
        Span::raw(" each · "),
        Span::styled(
            format!("{} copies", group.files.len()),
            Style::new().fg(Color::White),
        ),
        Span::raw(" · "),
        Span::styled(
            format!("{} freeable", format_size(group.reclaimable())),
            Style::new().fg(Color::Green),
        ),
    ]));
    let hash = if group.hash.len() > 16 {
        format!("{}…", &group.hash[..16])
    } else {
        group.hash.clone()
    };
    lines.push(Line::from(vec![
        Span::styled("blake3 ", Style::new().fg(DIM)),
        Span::styled(hash, Style::new().fg(DIM)),
    ]));
    if let Some(keeper) = app.keeper_index(app.current_group_index().unwrap_or(0))
        && let Some(file) = group.files.get(keeper)
    {
        lines.push(Line::from(vec![
            Span::styled("keep ", Style::new().fg(DIM)),
            Span::styled(app.keep.label(), Style::new().fg(Color::Green)),
            Span::raw(" → "),
            Span::styled(
                ellipsize(
                    &file.path.to_string_lossy(),
                    width.saturating_sub(14) as usize,
                ),
                Style::new().fg(Color::Green),
            ),
        ]));
    }
    if let Some(report) = &app.report
        && report.elapsed_ms > 0
    {
        lines.push(Line::from(Span::styled(
            format!(
                "scanned in {} · {} warnings",
                format_elapsed(report.elapsed_ms),
                report.warnings.len()
            ),
            Style::new().fg(DIM),
        )));
    }
    lines
}

fn footer(frame: &mut Frame, area: Rect, app: &App) {
    if app.phase == Phase::Deleting {
        delete_bar(frame, area, app);
        return;
    }
    let width = area.width as usize;
    let hints = "j/k move  Tab pane  space mark  a select  s keep  d trash  D purge  o finder  r rescan  / filter  ? help  q quit";
    let status = if app.status.is_empty() {
        hints.to_string()
    } else if width > 80 {
        format!("{}   ·   {}", app.status, hints)
    } else {
        app.status.clone()
    };
    let style = if app.confirm.is_some() {
        Style::new().fg(Color::Black).bg(Color::Yellow)
    } else {
        Style::new().fg(DIM)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_right(&status, width),
            style,
        ))),
        area,
    );
}

fn empty(frame: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(DIM))
        .title(" kembar ");
    let mut lines = vec![
        Line::from(Span::styled(
            "No duplicates to show.",
            Style::new().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    if let Some(report) = &app.report {
        for warning in report.warnings.iter().take(6) {
            lines.push(Line::from(Span::styled(
                format!("⚠ {warning}"),
                Style::new().fg(Color::Yellow),
            )));
        }
        for error in report.errors.iter().take(6) {
            lines.push(Line::from(Span::styled(
                format!("✗ {error}"),
                Style::new().fg(Color::LightRed),
            )));
        }
        if report.elapsed_ms > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!("scan finished in {}", format_elapsed(report.elapsed_ms)),
                Style::new().fg(DIM),
            )));
        }
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "r rescan   ·   q quit",
        Style::new().fg(DIM),
    )));
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(block)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn help_overlay(frame: &mut Frame, area: Rect) {
    let entries = [
        ("j / k", "move selection"),
        ("g / G", "jump to first / last"),
        ("Tab, h, l", "switch between groups and copies"),
        ("space", "toggle mark (groups pane: smart-mark the group)"),
        ("a", "Smart Selection: mark all copies except the keepers"),
        ("A", "clear every mark"),
        ("s", "cycle keep strategy: oldest → newest → shallowest → …"),
        ("d", "move marked files to Trash (y to confirm)"),
        ("D", "permanently delete marked files (y to confirm)"),
        ("o", "reveal current file in Finder"),
        ("/", "filter groups by path"),
        ("r", "rescan"),
        ("q", "quit"),
    ];
    let height = (entries.len() as u16) + 4;
    let popup = centered(area, 74, height);
    frame.render_widget(Clear, popup);
    let mut lines = vec![Line::from(Span::styled(
        "kembar — keys",
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    ))];
    for (key, desc) in entries {
        lines.push(Line::from(vec![
            Span::styled(format!("{key:<12}"), Style::new().fg(Color::Yellow)),
            Span::raw(desc),
        ]));
    }
    frame.render_widget(
        Paragraph::new(Text::from(lines)).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(ACCENT))
                .title(" help · any key to close "),
        ),
        popup,
    );
}

fn confirm_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let Some(pending) = &app.confirm else {
        return;
    };
    let popup = centered(area, 64, 7);
    frame.render_widget(Clear, popup);
    let (title, verb, colour) = match pending.kind {
        crate::tui::app::DeleteKind::Trash => (" move to Trash ", "move to Trash", Color::Yellow),
        crate::tui::app::DeleteKind::Purge => {
            (" permanent delete ", "permanently delete", Color::LightRed)
        }
    };
    let lines = vec![
        Line::from(vec![
            Span::styled(
                verb.to_string(),
                Style::new().fg(colour).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!(
                " {} files · {}",
                pending.files.len(),
                format_size(pending.bytes)
            )),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "y confirm   ·   n / Esc cancel",
            Style::new().fg(DIM),
        )),
    ];
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(colour))
                    .title(title),
            )
            .wrap(Wrap { trim: true }),
        popup,
    );
}

fn delete_bar(frame: &mut Frame, area: Rect, app: &App) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).split(area);
    let (verb, color) = match app.delete_kind {
        Some(DeleteKind::Purge) => ("Deleting", Color::LightRed),
        _ => ("Moving to Trash", Color::Yellow),
    };
    let pct = if app.delete_total == 0 {
        0
    } else {
        (app.delete_completed as u64 * 100 / app.delete_total as u64).min(100) as u16
    };
    let glyph = SPIN[app.spin % SPIN.len()];
    let hint = if app.delete_cancelling {
        "cancelling"
    } else {
        "q cancel"
    };
    let text = format!(
        "{glyph} {verb}  {}/{} · {} / {} · {hint}",
        app.delete_completed,
        app.delete_total,
        format_size(app.delete_freed),
        format_size(app.delete_bytes_total),
    );
    let width = rows[0].width as usize;
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            fit_bar(&text, width),
            Style::new()
                .fg(Color::Black)
                .bg(color)
                .add_modifier(Modifier::BOLD),
        ))),
        rows[0],
    );
    frame.render_widget(
        Gauge::default()
            .gauge_style(Style::new().fg(color).bg(Color::Rgb(30, 30, 36)))
            .percent(pct)
            .label(format!("{pct}%")),
        rows[1],
    );
}

fn fit_bar(text: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    let fitted = truncate_right(text, width);
    let len = fitted.chars().count();
    if len >= width {
        fitted
    } else {
        format!("{fitted}{}", " ".repeat(width - len))
    }
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn delete_status_bar_shows_progress() {
        let backend = TestBackend::new(80, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = crate::tui::app::blank_for_test();
        app.phase = Phase::Deleting;
        app.delete_kind = Some(DeleteKind::Trash);
        app.delete_completed = 40;
        app.delete_total = 200;
        app.delete_freed = 1024 * 1024;
        app.delete_bytes_total = 5 * 1024 * 1024;

        terminal.draw(|frame| draw(frame, &mut app)).unwrap();

        let view = terminal.backend().to_string();
        assert!(view.contains("Moving to Trash"), "{view}");
        assert!(view.contains("40/200"), "{view}");
        assert!(view.contains("1.0 MB"), "{view}");
        assert!(view.contains("5.0 MB"), "{view}");
        assert!(view.contains("q cancel"), "{view}");
        assert!(view.contains("20%"), "{view}");
    }
}
