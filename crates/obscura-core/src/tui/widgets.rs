//! Platform-independent terminal UI building blocks.

use ratatui::layout::{Alignment, Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Padding, Paragraph, Wrap};
use ratatui::Frame;

use crate::{events::LogLevel, theme, tui::Screen};

pub fn panel<'a>(title: impl Into<Line<'a>>, focused: bool) -> Block<'a> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Rounded
        } else {
            BorderType::Plain
        })
        .border_style(if focused {
            theme::focus()
        } else {
            theme::border()
        })
        .title_style(if focused {
            theme::strong()
        } else {
            theme::dim().add_modifier(Modifier::BOLD)
        })
        .title(title)
        .padding(Padding::horizontal(1))
}

pub fn hints(items: &[(&str, &str)]) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, (key, label)) in items.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", theme::base()));
        }
        spans.push(Span::styled(format!("[{key}]"), theme::key()));
        spans.push(Span::styled(format!(" {label}"), theme::dim()));
    }
    Line::from(spans)
}

pub fn badge(text: &str, style: Style) -> Span<'static> {
    Span::styled(format!(" {text} "), style.add_modifier(Modifier::BOLD))
}

pub fn tab_line(current: Screen) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, screen) in Screen::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("·", theme::border()));
        }
        let is_current = *screen == current;
        let title_style = if is_current {
            Style::default()
                .fg(theme::ACCENT)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme::dim()
        };
        let number_style = if is_current {
            theme::accent().add_modifier(Modifier::BOLD)
        } else {
            theme::key()
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(format!("{}:", i + 1), number_style));
        spans.push(Span::styled(format!("{} ", screen.title()), title_style));
    }
    Line::from(spans)
}

pub fn palette_matches<'a>(
    query: &str,
    commands: impl IntoIterator<Item = (usize, &'a str, &'a str, &'a str)>,
) -> Vec<usize> {
    let query = query.trim().to_ascii_lowercase();
    commands
        .into_iter()
        .filter_map(|(index, id, label, hint)| {
            (query.is_empty()
                || label.to_ascii_lowercase().contains(&query)
                || hint.to_ascii_lowercase().contains(&query)
                || id.to_ascii_lowercase().contains(&query))
            .then_some(index)
        })
        .collect()
}

/// Draws a platform-provided tab line and status spans with shared spacing.
pub fn draw_header(f: &mut Frame, area: Rect, tabs: Line<'static>, status: Vec<Span<'static>>) {
    let status_width = status.iter().map(|span| span.width() as u16).sum::<u16>();
    let areas = split(
        area,
        &[Constraint::Min(1), Constraint::Length(status_width.max(1))],
    );
    f.render_widget(Paragraph::new(tabs), areas[0]);
    f.render_widget(
        Paragraph::new(Line::from(status)).alignment(Alignment::Right),
        areas[1],
    );
}

/// Draws platform-provided key hints and an optional shared-style toast.
pub fn draw_footer(
    f: &mut Frame,
    area: Rect,
    hints: Line<'static>,
    toast: Option<(&str, LogLevel)>,
) {
    let hint_width = hints.width() as u16;
    let areas = split(
        area,
        &[
            Constraint::Min(1),
            Constraint::Length(area.width.saturating_sub(hint_width)),
        ],
    );
    f.render_widget(Paragraph::new(hints), areas[0]);
    if let Some((text, level)) = toast {
        let style = theme::level(level).add_modifier(Modifier::BOLD);
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(text.to_owned(), style)))
                .alignment(Alignment::Right),
            areas[1],
        );
    }
}

pub fn centered_rect(width_pct: u16, height_pct: u16, area: Rect) -> Rect {
    let vertical = ratatui::layout::Layout::vertical([
        Constraint::Percentage((100 - height_pct) / 2),
        Constraint::Percentage(height_pct),
        Constraint::Percentage((100 - height_pct) / 2),
    ])
    .split(area);
    let horizontal = ratatui::layout::Layout::horizontal([
        Constraint::Percentage((100 - width_pct) / 2),
        Constraint::Percentage(width_pct),
        Constraint::Percentage((100 - width_pct) / 2),
    ])
    .split(vertical[1]);
    horizontal[1]
}

pub fn split(area: Rect, constraints: &[Constraint]) -> Vec<Rect> {
    ratatui::layout::Layout::horizontal(constraints.to_vec())
        .split(area)
        .to_vec()
}

pub fn split_v(area: Rect, constraints: &[Constraint]) -> Vec<Rect> {
    ratatui::layout::Layout::vertical(constraints.to_vec())
        .split(area)
        .to_vec()
}

pub fn kv(label: &str, value: Span<'static>, label_width: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<label_width$}"), theme::dim()),
        value,
    ])
}

pub fn clock(at: std::time::SystemTime) -> String {
    use chrono::{DateTime, Local};
    let dt: DateTime<Local> = at.into();
    dt.format("%H:%M:%S").to_string()
}

pub fn human_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

pub fn ellipsize(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        return s.to_string();
    }
    let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

pub fn level_tag(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => "info",
        LogLevel::Success => "done",
        LogLevel::Warning => "warn",
        LogLevel::Error => "err ",
    }
}

pub fn wrap() -> Wrap {
    Wrap { trim: false }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_duration_formats_each_magnitude() {
        use std::time::Duration;
        assert_eq!(human_duration(Duration::from_secs(9)), "9s");
        assert_eq!(human_duration(Duration::from_secs(64)), "1m 04s");
        assert_eq!(human_duration(Duration::from_secs(3720)), "1h 02m");
    }

    #[test]
    fn split_partitions_the_full_area() {
        let area = Rect::new(0, 0, 80, 10);
        let parts = split(area, &[Constraint::Length(10), Constraint::Min(1)]);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].width, 10);
        assert_eq!(parts[0].width + parts[1].width, 80);
        assert_eq!(parts[1].x, 10);
    }

    #[test]
    fn palette_matching_uses_trimmed_case_insensitive_fields() {
        let matches = palette_matches(
            "  ARCH ",
            [
                (0, "quit", "Quit", "close the program"),
                (1, "archive", "Open Archive", "browse history"),
            ],
        );
        assert_eq!(matches, [1]);
    }
}
