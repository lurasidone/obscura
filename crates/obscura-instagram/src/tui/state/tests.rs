use crate::cleaner::TargetResolution;
use crate::config::Config;
use crate::events::{ChannelStats, CleanerEvent, Phase, RetryCount, RunStats};
use crate::history::DeleteOutcome;
use crate::model::DirectThread;
use std::time::Duration;

use super::super::action::{self, Action, Effect, Field, Overlay, Screen};
use super::{mask_sessionid, AppState, Form, RunStatus};

#[test]
fn form_validation_checks_sessionid_and_dates() {
    let mut form = Form::from_config(&Config::from_env());
    form.sessionid = String::new();
    let errs = form.validate();
    assert!(errs.contains_key(&Field::SessionId));

    form.sessionid = "fake_sessionid".to_string();
    form.before = "invalid_date".to_string();
    let errs = form.validate();
    assert!(errs.contains_key(&Field::Before));

    form.before = "2026-01-01".to_string();
    let errs = form.validate();
    assert!(!errs.contains_key(&Field::Before));
}

#[test]
fn settings_navigation_wraps_around_every_field() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Settings;
    assert_eq!(app.settings.field(), Field::ALL[0]);

    app.apply_action(Action::Up);
    assert_eq!(app.settings.field(), *Field::ALL.last().unwrap());

    app.apply_action(Action::Down);
    assert_eq!(app.settings.field(), Field::ALL[0]);
}

#[test]
fn enter_on_targets_starts_a_run_with_the_picks() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Targets;
    app.form.sessionid = "fake_sessionid".to_string();
    app.form.dry_run = true;
    app.form.content.dms = true;
    app.targets.threads = vec![thread("thread_123")];
    app.targets
        .selected_threads
        .insert("thread_123".to_string());

    assert!(matches!(
        app.apply_action(Action::Activate).as_slice(),
        [Effect::StartRun(_)]
    ));
    assert_eq!(app.screen, Screen::Run);
}

#[test]
fn selecting_quit_from_command_palette_returns_quit_effect() {
    let mut app = AppState::new(Config::from_env());
    app.overlay = Overlay::Palette;
    app.palette.matches = action::commands()
        .iter()
        .enumerate()
        .filter(|(_, command)| command.id == "quit")
        .map(|(index, _)| index)
        .collect();
    app.palette.cursor = 0;

    assert!(matches!(
        app.apply_action(Action::PaletteSelect).as_slice(),
        [Effect::Quit]
    ));
}

#[test]
fn active_run_cannot_be_started_again() {
    let mut app = AppState::new(Config::from_env());
    app.form.sessionid = "fake_sessionid".into();
    app.form.dry_run = true;
    app.form.content.posts = true;
    app.run.status = RunStatus::Running;

    assert!(app.apply_action(Action::Activate).is_empty());
    assert!(app
        .toast
        .as_ref()
        .is_some_and(|toast| toast.text.contains("already active")));
}

fn thread(id: &str) -> DirectThread {
    DirectThread {
        thread_id: id.to_string(),
        pk: None,
        thread_title: None,
        is_group: false,
        users: Vec::new(),
        items: Vec::new(),
        has_older: false,
        prev_cursor: None,
        oldest_cursor: None,
    }
}

fn run_targets(app: &mut AppState) -> Vec<TargetResolution> {
    app.form.sessionid = "fake_sessionid".to_string();
    app.form.dry_run = true;
    app.form.limit = "0".to_string();
    app.form.throttle_ms = "2000".to_string();
    app.form.scan_throttle_ms = "350".to_string();
    app.screen = Screen::Settings;
    let effects = app.apply_action(Action::Activate);
    match effects.into_iter().next() {
        Some(Effect::StartRun(cfg)) => cfg.targets,
        other => panic!("expected StartRun, got {other:?}"),
    }
}

