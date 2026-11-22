use ratatui::backend::TestBackend;
use ratatui::Terminal;

use crate::config::Config;
use crate::events::{CleanerEvent, LogLevel};
use crate::history::DeleteOutcome;
use crate::tui::action::{Overlay, Screen};
use crate::tui::state::AppState;
use crate::tui::views;

fn render(app: &AppState, width: u16, height: u16) -> Vec<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("create test terminal");
    terminal
        .draw(|f| views::render(f, app))
        .expect("draw to test terminal");
    buffer_lines(terminal.backend())
}

fn buffer_lines(backend: &TestBackend) -> Vec<String> {
    let buffer = backend.buffer();
    let mut lines = Vec::new();
    for y in 0..buffer.area.height {
        let mut line = String::new();
        for x in 0..buffer.area.width {
            let cell = buffer.cell((x, y)).expect("valid cell");
            line.push_str(cell.symbol());
        }
        lines.push(line);
    }
    lines
}

fn text(lines: &[String]) -> String {
    lines.join("\n")
}

const SIZES: [(u16, u16); 3] = [(60, 20), (100, 32), (160, 45)];

fn app_with_data() -> AppState {
    let mut cfg = Config::from_env();
    cfg.sessionid = "fake_sessionid_123456789".into();
    let mut app = AppState::new(cfg);
    app.connection = crate::tui::state::Connection::Ready;
    app.identity = Some("my_user".into());
    app.targets.threads = vec![crate::model::DirectThread {
        thread_id: "thread_100".into(),
        pk: Some("thread_100".into()),
        thread_title: Some("alice_and_bob".into()),
        is_group: false,
        users: vec![crate::model::UserShort {
            pk: "999".into(),
            username: "alice".into(),
            full_name: "Alice A".into(),
        }],
        items: vec![crate::model::DirectMessage {
            item_id: "msg_1".into(),
            id: None,
            user_id: Some("999".into()),
            timestamp: 1700000000000000,
            item_type: Some("text".into()),
            text: Some("hello world".into()),
            client_context: None,
            extra: Default::default(),
        }],
        has_older: false,
        prev_cursor: None,
        oldest_cursor: None,
    }];
    app.apply_event(CleanerEvent::Started { channel_count: 1 });
    app.apply_event(CleanerEvent::PlanReady { total: 2 });
    app.apply_event(CleanerEvent::ChannelStarted {
        channel_id: "thread_100".into(),
        channel_name: "alice".into(),
        index: 0,
        total: 1,
        phase: crate::events::Phase::Delete,
    });
    app.apply_event(CleanerEvent::MessageDeleted {
        preview: "hello world".into(),
        outcome: DeleteOutcome::Deleted,
    });
    app.apply_event(CleanerEvent::Log {
        level: LogLevel::Warning,
        message: "a warning line".into(),
    });
    app.run.status = crate::tui::state::RunStatus::Running;
    app.archive.days = vec![chrono::Utc::now().date_naive()];
    app.archive.records = vec![record(DeleteOutcome::Deleted)];
    app
}

fn record(outcome: DeleteOutcome) -> crate::history::DeletedRecord {
    crate::history::DeletedRecord {
        deleted_at: chrono::Utc::now(),
        outcome,
        target_id: "thread_100".into(),
        target_name: "alice".into(),
        item_id: "msg_1".into(),
        kind: "dm".into(),
        author_id: "12345".into(),
        author_name: "my_user".into(),
        content: "archived body".into(),
        timestamp: "2026-09-17T12:00:00Z".into(),
        media_type: None,
        attachments: Vec::new(),
        preview: "archived body".into(),
    }
}

#[test]
fn every_screen_renders_at_every_size() {
    for screen in Screen::ALL {
        for (w, h) in SIZES {
            let mut app = app_with_data();
            app.screen = screen;
            let lines = render(&app, w, h);
            let body = text(&lines);
            assert!(
                body.contains(screen.title()),
                "{screen:?} at {w}x{h} did not draw its tab:\n{body}"
            );
            assert!(
                body.trim().len() > 40,
                "{screen:?} at {w}x{h} rendered almost nothing:\n{body}"
            );
        }
    }
}

#[test]
fn settings_screen_shows_fields_and_validation_state() {
    let app = app_with_data();
    let lines = render(&app, 100, 32);
    let body = text(&lines);
    assert!(body.contains("Session ID"));
    assert!(body.contains("What to delete"));
    assert!(body.contains("DM messages"));
}

