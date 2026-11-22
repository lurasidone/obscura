use ratatui::layout::Rect;
use ratatui::Frame;

use super::super::state::AppState;

pub fn draw(f: &mut Frame, app: &AppState, area: Rect) {
    let archive = &app.archive;
    obscura_core::tui::archive::draw(
        f,
        area,
        obscura_core::tui::archive::ArchiveViewData {
            days: &archive.days,
            day_cursor: archive.day_cursor,
            channels: &archive.channels,
            channel_cursor: archive.channel_cursor,
            records: &archive.records,
            record_cursor: archive.record_cursor,
            focus: archive.focus,
            scope_label: archive.scope.label(),
        },
    );
}