#[test]
fn picks_narrow_dms_and_keep_posts() {
    let mut app = AppState::new(Config::from_env());
    app.form.content.posts = true;
    app.form.content.dms = true;
    app.targets.threads = vec![thread("t1"), thread("t2"), thread("t3")];
    app.targets.selected_threads.insert("t3".to_string());
    app.targets.selected_threads.insert("t1".to_string());

    match run_targets(&mut app).as_slice() {
        [TargetResolution::Posts, TargetResolution::ThreadObjects(picked)] => {
            let ids: Vec<&str> = picked.iter().map(|t| t.thread_id.as_str()).collect();
            assert_eq!(ids, ["t1", "t3"]);
        }
        other => panic!("expected posts and the picked threads, got {other:?}"),
    }
}

#[test]
fn no_picks_means_the_whole_inbox() {
    let mut app = AppState::new(Config::from_env());
    app.form.content.posts = true;
    app.form.content.dms = true;
    app.targets.threads = vec![thread("t1")];

    let targets = run_targets(&mut app);
    assert!(matches!(
        targets.as_slice(),
        [TargetResolution::Posts, TargetResolution::AllDms]
    ));
}

#[test]
fn typed_thread_ids_beat_stale_picks() {
    let mut app = AppState::new(Config::from_env());
    app.form.content.dms = true;
    app.form.threads = "typed_1, typed_2".to_string();
    app.targets.threads = vec![thread("t1")];
    app.targets.selected_threads.insert("t1".to_string());

    match run_targets(&mut app).as_slice() {
        [TargetResolution::Threads(ids)] => assert_eq!(ids, &["typed_1", "typed_2"]),
        other => panic!("expected the typed ids to win, got {other:?}"),
    }
}

#[test]
fn picks_do_nothing_without_dms_or_reactions() {
    let mut app = AppState::new(Config::from_env());
    app.form.content.posts = true;
    app.targets.threads = vec![thread("t1")];
    app.targets.selected_threads.insert("t1".to_string());

    assert!(matches!(
        run_targets(&mut app).as_slice(),
        [TargetResolution::Posts]
    ));
}

fn form_with(content: &str) -> Form {
    let mut form = Form::from_config(&Config::from_env());
    form.sessionid = "fake_sessionid".to_string();
    form.content = crate::config::parse_content_value(content).unwrap();
    form
}

#[test]
fn reactions_without_dms_walk_threads_but_never_unsend() {
    let cfg = form_with("reactions").to_cleaner_config().unwrap();
    assert!(matches!(cfg.targets.as_slice(), [TargetResolution::AllDms]));
    assert!(cfg.content.dm_reactions);
    assert!(!cfg.unsend_dms);
}

#[test]
fn reactions_with_dms_also_unsend() {
    let cfg = form_with("dms, reactions").to_cleaner_config().unwrap();
    assert!(cfg.content.dm_reactions);
    assert!(cfg.unsend_dms);
}

#[test]
fn dms_alone_unsend_without_touching_reactions() {
    let cfg = form_with("dms").to_cleaner_config().unwrap();
    assert!(!cfg.content.dm_reactions);
    assert!(cfg.unsend_dms);
}

#[test]
fn activity_classes_alone_walk_no_threads() {
    let cfg = form_with("likes").to_cleaner_config().unwrap();
    assert!(cfg.targets.is_empty());
    assert!(!cfg.unsend_dms);
}

#[test]
fn typed_threads_require_dms_or_reactions() {
    let mut form = form_with("posts");
    form.threads = "123".to_string();
    assert!(form.validate().contains_key(&Field::Threads));

    form.content.activity.dm_reactions = true;
    assert!(!form.validate().contains_key(&Field::Threads));
}

