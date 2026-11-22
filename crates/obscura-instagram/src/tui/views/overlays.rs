use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::config::{thread_id_list, DmKind};

use super::super::action::{self, Overlay};
use super::super::keys;
use super::super::state::{AppState, Form};
use super::super::theme;
use super::super::widgets::{centered_rect, ellipsize, panel, split_v, wrap};

pub fn draw(f: &mut Frame, app: &AppState, area: Rect) {
    match app.overlay {
        Overlay::None => {}
        Overlay::Palette => draw_palette(f, app, area),
        Overlay::Confirm => draw_confirm(f, app, area),
        Overlay::Help => draw_help(f, area),
    }
}

fn draw_palette(f: &mut Frame, app: &AppState, area: Rect) {
    let win = centered_rect(60, 45, area);
    f.render_widget(Clear, win);
    let block = panel(
        Line::from(vec![
            Span::styled(" Command palette ", theme::strong()),
            Span::styled("· type to filter · Enter select · Esc close ", theme::dim()),
        ]),
        true,
    );
    let inner = block.inner(win);
    f.render_widget(block, win);

    let prompt = Line::from(vec![
        Span::styled("> ", theme::accent().add_modifier(Modifier::BOLD)),
        Span::styled(app.palette.query.clone(), theme::strong()),
        Span::styled("▏", theme::accent().add_modifier(Modifier::SLOW_BLINK)),
    ]);
    let parts = split_v(inner, &[Constraint::Length(1), Constraint::Min(1)]);
    f.render_widget(Paragraph::new(prompt), parts[0]);

    let items: Vec<ListItem> = app
        .palette
        .matches
        .iter()
        .filter_map(|&i| action::commands().get(i))
        .map(|cmd| {
            let line = Line::from(vec![
                Span::styled(format!("{:<18}", cmd.label), theme::strong()),
                Span::styled(format!(" {}", cmd.hint), theme::dim()),
            ]);
            ListItem::new(line)
        })
        .collect();

    if items.is_empty() {
        let empty = Paragraph::new("No matching commands").style(theme::dim());
        f.render_widget(empty, parts[1]);
        return;
    }

    let mut state = ListState::default();
    state.select(Some(app.palette.cursor.min(items.len().saturating_sub(1))));
    f.render_stateful_widget(
        List::new(items)
            .style(theme::base())
            .highlight_style(theme::selected())
            .highlight_symbol("▶ "),
        parts[1],
        &mut state,
    );
}

fn scope_row(label: &str, value: String) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label}: "), theme::dim()),
        Span::styled(value, theme::accent()),
    ])
}

fn date_scope(form: &Form) -> String {
    let (after, before) = (form.after.trim(), form.before.trim());
    match (after.is_empty(), before.is_empty()) {
        (true, true) => "all dates, no filter".to_string(),
        (false, true) => format!("newer than {after}"),
        (true, false) => format!("older than {before}"),
        (false, false) => format!("newer than {after}, older than {before}"),
    }
}

fn thread_scope(app: &AppState) -> String {
    let typed = thread_id_list(&app.form.threads).len();
    if app.has_picker_override() {
        format!("{} picked", app.picks_in_scope_len())
    } else if typed > 0 {
        format!("{typed} typed")
    } else {
        "whole inbox".to_string()
    }
}

