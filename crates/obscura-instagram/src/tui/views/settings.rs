use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::super::action::Field;
use super::super::state::{mask_sessionid, AppState};
use super::super::theme;
use super::super::widgets::{kv, panel, split_v, wrap};

pub fn draw(f: &mut Frame, app: &AppState, area: Rect) {
    let parts = split_v(area, &[Constraint::Min(8), Constraint::Length(6)]);
    draw_fields(f, app, parts[0]);
    draw_context(f, app, parts[1]);
}

fn draw_fields(f: &mut Frame, app: &AppState, area: Rect) {
    let editing = app.settings.editing;

    let header = if editing {
        vec![
            Span::styled(" Settings ", theme::strong()),
            Span::styled("· editing: Enter saves, Esc discards ", theme::warn()),
        ]
    } else {
        vec![
            Span::styled(" Settings ", theme::strong()),
            Span::styled(
                "· j/k move · i/e edit · Space toggle · F5 run ",
                theme::dim(),
            ),
        ]
    };
    let inner = panel(Line::from(header.clone()), !editing).inner(area);

    let focused = app.settings.field();
    let label_width = 20;
    let mut lines: Vec<Line> = Vec::new();

    let mut focused_line: Option<usize> = None;

    let mut section = "";
    for field in Field::ALL {
        if field.section() != section {
            section = field.section();
            lines.push(Line::from(Span::styled(
                section,
                theme::accent().add_modifier(Modifier::BOLD),
            )));
        }
        let is_cur = field == focused;
        if is_cur {
            focused_line = Some(lines.len());
        }
        let mut spans: Vec<Span> = Vec::new();

        if is_cur {
            spans.push(Span::styled("▶ ", theme::accent()));
        } else {
            spans.push(Span::raw("  "));
        }

        let label_style = if is_cur {
            theme::strong()
        } else {
            theme::dim()
        };
        spans.push(Span::styled(
            format!("{:<label_width$}", field.label()),
            label_style,
        ));

        if is_cur && editing {
            let buf = &app.settings.buffer;
            let chars: Vec<char> = buf.chars().collect();
            let cur = app.settings.buffer_cursor.min(chars.len());
            let before: String = chars[..cur].iter().collect();
            let after: String = chars[cur..].iter().collect();
            spans.push(Span::styled(before, theme::selected()));
            spans.push(Span::styled(
                "▏",
                theme::accent().add_modifier(Modifier::SLOW_BLINK),
            ));
            spans.push(Span::styled(after, theme::selected()));
        } else {
            match field {
                Field::SessionId => {
                    spans.push(Span::styled(
                        mask_sessionid(&app.form.sessionid),
                        theme::base(),
                    ));
                }
                Field::DmKind => {
                    spans.push(Span::styled(
                        format!("[ {} ]", app.form.dm_kind.label()),
                        theme::accent().add_modifier(Modifier::BOLD),
                    ));
                }
                Field::Threads if app.form.threads.trim().is_empty() => {
                    let (text, style) = if app.has_picker_override() {
                        (
                            format!("{} picked on Targets", app.picked_threads_len()),
                            theme::warn().add_modifier(Modifier::BOLD),
                        )
                    } else {
                        ("whole inbox".to_string(), theme::dim())
                    };
                    spans.push(Span::styled(text, style));
                }
                _ => {
                    if let Some(on) = app.form.flag(field) {
                        let (text, style) = if on {
                            ("[✓] on", theme::warn().add_modifier(Modifier::BOLD))
                        } else {
                            ("[ ] off", theme::dim())
                        };
                        spans.push(Span::styled(text, style));
                    } else {
                        let val = app.form.get_text(field);
                        let display = if val.trim().is_empty() {
                            "(none)".to_string()
                        } else {
                            val
                        };
                        let style = if is_cur {
                            theme::strong()
                        } else {
                            theme::base()
                        };
                        spans.push(Span::styled(display, style));
                    }
                }
            }
        }

        if let Some(err) = app.settings.errors.get(&field) {
            spans.push(Span::raw("  "));
            spans.push(Span::styled(format!("(! {err})"), theme::err()));
        }

        lines.push(Line::from(spans));
    }

    let height = inner.height as usize;
    let total = lines.len();
    let offset = match focused_line {
        Some(line) if height > 0 && total > height => (line + 1).saturating_sub(height),
        _ => 0,
    };

    let mut header = header;
    if total > height && height > 0 {
        let shown = (offset + 1).min(total);
        let last = (offset + height).min(total);
        header.push(Span::styled(
            format!("· {shown}-{last}/{total} "),
            theme::warn(),
        ));
    }
    f.render_widget(panel(Line::from(header), !editing), area);

    f.render_widget(
        Paragraph::new(lines)
            .style(theme::base())
            .scroll((offset as u16, 0)),
        inner,
    );
}

fn draw_context(f: &mut Frame, app: &AppState, area: Rect) {
    let block = panel("Details", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let focused = app.settings.field();
    let mut lines = vec![Line::from(Span::styled(focused.hint(), theme::base()))];
    if let Some(err) = app.settings.errors.get(&focused) {
        lines.push(kv("Error: ", Span::styled(err.clone(), theme::err()), 7));
    } else {
        lines.push(Line::from(Span::styled(
            focused.how_to_change(),
            theme::dim(),
        )));
    }

    f.render_widget(
        Paragraph::new(lines).style(theme::base()).wrap(wrap()),
        inner,
    );
}