#[test]
fn refreshed_inbox_drops_unresolvable_picks() {
    let mut app = AppState::new(Config::from_env());
    app.form.content.dms = true;
    app.targets.threads = vec![thread("t1"), thread("t2")];
    app.targets.selected_threads.insert("t1".to_string());
    app.targets.selected_threads.insert("t2".to_string());

    app.apply_event(CleanerEvent::DMsLoaded {
        channels: vec![thread("t2")],
    });

    assert_eq!(
        app.targets.selected_threads.iter().collect::<Vec<_>>(),
        vec!["t2"]
    );

    match run_targets(&mut app).as_slice() {
        [TargetResolution::ThreadObjects(picked)] => assert_eq!(picked.len(), 1),
        other => panic!("expected the surviving pick, got {other:?}"),
    }
}

fn running_app() -> AppState {
    let mut app = AppState::new(Config::from_env());
    app.form.dry_run = false;
    app.run.dry_run = false;
    app.run.status = RunStatus::Running;
    app
}

fn started(id: &str, name: &str, phase: Phase) -> CleanerEvent {
    CleanerEvent::ChannelStarted {
        channel_id: id.into(),
        channel_name: name.into(),
        index: 0,
        total: 1,
        phase,
    }
}

fn scan_done(id: &str, scanned: u64, found: u64) -> CleanerEvent {
    CleanerEvent::ChannelDone {
        channel_id: id.into(),
        stats: ChannelStats {
            scanned,
            found,
            ..ChannelStats::default()
        },
        phase: Phase::Scan,
    }
}

fn deleted(outcome: DeleteOutcome) -> CleanerEvent {
    CleanerEvent::MessageDeleted {
        preview: "x".into(),
        outcome,
    }
}

fn failed() -> CleanerEvent {
    CleanerEvent::MessageFailed {
        message_id: "m".into(),
        error: "boom".into(),
    }
}

#[test]
fn scanning_counts_found_items_without_touching_deleted() {
    let mut app = running_app();
    app.apply_event(started("t", "alice", Phase::Scan));
    app.apply_event(CleanerEvent::ScanProgress {
        scanned: 10,
        found: 4,
    });
    app.apply_event(scan_done("t", 10, 4));

    assert_eq!(app.run.stats.messages_scanned, 10);
    assert_eq!(app.run.found, 4);
    assert_eq!(app.run.stats.messages_deleted, 0);
    assert_eq!(app.run.channels[0].stats.found, 4);
}

#[test]
fn finishing_a_target_does_not_count_its_deletes_again() {
    let mut app = running_app();
    app.apply_event(CleanerEvent::PlanReady { total: 3 });
    app.apply_event(started("t", "alice", Phase::Delete));
    app.apply_event(deleted(DeleteOutcome::Deleted));
    app.apply_event(deleted(DeleteOutcome::Forbidden));
    app.apply_event(failed());
    app.apply_event(CleanerEvent::ChannelDone {
        channel_id: "t".into(),
        stats: ChannelStats {
            found: 3,
            deleted: 1,
            skipped: 1,
            failed: 1,
            ..ChannelStats::default()
        },
        phase: Phase::Delete,
    });

    let stats = app.run.stats;
    assert_eq!(stats.messages_deleted, 1);
    assert_eq!(stats.messages_skipped, 1);
    assert_eq!(stats.messages_failed, 1);
    // refused and failed items are finished with too, so the gauge can reach 100%
    assert_eq!(app.run.plan_done, 3);
    assert_eq!(app.run.channels.len(), 1);
    assert_eq!(app.run.channels[0].stats.deleted, 1);
}

#[test]
fn overlapping_scans_and_their_deletes_share_a_correctly_named_row() {
    let mut app = running_app();
    app.apply_event(started("a", "alice", Phase::Scan));
    app.apply_event(started("b", "bob", Phase::Scan));
    app.apply_event(scan_done("a", 5, 2));
    app.apply_event(scan_done("b", 7, 3));
    assert!(app.run.current_progress.is_none());

    app.apply_event(started("a", "alice", Phase::Delete));
    app.apply_event(deleted(DeleteOutcome::Deleted));

    assert_eq!(app.run.channels.len(), 2);
    let row = app.run.channels.last().unwrap();
    assert_eq!((row.id.as_str(), row.name.as_str()), ("a", "alice"));
    assert_eq!((row.stats.found, row.stats.deleted), (2, 1));
    assert!(row.deleting);
}

