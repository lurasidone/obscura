use crate::config::{Config, ScopeKind};
use crate::events::PrunerEvent;
use crate::model::{Channel, User};
use crate::pruner::TargetResolution;

use super::super::action::{self, Action, Effect, Field, Overlay, Screen};
use super::{mask_token, AppState, Form, RunStatus};

#[test]
fn form_validation_checks_token_and_dates() {
    let mut form = Form::from_config(&Config::from_env());
    form.token = String::new();
    let errs = form.validate();
    assert!(errs.contains_key(&Field::Token));

    form.token = "fake_token_value".to_string();
    form.before = "invalid_date".to_string();
    let errs = form.validate();
    assert!(errs.contains_key(&Field::Before));

    form.before = "2026-01-01".to_string();
    let errs = form.validate();
    assert!(!errs.contains_key(&Field::Before));
}

#[test]
fn settings_navigation_skips_hidden_payload() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Settings;
    app.settings.cursor = Field::ALL
        .iter()
        .position(|field| *field == Field::Scope)
        .unwrap();

    app.apply_action(Action::Down);
    assert_eq!(app.settings.field(), Field::Before);

    app.form.scope.kind = ScopeKind::Channels;
    app.settings.cursor = Field::ALL
        .iter()
        .position(|field| *field == Field::Scope)
        .unwrap();
    app.apply_action(Action::Down);
    assert_eq!(app.settings.field(), Field::Payload);
}

#[test]
fn cycling_away_from_channels_scope_moves_focus_off_hidden_payload() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Settings;
    app.form.scope.kind = ScopeKind::Channels;
    app.settings.cursor = Field::ALL
        .iter()
        .position(|field| *field == Field::Scope)
        .unwrap();

    // Channels -> None: the payload becomes hidden, so focus leaves it.
    app.apply_action(Action::CycleNext);
    assert_eq!(app.form.scope.kind, ScopeKind::None);
    assert_eq!(app.settings.field(), Field::Scope);
}

#[test]
fn target_promotion_sets_channels_scope() {
    let mut app = AppState::new(Config::from_env());
    app.screen = Screen::Targets;
    let first = dm("1556238032934277223", "alice");
    let second = dm("1561787017417855028", "bob");
    app.targets.threads = vec![first.clone(), second.clone()];
    app.targets.selected_threads.insert(first.id.clone());
    app.targets.selected_threads.insert(second.id.clone());

    app.apply_action(Action::Activate);

    assert_eq!(app.form.scope.kind, ScopeKind::Channels);
    assert_eq!(
        app.form.scope.channels,
        "1556238032934277223, 1561787017417855028"
    );
    assert_eq!(app.screen, Screen::Settings);
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
    app.form.token = "fake_token_value".into();
    app.form.dry_run = true;
    app.run.status = RunStatus::Running;

    assert!(app.apply_action(Action::Activate).is_empty());
    assert!(app
        .toast
        .as_ref()
        .is_some_and(|toast| toast.text.contains("already active")));
}

fn dm(id: &str, username: &str) -> Channel {
    Channel {
        id: id.into(),
        kind: 1,
        channel_type: 1,
        name: None,
        guild_id: None,
        recipients: vec![User {
            id: format!("user-{id}"),
            username: username.into(),
        }],
    }
}

fn run_targets(app: &mut AppState) -> Vec<TargetResolution> {
    app.form.token = "fake_token_value".to_string();
    app.form.dry_run = true;
    app.form.limit = "0".to_string();
    app.form.throttle_ms = "2000".to_string();
    app.form.scan_throttle_ms = "350".to_string();
    app.form.concurrency = "4".to_string();
    app.screen = Screen::Settings;
    let effects = app.apply_action(Action::Activate);
    match effects.into_iter().next() {
        Some(Effect::StartRun(cfg)) => cfg.targets,
        other => panic!("expected StartRun, got {other:?}"),
    }
}

