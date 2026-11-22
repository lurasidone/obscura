use std::collections::HashSet;

use crate::config::{ContentFilter, DmKind};
use crate::error::DiscordError;
use crate::events::{ChannelStats, RunStats};
use crate::model::{Channel, Message, MessageAuthor, User};

use super::dates::{parse_to_datetime, validate_date_input};
use super::execute::{
    classify_delete_error, group_by_target, is_retryable_delete_error, map_delete_bool,
    record_outcome, DeleteStep,
};
use super::filters::{
    channel_matches_kind, first_protected_channel, in_date_window, should_delete_message,
};
use super::resolve::{admit_channel, Skips};
use super::{drop_already_seen, finalize_plan, ItemKind, Plan, PlannedDelete};

fn channel(id: &str, kind: i32, user_ids: &[&str]) -> Channel {
    Channel {
        id: id.into(),
        kind,
        channel_type: kind,
        name: Some(format!("c-{id}")),
        guild_id: None,
        recipients: user_ids
            .iter()
            .map(|u| User {
                id: (*u).into(),
                username: format!("u{u}"),
            })
            .collect(),
    }
}

fn message(id: &str, user: &str) -> Message {
    Message {
        id: id.into(),
        channel_id: "c".into(),
        author: Some(MessageAuthor { id: user.into() }),
        content: "hello".into(),
        timestamp: "2024-01-15T12:00:00Z".into(),
        message_type: 0,
        ..Default::default()
    }
}

fn planned(target: &str, item: &str) -> PlannedDelete {
    PlannedDelete {
        target_id: target.into(),
        item_id: item.into(),
        ..Default::default()
    }
}

#[test]
fn first_protected_channel_returns_match() {
    let t = channel("1", 1, &["a", "b"]);
    let protect: HashSet<&str> = ["b"].into_iter().collect();
    assert_eq!(first_protected_channel(&t, &protect).as_deref(), Some("ub"));
}

#[test]
fn first_protected_channel_matches_username_and_channel_id() {
    let mut t = channel("999", 1, &["a", "b"]);
    t.name = Some("general".into());
    for entry in ["ub", "@ub", "@b", "999", "@999", "general", "#general"] {
        let protect: HashSet<&str> = [entry].into_iter().collect();
        assert!(
            first_protected_channel(&t, &protect).is_some(),
            "expected {entry:?} to match"
        );
    }
}

#[test]
fn first_protected_channel_none() {
    let t = channel("1", 1, &["a"]);
    let protect: HashSet<&str> = HashSet::new();
    assert!(first_protected_channel(&t, &protect).is_none());
}

#[test]
fn should_delete_filters_other_authors() {
    assert!(!should_delete_message(
        &message("1", "x"),
        "me",
        None,
        None,
        ContentFilter::All,
        false
    ));
    assert!(should_delete_message(
        &message("2", "me"),
        "me",
        None,
        None,
        ContentFilter::All,
        false
    ));
}

#[test]
fn should_delete_date_window() {
    let m = message("1", "me");
    let before = parse_to_datetime("2024-06-01", crate::pruner::dates::DateBound::After).unwrap();
    let after = parse_to_datetime("2023-01-01", crate::pruner::dates::DateBound::After).unwrap();
    assert!(should_delete_message(
        &m,
        "me",
        Some(before),
        Some(after),
        ContentFilter::All,
        false
    ));
    assert!(!should_delete_message(
        &m,
        "me",
        Some(parse_to_datetime("2023-01-01", crate::pruner::dates::DateBound::After).unwrap()),
        None,
        ContentFilter::All,
        false
    ));
}

#[test]
fn unparseable_timestamp_fails_closed() {
    let mut m = message("notanid", "me");
    m.timestamp = String::new();
    assert!(!should_delete_message(
        &m,
        "me",
        None,
        None,
        ContentFilter::All,
        false
    ));
    assert!(!in_date_window(None, None, None));
}

#[test]
fn channel_kind_filter() {
    let one = channel("1", 1, &["a"]);
    let grp = channel("2", 3, &["a", "b"]);
    assert!(channel_matches_kind(&one, DmKind::OneOnOne));
    assert!(!channel_matches_kind(&one, DmKind::Group));
    assert!(channel_matches_kind(&grp, DmKind::Group));
    assert!(channel_matches_kind(&grp, DmKind::All));
}