#[test]
fn a_retried_item_stops_counting_as_failed() {
    let mut app = running_app();
    app.apply_event(CleanerEvent::PlanReady { total: 2 });
    app.apply_event(started("t", "alice", Phase::Delete));
    app.apply_event(deleted(DeleteOutcome::Deleted));
    app.apply_event(failed());

    app.apply_event(CleanerEvent::Retrying {
        counts: vec![RetryCount {
            target_id: "t".into(),
            failed: 1,
            skipped: 0,
        }],
    });
    app.apply_event(CleanerEvent::PlanReady { total: 2 });
    app.apply_event(started("t", "alice", Phase::Delete));
    app.apply_event(deleted(DeleteOutcome::Deleted));

    assert_eq!(app.run.found, 2);
    assert_eq!(app.run.plan_done, 2);
    assert_eq!(app.run.stats.messages_deleted, 2);
    assert_eq!(app.run.stats.messages_failed, 0);
    assert_eq!(app.run.channels[0].stats.failed, 0);
}

#[test]
fn done_keeps_a_cancelled_or_failed_status() {
    let mut app = running_app();
    app.apply_event(CleanerEvent::Cancelled);
    app.apply_event(CleanerEvent::Done {
        stats: RunStats {
            was_cancelled: true,
            plan_total: 5,
            messages_deleted: 2,
            ..RunStats::default()
        },
    });
    assert_eq!(app.run.status, RunStatus::Cancelled);
    assert_eq!(app.run.found, 5);
    assert_eq!(app.run.stats.messages_deleted, 2);

    let mut app = running_app();
    app.apply_event(CleanerEvent::Warning {
        message: "w".into(),
    });
    app.apply_event(CleanerEvent::Error {
        message: "could not load targets".into(),
    });
    app.apply_event(CleanerEvent::Done {
        stats: RunStats::default(),
    });
    assert_eq!(app.run.status, RunStatus::Failed);
    assert_eq!(app.run.warnings, 1);
}

#[test]
fn a_retry_only_clears_the_failures_of_its_own_target() {
    let mut app = running_app();
    app.apply_event(started("a", "alice", Phase::Delete));
    app.apply_event(failed());
    app.apply_event(started("b", "bob", Phase::Delete));
    app.apply_event(failed());

    app.apply_event(CleanerEvent::Retrying {
        counts: vec![RetryCount {
            target_id: "a".into(),
            failed: 1,
            skipped: 0,
        }],
    });

    let row = |id: &str| app.run.channels.iter().find(|r| r.id == id).unwrap();
    assert_eq!(row("a").stats.failed, 0);
    assert_eq!(row("b").stats.failed, 1);
    assert_eq!(app.run.stats.messages_failed, 1);
}

#[test]
fn a_stopped_run_counts_what_it_never_tried() {
    let mut app = running_app();
    app.apply_event(CleanerEvent::Done {
        stats: RunStats {
            was_cancelled: true,
            plan_total: 10,
            messages_deleted: 3,
            messages_failed: 1,
            ..RunStats::default()
        },
    });
    assert_eq!(app.run.not_attempted(), 6);
}

#[test]
fn a_cancelled_run_stays_active_until_it_has_really_stopped() {
    let mut app = running_app();
    assert!(matches!(
        app.apply_action(Action::Back).as_slice(),
        [Effect::CancelRun]
    ));
    assert!(app.run.cancelling);
    assert_eq!(app.run.phase_label(), "cancelling");

    app.apply_event(CleanerEvent::Cancelled);
    assert!(app.run.is_running());
    assert!(app.apply_action(Action::Activate).is_empty());

    app.apply_event(CleanerEvent::Done {
        stats: RunStats {
            was_cancelled: true,
            ..RunStats::default()
        },
    });
    assert_eq!(app.run.status, RunStatus::Cancelled);
    assert!(!app.run.cancelling);
}