#[test]
fn picked_channels_override_blanket_scope() {
    let mut app = AppState::new(Config::from_env());
    app.form.scope.kind = ScopeKind::All;
    let first = dm("111", "alice");
    let second = dm("222", "bob");
    let third = dm("333", "carol");
    app.targets.threads = vec![first, second, third];
    app.targets.selected_threads.insert("222".to_string());
    app.targets.selected_threads.insert("111".to_string());

    match run_targets(&mut app).as_slice() {
        [TargetResolution::ChannelObjects(picked)] => {
            let ids: Vec<&str> = picked.iter().map(|t| t.id.as_str()).collect();
            assert_eq!(ids, ["111", "222"]);
        }
        other => panic!("expected only picked channels, got {other:?}"),
    }
}

#[test]
fn empty_picker_keeps_scope_targets() {
    let mut app = AppState::new(Config::from_env());
    app.form.scope.kind = ScopeKind::All;
    app.targets.threads = vec![dm("111", "alice")];

    let targets = run_targets(&mut app);
    assert!(matches!(
        targets.as_slice(),
        [TargetResolution::AllDms, TargetResolution::AllGuilds]
    ));
}

#[test]
fn dms_scope_is_narrowed_by_picks() {
    let mut app = AppState::new(Config::from_env());
    app.form.scope.kind = ScopeKind::Dms;
    app.targets.threads = vec![dm("111", "alice"), dm("222", "bob")];
    app.targets.selected_threads.insert("222".to_string());

    match run_targets(&mut app).as_slice() {
        [TargetResolution::ChannelObjects(picked)] => {
            assert_eq!(picked.len(), 1);
            assert_eq!(picked[0].id, "222");
        }
        other => panic!("expected only picked channels, got {other:?}"),
    }
}

#[test]
fn typed_channels_scope_beats_stale_picks() {
    let mut app = AppState::new(Config::from_env());
    app.form.scope.kind = ScopeKind::Channels;
    app.form.scope.channels = "typed_1, typed_2".to_string();
    app.targets.threads = vec![dm("111", "alice")];
    app.targets.selected_threads.insert("111".to_string());

    match run_targets(&mut app).as_slice() {
        [TargetResolution::Channels(ids)] => assert_eq!(ids, &["typed_1", "typed_2"]),
        other => panic!("expected the typed ids to win, got {other:?}"),
    }
}

#[test]
fn none_scope_runs_nothing() {
    let mut app = AppState::new(Config::from_env());
    app.form.scope.kind = ScopeKind::None;
    app.targets.threads = vec![dm("111", "alice")];
    app.targets.selected_threads.insert("111".to_string());

    assert!(run_targets(&mut app).is_empty());
}

#[test]
fn refreshed_inbox_drops_unresolvable_picks() {
    let mut app = AppState::new(Config::from_env());
    app.form.scope.kind = ScopeKind::All;
    app.targets.threads = vec![dm("111", "alice"), dm("222", "bob")];
    app.targets.selected_threads.insert("111".to_string());
    app.targets.selected_threads.insert("222".to_string());

    app.apply_event(PrunerEvent::DMsLoaded {
        channels: vec![dm("222", "bob")],
    });

    assert_eq!(
        app.targets.selected_threads.iter().collect::<Vec<_>>(),
        vec!["222"]
    );

    match run_targets(&mut app).as_slice() {
        [TargetResolution::ChannelObjects(picked)] => assert_eq!(picked.len(), 1),
        other => panic!("expected the surviving pick, got {other:?}"),
    }
}

#[test]
fn mask_token_hides_secrets() {
    assert_eq!(mask_token(""), "(empty)");
    assert_eq!(mask_token("123"), "••••••");
    assert_eq!(mask_token("1234567890abcdef"), "••••••••••••cdef");
    assert_eq!(mask_token("1234567890🔐"), "••••••••••••890🔐");
}
