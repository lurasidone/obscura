use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, TableState};
use ratatui::Frame;

use super::super::state::{AppState, Connection};
use super::super::theme;
use super::super::widgets::{ellipsize, panel, split_v};
use crate::model::Channel;
pub fn draw(f: &mut Frame, app: &AppState, area: Rect) {
    let parts = split_v(area, &[Constraint::Min(8), Constraint::Length(5)]);
    draw_list(f, app, parts[0]);
    draw_summary(f, app, parts[1]);
}

fn recipients(t: &Channel) -> String {
    t.display_name()
}

fn channel_kind(channel: &Channel) -> &'static str {
    if channel.is_group_dm() {
        "group"
    } else {
        "1on1"
    }
}

fn draw_list(f: &mut Frame, app: &AppState, area: Rect) {
    let count = app.targets.filtered_threads(app.form.dm_kind).count();
    let title = format!(
        " Discord channels ({count} available) · Space pick · P protect · Enter copy to scope "
    );
    let block = panel(title, true);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if app.targets.loading {
        let msg = Paragraph::new("Loading your Discord DMs…").style(theme::accent());
        f.render_widget(msg, inner);
        return;
    }

    if count == 0 {
        let msg = Paragraph::new(
            "No channels found matching filter. Press 'R' to reconnect or check DM kind in Settings.",
        )
        .style(theme::dim());
        f.render_widget(msg, inner);
        return;
    }

    let header_cells = [
        "",
        "Sel",
        "Prot",
        "Type",
        "Title / Participants",
        "Kind / guild",
        "Channel ID",
    ]
    .into_iter()
    .map(|h| Cell::from(Span::styled(h, theme::strong())));
    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app
        .targets
        .filtered_threads(app.form.dm_kind)
        .enumerate()
        .map(|(i, channel)| {
            let is_selected_cursor = i == app.targets.thread_cursor;
            let is_checked = app.targets.selected_threads.contains(&channel.id);
            let is_prot = app.targets.is_protected(channel);

            let pointer = if is_selected_cursor { "▶" } else { " " };
            let sel_box = if is_checked { "[✓]" } else { "[ ]" };
            let prot_tag = if is_prot { "[P]" } else { "   " };
            let type_tag = channel_kind(channel);

            let title_str = ellipsize(&recipients(channel), 26);
            let snippet = if channel.is_dm_like() {
                "DM".to_string()
            } else {
                channel.guild_id.clone().unwrap_or_default()
            };

            let style = if is_selected_cursor {
                theme::selected()
            } else {
                theme::base()
            };

            Row::new(vec![
                Cell::from(Span::styled(pointer, theme::accent())),
                Cell::from(Span::styled(
                    sel_box,
                    if is_checked {
                        theme::ok().add_modifier(Modifier::BOLD)
                    } else {
                        theme::dim()
                    },
                )),
                Cell::from(Span::styled(
                    prot_tag,
                    if is_prot {
                        theme::warn().add_modifier(Modifier::BOLD)
                    } else {
                        theme::dim()
                    },
                )),
                Cell::from(Span::styled(type_tag, theme::dim())),
                Cell::from(Span::styled(title_str, theme::strong())),
                Cell::from(Span::styled(snippet, theme::dim())),
                Cell::from(Span::styled(channel.id.clone(), theme::dim())),
            ])
            .style(style)
        });

    let widths = [
        Constraint::Length(1),
        Constraint::Length(4),
        Constraint::Length(4),
        Constraint::Length(6),
        Constraint::Length(28),
        Constraint::Min(20),
        Constraint::Length(22),
    ];

    let mut state = TableState::default();
    state.select(Some(app.targets.thread_cursor));
    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(theme::selected());
    f.render_stateful_widget(table, inner, &mut state);
}

fn draw_summary(f: &mut Frame, app: &AppState, area: Rect) {
    let block = panel("Selection & Status", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sel_count = app.targets.selected_threads.len();
    let prot_count = app.targets.protected_users.len();
    let channels_count = app.targets.threads.len();

    let conn_span = match (&app.connection, &app.identity) {
        (Connection::Ready, Some(user)) => {
            Span::styled(format!("Connected as @{user}"), theme::ok())
        }
        (Connection::Connecting, _) => Span::styled("Connecting to Discord…", theme::warn()),
        (Connection::Failed, _) => Span::styled("Offline / probe failed", theme::err()),
        (Connection::Idle, _) => Span::styled("Not connected (press R to probe)", theme::dim()),
        _ => Span::styled("Connected", theme::ok()),
    };

    let line1 = Line::from(vec![
        Span::styled("Account: ", theme::dim()),
        conn_span,
        Span::raw("   ·   "),
        Span::styled(format!("{channels_count} channels loaded"), theme::base()),
        Span::raw("   ·   "),
        Span::styled(
            format!("{sel_count} selected"),
            if sel_count > 0 {
                theme::ok().add_modifier(Modifier::BOLD)
            } else {
                theme::dim()
            },
        ),
        Span::raw("   ·   "),
        Span::styled(format!("{prot_count} protected"), theme::warn()),
    ]);

    let line2 = match (app.has_picker_override(), sel_count) {
        (true, _) => Line::from(vec![
            Span::styled("Run: ", theme::dim()),
            Span::styled(
                format!("only {} picked channel(s)", app.picked_threads_len()),
                theme::ok().add_modifier(Modifier::BOLD),
            ),
            Span::styled(". Scope ignored.", theme::dim()),
        ]),
        (false, 0) => Line::from(vec![
            Span::styled("Run: ", theme::dim()),
            Span::styled("the scope decides; ", theme::dim()),
            Span::styled("Space", theme::key()),
            Span::styled(" picks a channel.", theme::dim()),
        ]),
        (false, n) => Line::from(vec![
            Span::styled("Run: ", theme::dim()),
            Span::styled(
                format!("{} scope decides; ", app.form.scope.kind.label()),
                theme::dim(),
            ),
            Span::styled("Enter", theme::key()),
            Span::styled(format!(" copies {n} picked."), theme::dim()),
        ]),
    };

    f.render_widget(
        Paragraph::new(vec![line1, line2]).style(theme::base()),
        inner,
    );
}