#[test]
fn turning_dry_run_on_mid_run_does_not_hide_that_this_run_is_live() {
    let mut app = running_app();
    app.apply_action(Action::ToggleDryRun);
    assert!(app.form.dry_run);
    assert!(!app.header_dry_run());
    assert!(!app.shown_dry_run());
}

#[test]
fn a_started_run_remembers_whether_it_is_dry() {
    let mut app = AppState::new(Config::from_env());
    app.form = form_with("posts");
    app.form.dry_run = true;
    app.form.sessionid = "fake_sessionid_123456789".into();
    let effects = app.apply_action(Action::Activate);
    assert!(matches!(effects.as_slice(), [Effect::StartRun(_)]));
    assert!(app.run.dry_run);
}

fn protected_thread_app() -> AppState {
    let mut app = AppState::new(Config::from_env());
    let mut alice = thread("t1");
    alice.users = vec![crate::model::UserShort {
        pk: "42".into(),
        username: "Alice".into(),
        full_name: String::new(),
    }];
    app.targets.threads = vec![alice];
    app.screen = Screen::Targets;
    app
}

fn type_protect_list(app: &mut AppState, list: &str) {
    app.settings.cursor = Field::ALL
        .iter()
        .position(|field| *field == Field::Protect)
        .unwrap();
    app.screen = Screen::Settings;
    app.apply_action(Action::StartEdit);
    for c in list.chars() {
        app.apply_action(Action::Input(c));
    }
    app.apply_action(Action::CommitEdit);
    app.screen = Screen::Targets;
}

#[test]
fn pressing_p_does_not_drop_protections_typed_in_settings() {
    let mut app = protected_thread_app();
    app.targets.threads[0].users.clear();
    type_protect_list(&mut app, "@friend, other");

    app.apply_action(Action::ToggleProtect);

    let kept = crate::config::thread_id_list(&app.form.protect_users);
    assert!(kept.iter().any(|entry| entry == "@friend"), "{kept:?}");
    assert!(kept.iter().any(|entry| entry == "other"), "{kept:?}");
    assert!(kept.iter().any(|entry| entry == "t1"), "{kept:?}");
}

#[test]
fn the_thread_list_reads_protections_the_way_the_run_does() {
    let mut app = protected_thread_app();
    type_protect_list(&mut app, "@ALICE");
    let thread = app.targets.threads[0].clone();
    assert!(app.targets.is_protected(&thread));

    // P lifts the protection the list shows, not just an entry for the thread ID
    app.apply_action(Action::ToggleProtect);
    assert!(!app.targets.is_protected(&thread));
    assert_eq!(app.form.protect_users, "");
}

#[test]
fn switching_screens_ends_the_edit_and_keeps_what_was_typed() {
    let mut app = AppState::new(Config::from_env());
    app.settings.cursor = Field::ALL
        .iter()
        .position(|field| *field == Field::Limit)
        .unwrap();
    app.apply_action(Action::StartEdit);
    app.apply_action(Action::InputClear);
    app.apply_action(Action::Input('7'));

    app.apply_action(Action::Goto(Screen::Run));

    assert!(!app.settings.editing);
    assert_eq!(app.form.limit, "7");
}

#[test]
fn fixing_a_setting_clears_its_error() {
    let mut app = AppState::new(Config::from_env());
    app.form = form_with("");
    app.settings.errors = app.form.validate();
    assert!(app.settings.errors.contains_key(&Field::ContentPosts));

    app.settings.cursor = Field::ALL
        .iter()
        .position(|field| *field == Field::ContentPosts)
        .unwrap();
    app.apply_action(Action::Toggle);

    assert!(!app.settings.errors.contains_key(&Field::ContentPosts));
}

#[test]
fn the_thread_cursor_stays_on_the_list_when_it_shrinks() {
    let mut app = AppState::new(Config::from_env());
    app.targets.threads = vec![thread("a"), thread("b"), thread("c")];
    app.targets.thread_cursor = 2;

    app.apply_event(CleanerEvent::DMsLoaded {
        channels: vec![thread("a")],
    });

    assert_eq!(app.targets.thread_cursor, 0);
}