#[test]
fn warnings_are_counted_on_the_run_screen_and_named_in_the_archive() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.apply_event(CleanerEvent::Warning {
        message: "WARNING @someone: could not check, kept".into(),
    });
    let body = text(&render(&app, 140, 40));
    assert!(body.contains("Warnings: 1"), "{body}");
    assert!(body.contains("WARNING @someone"), "{body}");

    app.screen = Screen::Archive;
    app.archive.records = vec![record(DeleteOutcome::Warning)];
    let body = text(&render(&app, 140, 40));
    assert!(body.contains("[WARNING]"), "{body}");
}

#[test]
fn settings_keeps_the_focused_content_toggle_on_screen() {
    let mut app = app_with_data();
    app.screen = Screen::Settings;
    app.settings.cursor = crate::tui::action::Field::ALL
        .iter()
        .position(|f| *f == crate::tui::action::Field::ContentReposts)
        .expect("content field present");

    let lines = render(&app, 100, 20);
    let body = text(&lines);
    assert!(
        body.contains("Reposts"),
        "focused content toggle scrolled off screen:\n{body}"
    );

    app.settings.cursor = 0;
    let body = text(&render(&app, 100, 20));
    assert!(
        body.contains("Session ID"),
        "top rows not restored:\n{body}"
    );
}

#[test]
fn settings_editor_renders_unicode_cursor_safely() {
    let mut app = app_with_data();
    app.screen = Screen::Settings;
    app.form.before = "😀date".into();
    app.settings.cursor = crate::tui::action::Field::ALL
        .iter()
        .position(|field| *field == crate::tui::action::Field::Before)
        .expect("before field present");
    app.settings.start_edit("😀date");
    app.settings.buffer_cursor = 1;

    let body = text(&render(&app, 100, 24));
    assert!(body.contains("date"));
}

#[test]
fn run_screen_shows_progress_stats_and_log() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.form.dry_run = false;
    let lines = render(&app, 100, 32);
    let body = text(&lines);
    assert!(body.contains("Status:"));
    assert!(body.contains("Scanned:"));
    assert!(body.contains("Found: 2"));
    assert!(body.contains("Deleted: 1"));
    assert!(body.contains("Skipped:"));
    assert!(body.contains("Failed:"));
    assert!(body.contains("Warnings:"));
    assert!(body.contains("del:1"));
    assert!(body.contains("Activity Log"));
}

#[test]
fn dry_run_says_would_delete_instead_of_deleted() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.run.dry_run = true;
    let body = text(&render(&app, 100, 32));
    assert!(body.contains("Would delete: 1"));
    assert!(!body.contains("Deleted:"));
}

#[test]
fn a_live_run_never_wears_the_dry_run_badge() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.form.dry_run = true;
    app.run.dry_run = false;
    let body = text(&render(&app, 100, 32));
    assert!(!body.contains("DRY RUN"), "{body}");
    assert!(body.contains("Deleted: 1"), "{body}");
}

#[test]
fn a_stopped_run_shows_what_it_never_tried() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.apply_event(CleanerEvent::Done {
        stats: crate::events::RunStats {
            was_cancelled: true,
            plan_total: 5,
            messages_deleted: 1,
            ..Default::default()
        },
    });
    let body = text(&render(&app, 100, 32));
    assert!(body.contains("Not attempted: 4"), "{body}");
}

#[test]
fn the_archive_scrolls_to_keep_the_cursor_on_screen() {
    let mut app = app_with_data();
    app.screen = Screen::Archive;
    app.archive.records = (0..200)
        .map(|i| {
            let mut rec = record(DeleteOutcome::Deleted);
            rec.preview = format!("record-number-{i}");
            rec
        })
        .collect();
    app.archive.record_cursor = 150;
    let body = text(&render(&app, 140, 30));
    assert!(body.contains("record-number-150"), "{body}");
}

#[test]
fn target_counts_survive_a_narrow_screen() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.run.channels[0].name = "a very long thread name indeed".into();
    let body = text(&render(&app, 80, 24));
    assert!(body.contains("del:1"));
}

#[test]
fn targets_screen_lists_threads() {
    let mut app = app_with_data();
    app.screen = Screen::Targets;
    let lines = render(&app, 100, 32);
    let body = text(&lines);
    assert!(body.contains("DM threads"));
    assert!(body.contains("alice"));
}

#[test]
fn archive_screen_shows_days_and_records() {
    let mut app = app_with_data();
    app.screen = Screen::Archive;
    let lines = render(&app, 100, 32);
    let body = text(&lines);
    assert!(body.contains("Days"));
    assert!(body.contains("Targets"));
    assert!(body.contains("Records"));
}