#[test]
fn admit_channel_skips_kind_protected_and_duplicates() {
    let dm_kind = DmKind::OneOnOne;
    let protect: HashSet<&str> = ["@ub"].into_iter().collect();
    let mut seen = HashSet::new();
    let mut skips = Skips::default();

    let group = channel("g", 3, &["a", "b"]);
    assert!(!admit_channel(
        &group, dm_kind, true, &protect, &mut seen, &mut skips
    ));
    assert_eq!(skips.kind, 1);

    let guarded = channel("p", 1, &["a", "b"]);
    assert!(!admit_channel(
        &guarded, dm_kind, true, &protect, &mut seen, &mut skips
    ));
    assert_eq!(skips.protected, 1);

    let ok = channel("t1", 1, &["a", "c"]);
    assert!(admit_channel(
        &ok, dm_kind, true, &protect, &mut seen, &mut skips
    ));
    assert!(
        !admit_channel(&ok, dm_kind, true, &protect, &mut seen, &mut skips),
        "same channel id must not be admitted twice"
    );

    let named_group = channel("g2", 3, &["a"]);
    assert!(admit_channel(
        &named_group,
        dm_kind,
        false,
        &protect,
        &mut seen,
        &mut skips
    ));

    let voice = channel("v", 2, &[]);
    assert!(!admit_channel(
        &voice, dm_kind, false, &protect, &mut seen, &mut skips
    ));
    assert_eq!(skips.non_text, 1);
}

#[test]
fn planned_delete_to_record() {
    let p = PlannedDelete {
        target_id: "123".into(),
        target_name: "#general".into(),
        item_id: "456".into(),
        kind: ItemKind::Message,
        preview: "hi".into(),
        author_id: "me".into(),
        author_name: "me".into(),
        content: "hi".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        media_type: Some("message".into()),
        attachments: vec![],
    };
    let r = p.to_record(crate::history::DeleteOutcome::DryRun);
    assert_eq!(r.target_id, "123");
    assert_eq!(r.item_id, "456");
    assert_eq!(r.kind, "message");
    assert_eq!(r.outcome, crate::history::DeleteOutcome::DryRun);
}

#[test]
fn finalize_plan_sorts_and_dedups_on_target_item_key() {
    let mut plan = Plan {
        entries: vec![
            planned("b", "2"),
            planned("a", "2"),
            planned("a", "1"),
            planned("a", "1"),
            planned("b", "1"),
        ],
        ..Default::default()
    };
    finalize_plan(&mut plan);
    let keys: Vec<(&str, &str)> = plan
        .entries
        .iter()
        .map(|e| (e.target_id.as_str(), e.item_id.as_str()))
        .collect();
    assert_eq!(keys, [("a", "1"), ("a", "2"), ("b", "1"), ("b", "2")]);
}

#[test]
fn verify_pass_never_redeletes_seen_items() {
    let mut first = Plan {
        entries: vec![planned("t", "1"), planned("t", "2")],
        ..Default::default()
    };
    finalize_plan(&mut first);
    let seen: HashSet<(String, String)> = first
        .entries
        .iter()
        .map(|e| (e.target_id.clone(), e.item_id.clone()))
        .collect();

    let mut leftover = Plan {
        entries: vec![planned("t", "2"), planned("t", "3")],
        ..Default::default()
    };
    finalize_plan(&mut leftover);
    drop_already_seen(&mut leftover, &seen);
    assert_eq!(leftover.entries.len(), 1);
    assert_eq!(leftover.entries[0].item_id, "3");
}

#[test]
fn group_by_target_keeps_serial_order() {
    let groups = group_by_target(vec![
        planned("a", "1"),
        planned("a", "2"),
        planned("b", "1"),
    ]);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].0, "a");
    assert_eq!(groups[0].2.len(), 2);
    assert_eq!(groups[1].0, "b");
    assert_eq!(groups[1].2.len(), 1);
}

#[test]
fn drop_already_seen_removes_ids() {
    let mut plan = Plan {
        entries: vec![planned("t1", "m1"), planned("t1", "m2")],
        ..Default::default()
    };
    let mut seen = HashSet::new();
    seen.insert(("t1".into(), "m1".into()));
    drop_already_seen(&mut plan, &seen);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].item_id, "m2");
}

#[test]
fn is_retryable_delete_error_only_5xx_and_http() {
    assert!(is_retryable_delete_error(&DiscordError::Api {
        status: 502,
        message: "bad".into()
    }));
    assert!(!is_retryable_delete_error(&DiscordError::Api {
        status: 400,
        message: "bad".into()
    }));
    assert!(!is_retryable_delete_error(&DiscordError::NotFound {
        resource: "x".into(),
        status: 404
    }));
}

#[test]
fn delete_errors_map_to_delete_steps() {
    assert_eq!(
        classify_delete_error(&DiscordError::NotFound {
            resource: "x".into(),
            status: 404
        }),
        DeleteStep::AlreadyGone
    );
    assert_eq!(
        classify_delete_error(&DiscordError::Forbidden {
            resource: "x".into(),
            status: 403
        }),
        DeleteStep::Forbidden
    );
    assert_eq!(
        classify_delete_error(&DiscordError::Api {
            status: 500,
            message: "x".into()
        }),
        DeleteStep::Failed
    );
}