#[test]
fn scrolling_the_log_stops_at_its_oldest_line() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Run;
    app.run.push_log(crate::events::LogLevel::Info, "a");
    app.run.push_log(crate::events::LogLevel::Info, "b");
    for _ in 0..10 {
        app.apply_action(Action::Up);
    }
    assert_eq!(app.run.log_scroll, 1);
    app.apply_action(Action::Down);
    assert_eq!(app.run.log_scroll, 0);
}

#[test]
fn the_archive_filter_goes_both_ways() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Archive;
    let start = app.archive.scope;
    app.apply_action(Action::CycleNext);
    assert_ne!(app.archive.scope, start);
    app.apply_action(Action::CyclePrev);
    assert_eq!(app.archive.scope, start);
}

fn dry_run_ready_app() -> AppState {
    let mut app = AppState::new(Config::from_env());
    app.form.sessionid = "fake_sessionid".to_string();
    app.form.dry_run = true;
    app.form.content.posts = true;
    app
}

#[test]
fn f5_and_the_palette_start_a_run_from_the_archive_but_enter_does_not() {
    let mut app = dry_run_ready_app();
    app.screen = Screen::Archive;
    assert!(app.apply_action(Action::Activate).is_empty());
    assert!(matches!(
        app.apply_action(Action::StartRun).as_slice(),
        [Effect::StartRun(_)]
    ));

    let start = action::commands().iter().find(|c| c.id == "start").unwrap();
    assert_eq!(start.action, Action::StartRun);
}

#[test]
fn reloading_the_archive_from_the_palette_does_not_reconnect() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Settings;
    let reload = action::commands()
        .iter()
        .find(|c| c.id == "refresh-archive")
        .unwrap();
    assert!(app.apply_action(reload.action.clone()).is_empty());
}

#[test]
fn no_new_run_while_the_failed_one_can_still_send_events() {
    let mut app = dry_run_ready_app();
    assert_eq!(app.apply_action(Action::StartRun).len(), 1);
    app.apply_event(CleanerEvent::Error {
        message: "boom".into(),
    });

    assert!(app.apply_action(Action::StartRun).is_empty());
    assert_eq!(
        app.run.status,
        RunStatus::Failed,
        "the failure stays visible"
    );

    app.apply_event(CleanerEvent::Done {
        stats: RunStats::default(),
    });
    assert_eq!(app.run.status, RunStatus::Failed);
    assert_eq!(app.apply_action(Action::StartRun).len(), 1);
}

fn archive_with_days(dir: &std::path::Path, days: &[&str]) -> super::ArchiveState {
    let store = crate::history::HistoryStore::with_root(dir);
    for (index, day) in days.iter().enumerate() {
        archive_record(&store, day, &format!("t{index}"));
    }
    let mut archive = super::ArchiveState {
        store,
        days: Vec::new(),
        channels: Vec::new(),
        records: Vec::new(),
        ..super::ArchiveState::default()
    };
    archive.day_cursor = 0;
    archive.channel_cursor = 0;
    archive.reload();
    archive
}

fn archive_record(store: &crate::history::HistoryStore, day: &str, target: &str) {
    let record = crate::history::DeletedRecord {
        deleted_at: format!("{day}T12:00:00Z").parse().unwrap(),
        outcome: DeleteOutcome::Deleted,
        target_id: target.into(),
        target_name: target.into(),
        item_id: "m".into(),
        kind: "dm".into(),
        author_id: String::new(),
        author_name: String::new(),
        content: String::new(),
        timestamp: String::new(),
        media_type: None,
        attachments: Vec::new(),
        preview: String::new(),
    };
    store.append(&record).unwrap();
}

