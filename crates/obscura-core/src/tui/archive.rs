//! Shared archive screen for the platform TUIs.

use chrono::NaiveDate;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::history::{ChannelRef, DeletedRecord};
use crate::theme;

use super::screen::ArchiveFocus;
use super::widgets::{ellipsize, panel, split, split_v, wrap};

pub struct ArchiveViewData<'a> {
    pub days: &'a [NaiveDate],
    pub day_cursor: usize,
    pub channels: &'a [ChannelRef],
    pub channel_cursor: usize,
    pub records: &'a [DeletedRecord],
    pub record_cursor: usize,
    pub focus: ArchiveFocus,
    pub scope_label: &'a str,
}

pub fn draw(f: &mut Frame, area: Rect, data: ArchiveViewData<'_>) {
    let parts = split_v(area, &[Constraint::Min(8), Constraint::Length(6)]);
    let top = split(
        parts[0],
        &[
            Constraint::Length(18),
            Constraint::Length(26),
            Constraint::Min(30),
        ],
    );
    draw_days(f, data.days, data.day_cursor, data.focus, top[0]);
    draw_channels(f, data.channels, data.channel_cursor, data.focus, top[1]);
    draw_records(
        f,
        data.records,
        data.record_cursor,
        data.focus,
        data.scope_label,
        top[2],
    );
    draw_detail(f, data.records.get(data.record_cursor), parts[1]);
}

fn draw_days(f: &mut Frame, days: &[NaiveDate], cursor: usize, focus: ArchiveFocus, area: Rect) {
    let is_focused = focus == ArchiveFocus::Days;
    let block = panel(format!(" Days ({}) ", days.len()), is_focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items = days
        .iter()
        .enumerate()
        .map(|(i, day)| {
            let is_current = i == cursor;
            let style = selected_style(is_current, is_focused);
            ListItem::new(Line::from(vec![
                Span::styled(pointer(is_current), theme::accent()),
                Span::styled(day.to_string(), style),
            ]))
        })
        .collect();
    draw_list(f, items, cursor, inner);
}

fn draw_channels(
    f: &mut Frame,
    channels: &[ChannelRef],
    cursor: usize,
    focus: ArchiveFocus,
    area: Rect,
) {
    let is_focused = focus == ArchiveFocus::Channels;
    let block = panel(format!(" Targets ({}) ", channels.len()), is_focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items = channels
        .iter()
        .enumerate()
        .map(|(i, channel)| {
            let is_current = i == cursor;
            ListItem::new(Line::from(vec![
                Span::styled(pointer(is_current), theme::accent()),
                Span::styled(
                    ellipsize(&channel.name, 18),
                    selected_style(is_current, is_focused),
                ),
            ]))
        })
        .collect();
    draw_list(f, items, cursor, inner);
}

fn draw_records(
    f: &mut Frame,
    records: &[DeletedRecord],
    cursor: usize,
    focus: ArchiveFocus,
    scope_label: &str,
    area: Rect,
) {
    let is_focused = focus == ArchiveFocus::Records;
    let title = format!(
        " Records ({}) · showing {scope_label} · [ ] to change ",
        records.len()
    );
    let block = panel(title, is_focused);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let items = records
        .iter()
        .enumerate()
        .map(|(i, record)| {
            let is_current = i == cursor;
            let outcome_tag = format!("[{}]", record.outcome.label());
            let preview = if !record.preview.is_empty() {
                &record.preview
            } else if !record.content.is_empty() {
                &record.content
            } else {
                &record.item_id
            };
            ListItem::new(Line::from(vec![
                Span::styled(pointer(is_current), theme::accent()),
                Span::styled(format!("{outcome_tag:<13}"), theme::outcome(record.outcome)),
                Span::styled(
                    format!(
                        " {} ",
                        record
                            .deleted_at
                            .with_timezone(&chrono::Local)
                            .format("%H:%M:%S")
                    ),
                    theme::dim(),
                ),
                Span::styled(
                    ellipsize(preview, 36),
                    selected_style(is_current, is_focused),
                ),
            ]))
        })
        .collect();
    draw_list(f, items, cursor, inner);
}

fn draw_detail(f: &mut Frame, record: Option<&DeletedRecord>, area: Rect) {
    let block = panel("Record · y copies JSON", false);
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(record) = record {
        f.render_widget(
            Paragraph::new(detail_lines(record))
                .style(theme::base())
                .wrap(wrap()),
            inner,
        );
    } else {
        f.render_widget(
            Paragraph::new("Select a record to see its details.").style(theme::dim()),
            inner,
        );
    }
}

fn detail_lines(record: &DeletedRecord) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled("Item ID: ", theme::dim()),
        Span::styled(record.item_id.clone(), theme::strong()),
        Span::raw("   ·   "),
        Span::styled("Outcome: ", theme::dim()),
        Span::styled(
            record.outcome.label(),
            theme::outcome(record.outcome).add_modifier(Modifier::BOLD),
        ),
        Span::raw("   ·   "),
        Span::styled("Time: ", theme::dim()),
        Span::styled(record.timestamp.clone(), theme::base()),
        Span::raw("   ·   "),
        Span::styled("Target: ", theme::dim()),
        Span::styled(record.target_name.clone(), theme::strong()),
    ])];

    if !record.author_name.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Author: ", theme::dim()),
            Span::styled(format!("@{}", record.author_name), theme::accent()),
        ]));
    }
    if !record.content.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Content: ", theme::dim()),
            Span::styled(record.content.clone(), theme::base()),
        ]));
    }
    if let Some(attachment) = record.attachments.first() {
        lines.push(Line::from(vec![
            Span::styled("Attachment: ", theme::dim()),
            Span::styled(attachment.url.clone(), theme::dim()),
        ]));
    }
    lines
}

fn draw_list(f: &mut Frame, items: Vec<ListItem>, cursor: usize, area: Rect) {
    let mut state = ListState::default().with_selected(Some(cursor));
    f.render_stateful_widget(List::new(items).style(theme::base()), area, &mut state);
}

fn pointer(current: bool) -> &'static str {
    if current {
        "▶ "
    } else {
        "  "
    }
}

fn selected_style(current: bool, focused: bool) -> ratatui::style::Style {
    if current && focused {
        theme::selected()
    } else if current {
        theme::strong()
    } else {
        theme::base()
    }
}