#[test]
fn overlays_render_and_cover_the_screen() {
    for overlay in [Overlay::Help, Overlay::Palette, Overlay::Confirm] {
        let mut app = app_with_data();
        app.overlay = overlay;
        let lines = render(&app, 100, 32);
        let body = text(&lines);
        match overlay {
            Overlay::Help => assert!(body.contains("Keyboard Shortcuts")),
            Overlay::Palette => assert!(body.contains("Command palette")),
            Overlay::Confirm => {
                assert!(body.contains("Confirm Permanent Deletion"));
                assert!(body.contains("to cancel"), "confirm keys cut off:\n{body}");
            }
            Overlay::None => {}
        }
    }
}

#[test]
fn help_shows_every_section_on_a_normal_terminal() {
    let mut app = app_with_data();
    app.overlay = Overlay::Help;
    let body = text(&render(&app, 100, 32));
    for (title, items) in crate::tui::keys::HELP_SECTIONS {
        assert!(body.contains(title), "{title} cut off:\n{body}");
        for (_, desc) in *items {
            assert!(body.contains(desc), "{desc:?} cut off:\n{body}");
        }
    }
}

#[test]
fn confirm_shows_everything_that_decides_what_goes() {
    let mut app = app_with_data();
    app.overlay = Overlay::Confirm;
    app.form.content.posts = true;
    app.form.content.dms = true;
    app.form.before = "2024-01-01".into();
    app.form.after = "2020-01-01".into();
    app.form.limit = "20".into();
    app.form.dm_kind = crate::config::DmKind::Group;
    app.form.dm_media_only = true;
    app.form.hide_threads = true;
    app.form.protect_users = "alice, bob".into();

    for (w, h) in [(60, 20), (100, 32)] {
        let body = text(&render(&app, w, h));
        for expected in [
            "older than",
            "first 20 items",
            "Media only",
            "Hide threads",
            "Esc",
        ] {
            assert!(body.contains(expected), "{expected} at {w}x{h}:\n{body}");
        }
    }
    let body = text(&render(&app, 100, 32));
    assert!(
        body.contains("newer than 2020-01-01, older than 2024-01-01"),
        "{body}"
    );
    assert!(body.contains("skipped with these filters"), "{body}");
    assert!(body.contains("2 (alice, bob)"), "{body}");
}

#[test]
fn confirm_says_when_nothing_narrows_the_run() {
    let mut app = app_with_data();
    app.overlay = Overlay::Confirm;
    app.form.content.dms = true;
    app.form.limit = "0".into();
    app.form.before = String::new();
    app.form.after = String::new();
    app.form.threads = String::new();
    let body = text(&render(&app, 100, 32));
    assert!(body.contains("all dates, no filter"), "{body}");
    assert!(body.contains("none, everything found"), "{body}");
    assert!(body.contains("whole inbox"), "{body}");
}

#[test]
fn confirm_does_not_promise_an_unfollow_the_dates_will_skip() {
    let mut app = app_with_data();
    app.overlay = Overlay::Confirm;
    app.form.content.activity.unfollow = true;
    app.form.before = "2024-01-01".into();
    let body = text(&render(&app, 100, 32));
    assert!(body.contains("Unfollow is skipped"), "{body}");
}

#[test]
fn confirm_fits_a_terminal_too_small_for_it() {
    let mut app = app_with_data();
    app.overlay = Overlay::Confirm;
    app.form.content.dms = true;
    app.form.protect_users = "a, b, c, d, e, f".into();
    let body = text(&render(&app, 40, 8));
    assert!(body.contains("Esc"), "the way out was cut off:\n{body}");
}

#[test]
fn an_empty_result_with_warnings_does_not_say_nothing_to_delete() {
    let mut app = app_with_data();
    app.screen = Screen::Run;
    app.run.status = crate::tui::state::RunStatus::Completed;
    app.run.plan_total = 0;
    app.run.warnings = 1;
    let body = text(&render(&app, 100, 32));
    assert!(body.contains("check the warnings"), "{body}");
}

#[test]
fn confirm_shows_its_keys_on_a_small_terminal() {
    let mut app = app_with_data();
    app.overlay = Overlay::Confirm;
    app.form.content.posts = true;
    app.form.content.dms = true;
    let body = text(&render(&app, 60, 20));
    assert!(body.contains("posts, dms"), "{body}");
    assert!(body.contains("Esc"), "cancel key cut off:\n{body}");
}