#[test]
fn record_outcome_accounts_deletes_skips_and_failures() {
    let mut channel_stats = ChannelStats::default();
    let mut stats = RunStats::default();
    let mut succeeded = HashSet::new();

    let mut message_entry = planned("t", "m1");
    message_entry.kind = ItemKind::Message;
    record_outcome(
        &message_entry,
        DeleteStep::Deleted,
        &mut channel_stats,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel_stats.deleted, 1);
    assert_eq!(stats.dm_messages_deleted, 1);
    assert!(succeeded.contains(&("t".to_string(), "m1".to_string())));

    record_outcome(
        &planned("t", "m2"),
        DeleteStep::AlreadyGone,
        &mut channel_stats,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel_stats.deleted, 2);
    assert_eq!(stats.dm_messages_deleted, 1);
    assert!(succeeded.contains(&("t".to_string(), "m2".to_string())));

    record_outcome(
        &planned("t", "m3"),
        DeleteStep::Forbidden,
        &mut channel_stats,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel_stats.skipped, 1);
    assert!(!succeeded.contains(&("t".to_string(), "m3".to_string())));

    record_outcome(
        &planned("t", "m4"),
        DeleteStep::Failed,
        &mut channel_stats,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel_stats.failed, 1);
}

#[test]
fn validate_date_input_accepts_common_formats() {
    assert!(validate_date_input("").unwrap().is_none());
    assert!(validate_date_input("2024-01-01").unwrap().is_some());
    assert!(validate_date_input("2024-01-01T12:00:00Z")
        .unwrap()
        .is_some());
    assert!(validate_date_input("2024-01-01 12:00:00")
        .unwrap()
        .is_some());
}

#[test]
fn validate_date_input_rejects_garbage() {
    assert!(validate_date_input("nope").is_err());
    assert!(validate_date_input("2024/01/01").is_err());
}

#[test]
fn from_message_builds_planned() {
    let m = message("99", "me");
    let p = PlannedDelete::from_message(&m, "cid", "#general", "me", "me");
    assert_eq!(p.target_id, "cid");
    assert_eq!(p.kind, ItemKind::Message);
    assert_eq!(p.item_id, "99");
    assert!(!p.preview.is_empty());
}

#[test]
fn map_delete_bool_false_is_err() {
    assert!(map_delete_bool(true, "x").is_ok());
    let err = map_delete_bool(false, "message delete").unwrap_err();
    assert!(!is_retryable_delete_error(&err));
}

#[test]
fn discord_search_response_shape_extracts_target_messages() {
    // Discord API returns {"messages": [[target_msg, ...context], ...], "total_results": N}
    let json = serde_json::json!({
        "total_results": 2,
        "messages": [
            [
                {
                    "id": "1001",
                    "channel_id": "ch1",
                    "author": { "id": "user1", "username": "alice" },
                    "content": "first target",
                    "timestamp": "2024-01-15T12:00:00Z"
                },
                {
                    "id": "1000",
                    "channel_id": "ch1",
                    "author": { "id": "user2", "username": "bob" },
                    "content": "context message",
                    "timestamp": "2024-01-15T11:59:00Z"
                }
            ],
            [
                {
                    "id": "1002",
                    "channel_id": "ch2",
                    "author": { "id": "user1", "username": "alice" },
                    "content": "second target",
                    "timestamp": "2024-01-15T12:05:00Z"
                }
            ]
        ]
    });

    let raw_messages = json
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .unwrap();
    assert_eq!(raw_messages.len(), 2);

    let parsed: Vec<Message> = raw_messages
        .iter()
        .filter_map(|entry| {
            let target = if let Some(arr) = entry.as_array() {
                arr.first()?
            } else {
                entry
            };
            serde_json::from_value(target.clone()).ok()
        })
        .collect();

    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].id, "1001");
    assert_eq!(parsed[0].content, "first target");
    assert_eq!(parsed[1].id, "1002");
    assert_eq!(parsed[1].content, "second target");
}

#[test]
fn only_1on1_dms_are_identified_as_dm() {
    let dm_1on1 = channel("1", 1, &["alice"]);
    let group_dm = channel("2", 3, &["alice", "bob"]);
    let mut guild_ch = channel("3", 0, &[]);
    guild_ch.guild_id = Some("guild_1".into());

    assert!(dm_1on1.is_dm(), "1:1 DM (type 1) must be is_dm()");
    assert!(!dm_1on1.is_group_dm());

    assert!(
        group_dm.is_group_dm(),
        "Group DM (type 3) must be is_group_dm()"
    );
    assert!(!group_dm.is_dm(), "Group DM (type 3) must NEVER be is_dm()");

    assert!(!guild_ch.is_dm(), "Guild channel must NEVER be is_dm()");
    assert!(
        !guild_ch.is_group_dm(),
        "Guild channel must NEVER be is_group_dm()"
    );
}

#[test]
fn default_config_has_close_dms_enabled() {
    // Verify that close_dms is true by default
    let cfg = crate::config::Config::from_env();
    assert!(cfg.close_dms);
}