#[test]
fn a_new_day_in_the_archive_does_not_move_the_selection() {
    let dir = std::env::temp_dir().join(format!("archive-{}", uuid::Uuid::new_v4()));
    let mut archive = archive_with_days(&dir, &["2026-01-02", "2026-01-01"]);
    archive.step_day(true);
    let selected = archive.days[archive.day_cursor];

    archive_record(&archive.store.clone(), "2026-01-03", "t9");
    archive.reload();

    assert_eq!(archive.days.len(), 3);
    assert_eq!(archive.days[archive.day_cursor], selected);
    assert_eq!(archive.records[0].target_id, "t1");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn moving_to_another_archive_day_starts_at_its_first_target() {
    let dir = std::env::temp_dir().join(format!("archive-{}", uuid::Uuid::new_v4()));
    let mut archive = archive_with_days(&dir, &["2026-01-02", "2026-01-01"]);
    archive_record(&archive.store.clone(), "2026-01-02", "t5");
    archive.reload();
    archive.step_channel(true);
    assert_eq!(archive.channel_cursor, 1);

    archive.step_day(true);
    assert_eq!(archive.channel_cursor, 0);
    assert_eq!(archive.record_cursor, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn picks_in_scope_leave_out_hidden_and_protected_threads() {
    let mut app = AppState::new(Config::from_env());
    let mut group = thread("group");
    group.is_group = true;
    app.targets.threads = vec![thread("a"), thread("b"), group];
    for id in ["a", "b", "group"] {
        app.targets.selected_threads.insert(id.to_string());
    }
    app.targets.protected_users.insert("b".to_string());
    app.form.dm_kind = crate::config::DmKind::OneOnOne;
    assert_eq!(app.picks_in_scope_len(), 1);
}

#[test]
fn rate_limit_sync_follows_the_limiter() {
    let mut app = running_app();
    app.run.sync_rate_limits(2, Duration::from_millis(500));
    assert_eq!(app.run.stats.rate_limit_hits, 2);
    assert_eq!(app.run.rate_limiter.cooldown_remaining_ms, 500);
    assert_eq!(app.run.sparkline_data.len(), 1);

    app.run.sync_rate_limits(2, Duration::ZERO);
    assert_eq!(app.run.rate_limiter.cooldown_remaining_ms, 0);
    assert_eq!(app.run.sparkline_data.len(), 1);
}

#[test]
fn mask_sessionid_hides_secrets() {
    assert_eq!(mask_sessionid(""), "(empty)");
    assert_eq!(mask_sessionid("123"), "••••••");
    assert_eq!(mask_sessionid("1234567890abcdef"), "••••••••••••cdef");
    assert_eq!(mask_sessionid("1234567890🔐"), "••••••••••••890🔐");
}

#[test]
fn space_toggles_the_focused_content_class_into_the_run() {
    let mut app = AppState::new(Config::from_env());
    app.form.sessionid = "fake_sessionid".to_string();
    app.form.dry_run = true;
    app.screen = Screen::Settings;

    assert!(!app.form.content.any());
    assert!(app.form.validate().contains_key(&Field::ContentPosts));

    app.settings.cursor = Field::ALL
        .iter()
        .position(|f| *f == Field::ContentLikes)
        .expect("content field present");
    app.apply_action(Action::Toggle);

    assert!(app.form.content.activity.likes);
    assert_eq!(app.form.content.label(), "likes");

    let cfg = app.form.to_cleaner_config().expect("valid config");
    assert!(cfg.targets.is_empty());
    assert!(cfg.content.likes && cfg.content.count() == 1);

    app.apply_action(Action::Toggle);
    assert!(!app.form.content.any());
    assert!(app.form.validate().contains_key(&Field::ContentPosts));
}

#[test]
fn every_flag_field_can_be_toggled() {
    let mut form = Form::from_config(&Config::from_env());
    for field in Field::ALL {
        if let Some(before) = form.flag(field) {
            form.toggle(field);
            assert_eq!(form.flag(field), Some(!before), "{field:?} did not toggle");
        }
    }
}