/// Everything that decides what goes, so nothing about the run is a surprise.
fn confirm_lines(app: &AppState, width: usize) -> Vec<Line<'static>> {
    let form = &app.form;
    let has_dates = !form.after.trim().is_empty() || !form.before.trim().is_empty();
    let limit = form.limit.trim().parse::<u64>().unwrap_or(0);

    let mut lines = vec![
        Line::from(Span::styled(
            "This run permanently deletes from your account:",
            theme::err().add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(form.content.label(), theme::accent())),
    ];
    if form.content.activity.unfollow {
        lines.push(if has_dates {
            Line::from(Span::styled(
                "Unfollow is skipped: follows have no date to filter by.",
                theme::warn(),
            ))
        } else {
            Line::from(Span::styled(
                "Unfollows accounts that don't follow you back.",
                theme::err(),
            ))
        });
    }
    lines.push(Line::raw(""));
    lines.push(scope_row("Dates", date_scope(form)));
    lines.push(scope_row(
        "Limit",
        if limit == 0 {
            "none, everything found".to_string()
        } else {
            format!("first {limit} items")
        },
    ));
    if form.content.touches_threads() {
        lines.push(scope_row("DM threads", thread_scope(app)));
        if form.dm_kind != DmKind::All {
            lines.push(scope_row("Conversation type", form.dm_kind.label().into()));
        }
    }
    if form.content.dms && form.dm_media_only {
        lines.push(scope_row(
            "Media only",
            "keeps text, Reels and shared posts".into(),
        ));
    }
    if form.content.dms && form.hide_threads {
        let hides = !form.dm_media_only && !has_dates && limit == 0;
        lines.push(scope_row(
            "Hide threads",
            if hides {
                "those left with none of your messages".into()
            } else {
                "skipped with these filters".into()
            },
        ));
    }
    let protected = thread_id_list(&form.protect_users);
    if !protected.is_empty() {
        let value = format!("{} ({})", protected.len(), protected.join(", "));
        let room = width.saturating_sub("Protected: ".len());
        lines.push(scope_row("Protected", ellipsize(&value, room)));
    }
    lines
}

fn confirm_keys() -> Line<'static> {
    Line::from(vec![
        Span::styled("Enter", theme::key()),
        Span::styled(" or ", theme::dim()),
        Span::styled("y", theme::key()),
        Span::styled(" to delete, ", theme::dim()),
        Span::styled("Esc", theme::key()),
        Span::styled(" or ", theme::dim()),
        Span::styled("n", theme::key()),
        Span::styled(" to cancel.", theme::dim()),
    ])
}

/// Rows a line takes once wrapped; words moving to the next row can add one more.
fn wrapped_rows(line: &Line, width: usize) -> u16 {
    let rows = line.width().div_ceil(width.max(1)).max(1);
    let spill = usize::from(rows > 1);
    (rows + spill) as u16
}

fn draw_confirm(f: &mut Frame, app: &AppState, area: Rect) {
    const BORDERS: u16 = 2;
    const SPACER: u16 = 1;

    let win_width = (area.width * 64 / 100).max(50).min(area.width);
    let inner_width = win_width.saturating_sub(BORDERS) as usize;
    let lines = confirm_lines(app, inner_width);
    let keys = confirm_keys();

    let keys_rows = wrapped_rows(&keys, inner_width);
    let body_rows: u16 = lines.iter().map(|l| wrapped_rows(l, inner_width)).sum();
    let win_height = (body_rows + SPACER + keys_rows + BORDERS).min(area.height);
    let win = Rect {
        x: area.x + (area.width - win_width) / 2,
        y: area.y + (area.height - win_height) / 2,
        width: win_width,
        height: win_height,
    };

    f.render_widget(Clear, win);
    let block = panel(
        Span::styled(
            " Confirm Permanent Deletion ",
            theme::err().add_modifier(Modifier::BOLD),
        ),
        true,
    );
    let inner = block.inner(win);
    f.render_widget(block, win);

    // the keys are pinned to the bottom, so a tiny terminal can't cut off the way out
    let parts = split_v(inner, &[Constraint::Min(0), Constraint::Length(keys_rows)]);
    f.render_widget(
        Paragraph::new(lines).style(theme::base()).wrap(wrap()),
        parts[0],
    );
    f.render_widget(
        Paragraph::new(keys).style(theme::base()).wrap(wrap()),
        parts[1],
    );
}

fn draw_help(f: &mut Frame, area: Rect) {
    let win = centered_rect(68, 90, area);
    f.render_widget(Clear, win);
    let block = panel(" Keyboard Shortcuts · Esc closes ", true);
    let inner = block.inner(win);
    f.render_widget(block, win);

    let width = inner.width as usize;
    let mut lines = Vec::new();
    for (title, items) in keys::HELP_SECTIONS {
        let prefix = format!("── {title} ");
        let rule = format!(
            "{prefix}{}",
            "─".repeat(width.saturating_sub(prefix.chars().count()))
        );
        lines.push(Line::from(Span::styled(
            rule,
            theme::accent().add_modifier(Modifier::BOLD),
        )));
        for (key, desc) in *items {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<20}"), theme::key()),
                Span::styled(*desc, theme::base()),
            ]));
        }
    }

    f.render_widget(Paragraph::new(lines).style(theme::base()), inner);
}
