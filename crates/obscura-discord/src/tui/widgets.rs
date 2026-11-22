use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::Frame;

use super::state::{AppState, Connection};
use super::theme;

pub use obscura_core::tui::widgets::{
    badge, centered_rect, clock, ellipsize, hints, human_duration, kv, level_tag, panel, split,
    split_v, tab_line, wrap,
};

pub fn status_spans(app: &AppState) -> Vec<Span<'static>> {
    let mut spans: Vec<Span> = Vec::new();
    if app.form.dry_run {
        spans.push(badge("DRY RUN", theme::warn()));
        spans.push(Span::raw(" "));
    }
    if app.run.is_running() {
        let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        let phase = match app.run.phase {
            crate::events::Phase::Scan => "scanning",
            crate::events::Phase::Delete => "deleting",
        };
        let idx = (app.run.plan_done + app.run.channels.len()) % frames.len();
        spans.push(Span::styled(
            format!("{} {phase} ", frames[idx]),
            theme::accent().add_modifier(Modifier::BOLD),
        ));
    }
    let (text, style) = match (&app.connection, &app.identity) {
        (Connection::Ready, Some(user)) => (format!("@{user}"), theme::ok()),
        (Connection::Ready, None) => ("connected".to_string(), theme::ok()),
        (Connection::Connecting, _) => ("connecting…".to_string(), theme::warn()),
        (Connection::Failed, _) => ("offline".to_string(), theme::err()),
        (Connection::Idle, _) => ("not connected".to_string(), theme::dim()),
    };
    spans.push(Span::styled(text, style));
    spans
}

pub fn draw_header(f: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    obscura_core::tui::widgets::draw_header(f, area, tab_line(app.screen), status_spans(app));
}

pub fn draw_footer(f: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    let hints = footer_hints(app);
    let toast = app
        .toast
        .as_ref()
        .map(|toast| (toast.text.as_str(), toast.level));
    obscura_core::tui::widgets::draw_footer(f, area, hints, toast);
}

pub fn footer_hints(app: &AppState) -> Line<'static> {
    hints(super::keys::footer_hints(app.screen, app.run.is_running()))
}
