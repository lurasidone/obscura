use std::collections::HashSet;

use crate::config::DmKind;
use crate::error::IgError;
use crate::events::{ChannelStats, RunStats};
use crate::model::{Caption, DirectMessage, DirectThread, Media, OwnReaction, UserShort};

use super::dates::{dm_timestamp_us, parse_to_datetime, validate_date_input};
use super::execute::{
    classify_delete_error, group_by_target, is_retryable_delete_error, map_delete_bool,
    record_outcome, DeleteStep,
};
use super::filters::{
    dm_action, first_protected_user, in_date_window, should_delete_dm, should_delete_media,
    thread_matches_kind, DmAction, DmRules,
};
use super::resolve::{admit_thread, has_dm_scope, target_count, ResolvedTargets, Skips};
use super::{
    drop_already_seen, finalize_plan, ItemKind, Plan, PlannedDelete, Settled, TargetResolution,
};

fn media(pk: &str, author: &str, taken_at: i64) -> Media {
    Media {
        pk: pk.into(),
        id: Some(format!("{pk}_{author}")),
        code: Some("AbC".into()),
        taken_at,
        media_type: 1,
        product_type: None,
        caption: Some(Caption {
            text: "caption".into(),
        }),
        user: Some(UserShort {
            pk: author.into(),
            username: "me".into(),
            full_name: String::new(),
        }),
        image_versions2: None,
        thumbnail_url: None,
    }
}

fn dm(id: &str, user: &str, ts_us: i64) -> DirectMessage {
    DirectMessage {
        item_id: id.into(),
        id: None,
        user_id: Some(user.into()),
        timestamp: ts_us,
        item_type: Some("text".into()),
        text: Some("hello".into()),
        client_context: None,
        extra: Default::default(),
    }
}

fn thread(id: &str, is_group: bool, user_ids: &[&str]) -> DirectThread {
    DirectThread {
        thread_id: id.into(),
        pk: None,
        thread_title: Some(format!("t-{id}")),
        is_group,
        users: user_ids
            .iter()
            .map(|u| UserShort {
                pk: (*u).into(),
                username: format!("u{u}"),
                full_name: String::new(),
            })
            .collect(),
        items: vec![],
        has_older: false,
        prev_cursor: None,
        oldest_cursor: None,
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
fn first_protected_user_returns_match() {
    let t = thread("1", false, &["a", "b"]);
    let protect: HashSet<&str> = ["b"].into_iter().collect();
    assert_eq!(first_protected_user(&t, &protect).as_deref(), Some("ub"));
}

#[test]
fn first_protected_user_matches_username_and_thread_id() {
    let t = thread("thread_999", false, &["a", "b"]);
    for entry in ["ub", "@ub", "@b", "thread_999", "@thread_999"] {
        let protect: HashSet<&str> = [entry].into_iter().collect();
        assert!(
            first_protected_user(&t, &protect).is_some(),
            "expected {entry:?} to match"
        );
    }
}

#[test]
fn first_protected_user_none() {
    let t = thread("1", false, &["a"]);
    let protect: HashSet<&str> = HashSet::new();
    assert!(first_protected_user(&t, &protect).is_none());
}

#[test]
fn should_delete_filters_other_authors() {
    assert!(!should_delete_media(
        &media("1", "x", 1_700_000_000),
        "me",
        None,
        None
    ));
    assert!(!should_delete_dm(
        &dm("i", "x", 1_700_000_000_000_000),
        "me",
        None,
        None
    ));
}

#[test]
fn should_delete_allows_self() {
    assert!(should_delete_media(
        &media("1", "me", 1_700_000_000),
        "me",
        None,
        None
    ));
    assert!(should_delete_dm(
        &dm("i", "me", 1_700_000_000_000_000),
        "me",
        None,
        None
    ));
}

#[test]
fn should_delete_date_window() {
    let m = media("1", "me", 1_700_000_000);
    let before = parse_to_datetime("2024-01-01", crate::cleaner::dates::DateBound::After).unwrap();
    let after = parse_to_datetime("2023-01-01", crate::cleaner::dates::DateBound::After).unwrap();
    assert!(should_delete_media(&m, "me", Some(before), Some(after)));
    assert!(!should_delete_media(
        &m,
        "me",
        Some(parse_to_datetime("2023-01-01", crate::cleaner::dates::DateBound::After).unwrap()),
        None
    ));
}

#[test]
fn unparseable_timestamp_fails_closed() {
    assert!(!should_delete_media(&media("1", "me", 0), "me", None, None));
    assert!(!in_date_window(None, None, None));
}

#[test]
fn thread_kind_filter() {
    let one = thread("1", false, &["a"]);
    let grp = thread("2", true, &["a", "b"]);
    assert!(thread_matches_kind(&one, DmKind::OneOnOne));
    assert!(!thread_matches_kind(&one, DmKind::Group));
    assert!(thread_matches_kind(&grp, DmKind::Group));
    assert!(thread_matches_kind(&grp, DmKind::All));
}

#[test]
fn admit_thread_skips_kind_protected_and_duplicates() {
    let dm_kind = DmKind::OneOnOne;
    let protect: HashSet<&str> = ["@ub"].into_iter().collect();
    let mut seen = HashSet::new();
    let mut skips = Skips::default();

    let group = thread("g", true, &["a", "b"]);
    assert!(!admit_thread(
        &group, dm_kind, true, &protect, &mut seen, &mut skips
    ));
    assert_eq!(skips.kind, 1);

    let guarded = thread("p", false, &["a", "b"]);
    assert!(!admit_thread(
        &guarded, dm_kind, true, &protect, &mut seen, &mut skips
    ));
    assert_eq!(skips.protected, 1);

    let ok = thread("t1", false, &["a", "c"]);
    assert!(admit_thread(
        &ok, dm_kind, true, &protect, &mut seen, &mut skips
    ));
    assert!(
        !admit_thread(&ok, dm_kind, true, &protect, &mut seen, &mut skips),
        "same thread id must not be admitted twice"
    );

    let named_group = thread("g2", true, &["a"]);
    assert!(admit_thread(
        &named_group,
        dm_kind,
        false,
        &protect,
        &mut seen,
        &mut skips
    ));
}

#[test]
fn planned_delete_to_record() {
    let p = PlannedDelete {
        target_id: "posts".into(),
        target_name: "Your posts".into(),
        item_id: "1_2".into(),
        kind: ItemKind::Media,
        preview: "hi".into(),
        author_id: "me".into(),
        author_name: "me".into(),
        content: "hi".into(),
        timestamp: "2024-01-01T00:00:00Z".into(),
        media_type: Some("photo".into()),
        original_message_client_context: None,
        attachments: vec![],
    };
    let r = p.to_record(crate::history::DeleteOutcome::DryRun);
    assert_eq!(r.target_id, "posts");
    assert_eq!(r.item_id, "1_2");
    assert_eq!(r.kind, "media");
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
    let seen: HashSet<super::ItemKey> = first.entries.iter().map(PlannedDelete::key).collect();

    // a verify re-scan that still lists an already-deleted item must drop it
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
    seen.insert(planned("t1", "m1").key());
    drop_already_seen(&mut plan, &seen);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].item_id, "m2");
}

#[test]
fn a_message_and_its_reaction_are_separate_items() {
    let mut message = planned("t", "m");
    message.kind = ItemKind::Dm;
    let mut reaction = planned("t", "m");
    reaction.kind = ItemKind::DmReaction;

    let mut plan = Plan {
        entries: vec![reaction.clone(), message.clone(), reaction.clone()],
        ..Default::default()
    };
    finalize_plan(&mut plan);
    assert_eq!(plan.entries.len(), 2);

    // removing the reaction must not mark the message as already handled
    let seen: HashSet<super::ItemKey> = [reaction.key()].into_iter().collect();
    drop_already_seen(&mut plan, &seen);
    assert_eq!(plan.entries.len(), 1);
    assert_eq!(plan.entries[0].kind, ItemKind::Dm);
}

const SENT_US: i64 = 1_600_000_000_000_000;
const REACTED_US: i64 = 1_700_000_000_000_000;

fn reacted(author: &str, reactions: serde_json::Value) -> DirectMessage {
    let mut msg = dm("m", author, SENT_US);
    msg.extra.insert("reactions".into(), reactions);
    msg
}

fn emoji_reaction(sender: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "emojis": [{ "sender_id": sender, "emoji": "🔥", "timestamp": REACTED_US.to_string() }]
    })
}

fn own_fire() -> DmAction {
    DmAction::RemoveReaction(OwnReaction {
        emoji: "🔥".into(),
        timestamp_us: Some(REACTED_US),
    })
}

#[test]
fn own_reaction_reads_string_and_numeric_senders() {
    for sender in [serde_json::json!("123"), serde_json::json!(123)] {
        let msg = reacted("x", emoji_reaction(sender));
        let reaction = msg.own_reaction("123").expect("own reaction");
        assert_eq!(reaction.emoji, "🔥");
        assert_eq!(reaction.timestamp_us, Some(REACTED_US));
    }
}

#[test]
fn own_reaction_ignores_other_people() {
    let msg = reacted("x", emoji_reaction(serde_json::json!("456")));
    assert_eq!(msg.own_reaction("123"), None);
    assert_eq!(msg.own_reaction(""), None);
    assert_eq!(dm("m", "x", SENT_US).own_reaction("123"), None);
}

#[test]
fn own_reaction_falls_back_to_the_legacy_like_list() {
    let msg = reacted(
        "x",
        serde_json::json!({ "likes": [{ "sender_id": 123, "timestamp": REACTED_US }], "emojis": [] }),
    );
    let reaction = msg.own_reaction("123").expect("own like");
    assert_eq!(reaction.emoji, "❤️");
    assert_eq!(reaction.timestamp_us, Some(REACTED_US));
}

#[test]
fn dm_action_unsend_covers_your_reaction_on_your_own_message() {
    let msg = reacted("me", emoji_reaction(serde_json::json!("me")));
    let rules = DmRules {
        unsend: true,
        reactions: true,
        ..Default::default()
    };
    assert_eq!(dm_action(&msg, "me", &rules), Some(DmAction::Unsend));
}

#[test]
fn dm_action_reactions_only_never_unsends() {
    let rules = DmRules {
        reactions: true,
        ..Default::default()
    };
    let own_message = reacted("me", emoji_reaction(serde_json::json!("me")));
    assert_eq!(dm_action(&own_message, "me", &rules), Some(own_fire()));
    assert_eq!(dm_action(&dm("m", "me", SENT_US), "me", &rules), None);

    let their_message = reacted("x", emoji_reaction(serde_json::json!("me")));
    assert_eq!(dm_action(&their_message, "me", &rules), Some(own_fire()));
}

#[test]
fn dm_action_removes_the_reaction_when_media_only_keeps_the_message() {
    let msg = reacted("me", emoji_reaction(serde_json::json!("me")));
    let rules = DmRules {
        unsend: true,
        media_only: true,
        reactions: true,
        ..Default::default()
    };
    assert_eq!(dm_action(&msg, "me", &rules), Some(own_fire()));
}

#[test]
fn dm_action_ignores_reactions_when_they_are_off() {
    let msg = reacted("x", emoji_reaction(serde_json::json!("me")));
    assert_eq!(dm_action(&msg, "me", &DmRules::default()), None);
}

#[test]
fn dm_action_dates_a_reaction_by_when_it_was_made() {
    let msg = reacted("x", emoji_reaction(serde_json::json!("me")));
    let between = dm_timestamp_us((SENT_US + REACTED_US) / 2);

    let after = DmRules {
        reactions: true,
        after: between,
        ..Default::default()
    };
    assert_eq!(dm_action(&msg, "me", &after), Some(own_fire()));

    let before = DmRules {
        reactions: true,
        before: between,
        ..Default::default()
    };
    assert_eq!(dm_action(&msg, "me", &before), None);
}

#[test]
fn target_count_gives_reactions_no_pass_of_their_own() {
    let resolved = ResolvedTargets {
        threads: vec![thread("1", false, &["a"])],
        content: crate::config::ContentFlags {
            likes: true,
            dm_reactions: true,
            ..Default::default()
        },
        ..Default::default()
    };
    assert_eq!(target_count(&resolved), 2);
}

#[test]
fn only_explicit_dm_targets_count_as_dm_scope() {
    assert!(!has_dm_scope(&[]));
    assert!(!has_dm_scope(&[TargetResolution::Posts]));
    assert!(has_dm_scope(&[TargetResolution::AllDms]));
    assert!(has_dm_scope(&[TargetResolution::Threads(vec!["1".into()])]));
}

#[test]
fn is_retryable_delete_error_only_5xx_and_http() {
    assert!(is_retryable_delete_error(&IgError::Api {
        status: 502,
        message: "bad".into()
    }));
    assert!(!is_retryable_delete_error(&IgError::Api {
        status: 400,
        message: "bad".into()
    }));
    assert!(!is_retryable_delete_error(&IgError::NotFound {
        resource: "x".into(),
        status: 404
    }));
    assert!(is_retryable_delete_error(&IgError::FeedbackRequired {
        message: "wait".into()
    }));
}

#[test]
fn delete_errors_map_to_delete_steps() {
    assert_eq!(
        classify_delete_error(&IgError::NotFound {
            resource: "x".into(),
            status: 404
        }),
        DeleteStep::AlreadyGone
    );
    assert_eq!(
        classify_delete_error(&IgError::Forbidden {
            resource: "x".into(),
            status: 403
        }),
        DeleteStep::Forbidden
    );
    assert_eq!(
        classify_delete_error(&IgError::Api {
            status: 500,
            message: "x".into()
        }),
        DeleteStep::Failed
    );
}

#[test]
fn only_previously_unresolved_items_count_as_retries() {
    let mut settled = Settled::default();
    settled.failed.insert(planned("t", "failed").key());
    settled.skipped.insert(planned("t", "refused").key());
    let plan = Plan {
        entries: vec![
            planned("t", "failed"),
            planned("t", "refused"),
            planned("t", "brand-new"),
        ],
        ..Plan::default()
    };

    assert_eq!(settled.count_new(&plan), 1);
    assert_eq!(
        settled.take_retries(&plan),
        [crate::events::RetryCount {
            target_id: "t".into(),
            failed: 1,
            skipped: 1,
        }]
    );
    // the outcome is forgotten, so the next pass does not subtract it a second time
    assert!(settled.take_retries(&plan).is_empty());
}

#[test]
fn record_outcome_accounts_deletes_skips_and_failures() {
    let mut channel = ChannelStats::default();
    let mut stats = RunStats::default();
    let mut succeeded = HashSet::new();

    // dry-run and real DM deletes take the identical accounting path
    let mut message = planned("t", "m1");
    message.kind = ItemKind::Dm;
    record_outcome(
        &message,
        DeleteStep::Deleted,
        &mut channel,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel.deleted, 1);
    assert_eq!(stats.dm_messages_deleted, 1);
    assert!(succeeded.contains(&message.key()));

    // NotFound counts as gone but is not an unsent DM
    record_outcome(
        &planned("t", "m2"),
        DeleteStep::AlreadyGone,
        &mut channel,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel.deleted, 2);
    assert_eq!(stats.dm_messages_deleted, 1);
    assert!(succeeded.contains(&planned("t", "m2").key()));

    record_outcome(
        &planned("t", "m3"),
        DeleteStep::Forbidden,
        &mut channel,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel.skipped, 1);
    assert!(!succeeded.contains(&planned("t", "m3").key()));

    record_outcome(
        &planned("t", "m4"),
        DeleteStep::Failed,
        &mut channel,
        &mut stats,
        &mut succeeded,
    );
    assert_eq!(channel.failed, 1);
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
fn from_media_builds_planned() {
    let m = media("99", "me", 1_700_000_000);
    let p = PlannedDelete::from_media(&m, "me");
    assert_eq!(p.target_id, "posts");
    assert_eq!(p.kind, ItemKind::Media);
    assert_eq!(p.item_id, "99_me");
    assert!(!p.preview.is_empty());
}

#[test]
fn from_dm_builds_planned() {
    let m = dm("item1", "me", 1_700_000_000_000_000);
    let p = PlannedDelete::from_dm(&m, "tid", "Alice", "me", "me");
    assert_eq!(p.target_id, "tid");
    assert_eq!(p.kind, ItemKind::Dm);
    assert_eq!(p.item_id, "item1");
    assert_eq!(p.content, "hello");
}

#[test]
fn map_delete_bool_false_is_err() {
    assert!(map_delete_bool(true, "x").is_ok());
    let err = map_delete_bool(false, "media delete").unwrap_err();
    assert!(!is_retryable_delete_error(&err));
}

#[test]
fn media_attachment_treats_a_null_reel_field_as_absent() {
    let mut photo = dm("m", "me", SENT_US);
    photo.item_type = Some("media".into());
    photo.extra.insert(
        "media".into(),
        serde_json::json!({ "media_type": 1, "clips_metadata": null }),
    );
    assert!(photo.is_media_attachment());

    photo.extra.insert(
        "media".into(),
        serde_json::json!({ "media_type": 2, "clips_metadata": {} }),
    );
    assert!(!photo.is_media_attachment());
}

#[test]
fn media_only_deletes_everything_except_text_reels_and_posts() {
    let media_only = DmRules {
        unsend: true,
        media_only: true,
        ..Default::default()
    };
    let of_type = |item_type: &str| {
        let mut msg = dm("m", "me", SENT_US);
        msg.item_type = Some(item_type.into());
        msg
    };

    for deleted in [
        "media",
        "visual_media",
        "voice_media",
        "animated_media",
        "raven_media",
        "story_share",
        "xma_story_share",
        "profile",
        "xma_profile",
        "location",
    ] {
        assert_eq!(
            dm_action(&of_type(deleted), "me", &media_only),
            Some(DmAction::Unsend),
            "{deleted} should be unsent"
        );
    }

    for kept in [
        "text",
        "link",
        "like",
        "placeholder",
        "reel_share",
        "clip",
        "xma_clip",
        "xma_reel_share",
        "media_share",
        "xma_media_share",
        "felix_share",
        "generic_xma",
        "",
    ] {
        assert_eq!(
            dm_action(&of_type(kept), "me", &media_only),
            None,
            "{kept:?} should be kept"
        );
    }

    // a message with no type at all is never unsent under the filter
    let mut untyped = dm("m", "me", SENT_US);
    untyped.item_type = None;
    assert_eq!(dm_action(&untyped, "me", &media_only), None);

    // the same reel-free check still catches a Reel sent as a media attachment
    let mut reel = of_type("media");
    reel.extra.insert(
        "media".into(),
        serde_json::json!({ "product_type": "clips" }),
    );
    assert_eq!(dm_action(&reel, "me", &media_only), None);
}

#[test]
fn from_dm_stores_humanized_media_type() {
    let mut m = dm("item1", "me", 1_700_000_000_000_000);
    m.item_type = Some("voice_media".into());
    m.text = None;
    let p = PlannedDelete::from_dm(&m, "tid", "Alice", "me", "me");
    assert!(p.media_type.as_deref().is_some_and(|s| !s.is_empty()));
    assert!(!p.preview.is_empty());

    m.item_type = Some("action_log".into());
    let p = PlannedDelete::from_dm(&m, "tid", "Alice", "me", "me");
    assert!(p.media_type.as_deref().is_some_and(|s| !s.is_empty()));
}

mod unfollow {
    use std::collections::HashSet;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use chrono::Utc;
    use tokio::sync::mpsc;

    use crate::client::InstagramClient;
    use crate::error::IgError;
    use crate::events::CleanerEvent;
    use crate::events::LogLevel;
    use crate::history::{DeleteOutcome, DeletedRecord, HistoryScope, HistoryStore};
    use crate::model::{FollowListResponse, FollowStatus, Friendship, UserShort};
    use crate::ratelimit::RateLimiter;

    use super::super::execute::{blocks_further_unfollows, execute_plan};
    use super::super::filters::unfollow_candidates;
    use super::super::resolve::ResolvedTargets;
    use super::super::scan::{build_plan, RunNotes, ScanCtx, ScanLimits};
    use super::super::{ContentFlags, ItemKind, Plan, PlannedDelete};

    const SELF_ID: &str = "1";

    type Requests = Arc<Mutex<Vec<String>>>;

    /// Answers each request from `route`, which gets "METHOD path" without the query.
    fn fake_instagram(route: fn(&str) -> (u16, String)) -> (String, Requests) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}/api/v1", listener.local_addr().unwrap());
        let requests: Requests = Arc::default();
        let log = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let head = read_request(&mut stream);
                let mut parts = head.split_whitespace();
                let method = parts.next().unwrap_or_default();
                let target = parts.next().unwrap_or_default();
                let full = format!("{method} {}", target.trim_start_matches("/api/v1/"));
                let request = full.split('?').next().unwrap_or_default().to_string();
                let (status, body) = route(&full);
                if !request.contains("current_user") {
                    log.lock().unwrap().push(request);
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (base_url, requests)
    }

    fn read_request(stream: &mut std::net::TcpStream) -> String {
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap_or(0);
            raw.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&raw);
            if let Some(end) = text.find("\r\n\r\n") {
                let body_len = text[..end]
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())?
                    })
                    .unwrap_or(0);
                if n == 0 || raw.len() >= end + 4 + body_len {
                    return text[..end].to_string();
                }
            } else if n == 0 {
                return text.into_owned();
            }
        }
    }

    fn users(ids: &[&str]) -> String {
        let list: Vec<String> = ids
            .iter()
            .map(|id| format!(r#"{{"pk":{id},"username":"user{id}","full_name":"U"}}"#))
            .collect();
        list.join(",")
    }

    fn ok_user() -> (u16, String) {
        (
            200,
            format!(r#"{{"user":{{"pk":{SELF_ID},"username":"me"}},"status":"ok"}}"#),
        )
    }

    fn friendship(following: bool, followed_by: bool) -> (u16, String) {
        (
            200,
            format!(r#"{{"following":{following},"followed_by":{followed_by},"status":"ok"}}"#),
        )
    }

    fn temp_dir() -> std::path::PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "instagram-cleaner-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    async fn client_for(base_url: &str) -> InstagramClient {
        let dir = temp_dir();
        let session = dir.join("session.json");
        let limiter = Arc::new(RateLimiter::new(
            Duration::from_millis(1),
            Arc::new(AtomicBool::new(false)),
        ));
        InstagramClient::new(
            &format!("{SELF_ID}%3Aabcdefghijklmnopqrstuvwxyz%3A1"),
            Some(base_url),
            Some(session.to_str().unwrap()),
            limiter,
        )
        .await
        .expect("client against the fake server")
    }

    async fn plan_with(
        route: fn(&str) -> (u16, String),
        protect: &[String],
        dated: bool,
    ) -> (Plan, Vec<String>) {
        let (plan, requests, _, _) = plan_and_logs(route, protect, dated).await;
        (plan, requests)
    }

    async fn plan_and_logs(
        route: fn(&str) -> (u16, String),
        protect: &[String],
        dated: bool,
    ) -> (Plan, Vec<String>, Vec<(bool, String)>, Vec<DeletedRecord>) {
        let root = temp_dir();
        let notes = RunNotes::new(HistoryStore::with_root(&root), true);
        let (base_url, requests) = fake_instagram(route);
        let client = client_for(&base_url).await;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, mut rx) = mpsc::channel::<CleanerEvent>(4096);
        let ctx = ScanCtx {
            client: &client,
            dm_unsend: false,
            dm_media_only: false,
            dm_reactions: false,
            protect_users: protect,
            notes: &notes,
            before: dated.then(Utc::now),
            after: None,
            limits: ScanLimits::first_pass(2),
            cancel: &cancel,
            tx: &tx,
        };
        let scope = ResolvedTargets {
            content: ContentFlags {
                unfollow: true,
                ..ContentFlags::default()
            },
            ..ResolvedTargets::default()
        };
        let plan = build_plan(&ctx, &scope, 0).await;
        let seen = requests.lock().unwrap().clone();
        let mut logs = Vec::new();
        while let Ok(event) = rx.try_recv() {
            match event {
                CleanerEvent::Log { level, message } => {
                    logs.push((matches!(level, LogLevel::Warning), message));
                }
                CleanerEvent::Warning { message } => logs.push((true, message)),
                _ => {}
            }
        }
        let records = HistoryStore::with_root(&root)
            .load_day(
                Utc::now().date_naive(),
                HistoryScope::DryRun,
                Some("unfollow"),
            )
            .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        (plan, seen, logs, records)
    }

    fn planned_ids(plan: &Plan) -> Vec<&str> {
        plan.entries.iter().map(|e| e.item_id.as_str()).collect()
    }

    /// Follows 2..=6 across two pages; only 2 follows back. 3 is one-sided, 4 turns
    /// out to follow back, 5 gets a reply without the facts, 6 is protected in the
    /// test using it, 7 can't be looked up at all.
    fn healthy_route(request: &str) -> (u16, String) {
        let path = request.split('?').next().unwrap();
        match path {
            "GET accounts/current_user/" => ok_user(),
            "GET friendships/1/followers/" => (200, format!(r#"{{"users":[{}]}}"#, users(&["2"]))),
            "GET friendships/1/following/" if request.contains("max_id=50") => (
                200,
                format!(r#"{{"users":[{}]}}"#, users(&["5", "6", "1", "7"])),
            ),
            "GET friendships/1/following/" => (
                200,
                format!(
                    r#"{{"users":[{}],"next_max_id":50}}"#,
                    users(&["2", "3", "4", "3"])
                ),
            ),
            "GET friendships/show/3/" | "GET friendships/show/6/" => friendship(true, false),
            "GET friendships/show/4/" => friendship(true, true),
            "GET friendships/show/5/" => (200, r#"{"status":"ok"}"#.to_string()),
            _ => (400, r#"{"message":"unexpected"}"#.to_string()),
        }
    }

    #[tokio::test]
    async fn plans_only_confirmed_one_sided_follows() {
        let protect = vec!["@USER6".to_string()];
        let (plan, requests) = plan_with(healthy_route, &protect, false).await;

        assert_eq!(planned_ids(&plan), ["3"]);
        let entry = &plan.entries[0];
        assert_eq!(entry.kind, ItemKind::Unfollow);
        assert_eq!(entry.author_name, "user3");
        assert!(entry.preview.contains("@user3"));
        assert!(
            !requests.iter().any(|r| r.contains("show/6")),
            "{requests:?}"
        );
        assert!(
            !requests.iter().any(|r| r.contains("show/2")),
            "{requests:?}"
        );
        assert!(
            !requests.iter().any(|r| r.starts_with("POST")),
            "{requests:?}"
        );
    }

    #[tokio::test]
    async fn accounts_that_follow_back_are_kept_and_unseen_ones_are_warned_by_name() {
        let protect = vec!["user6".to_string()];
        let (plan, _, logs, records) = plan_and_logs(healthy_route, &protect, false).await;

        let saved: Vec<&DeletedRecord> = records
            .iter()
            .filter(|r| r.outcome == DeleteOutcome::Warning)
            .collect();
        assert_eq!(saved.len(), 2, "{records:?}");
        assert!(saved
            .iter()
            .any(|r| r.author_name == "user5" && r.preview.starts_with("WARNING @user5")));
        assert!(saved.iter().any(|r| r.author_name == "user7"));
        let summary = records
            .iter()
            .find(|r| r.outcome == DeleteOutcome::Summary)
            .expect("summary saved");
        for part in [
            "1 listed followers",
            "8 following",
            "1 to unfollow",
            "2 follow you back",
            "1 protected",
            "2 warning(s)",
        ] {
            assert!(
                summary.preview.contains(part),
                "{part}: {}",
                summary.preview
            );
        }

        assert_eq!(planned_ids(&plan), ["3"]);
        let warnings: Vec<&str> = logs
            .iter()
            .filter(|(warning, _)| *warning)
            .map(|(_, message)| message.as_str())
            .collect();
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings.iter().all(|w| w.starts_with("WARNING @")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("@user5")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("@user7")),
            "{warnings:?}"
        );
        assert!(!logs.is_empty());
    }

    #[tokio::test]
    async fn an_unread_list_is_saved_as_a_warning() {
        fn route(request: &str) -> (u16, String) {
            match request.split('?').next().unwrap() {
                "GET accounts/current_user/" => ok_user(),
                _ => (400, r#"{"message":"nope"}"#.to_string()),
            }
        }
        let (_, _, _, records) = plan_and_logs(route, &[], false).await;
        assert_eq!(records.len(), 1, "{records:?}");
        assert_eq!(records[0].outcome, DeleteOutcome::Warning);
        assert!(records[0].preview.contains("followers list"), "{records:?}");
    }

    #[test]
    fn a_warning_is_only_kept_once_per_run() {
        let notes = RunNotes::new(HistoryStore::with_root(temp_dir()), true);
        assert!(notes.first_warning("5"));
        assert!(!notes.first_warning("5"));
        assert!(notes.first_warning("7"));
    }

    #[tokio::test]
    async fn a_broken_follower_page_plans_nothing() {
        fn route(request: &str) -> (u16, String) {
            match request.split('?').next().unwrap() {
                "GET accounts/current_user/" => ok_user(),
                "GET friendships/1/followers/" if request.contains("max_id=9") => {
                    (400, r#"{"message":"nope"}"#.to_string())
                }
                "GET friendships/1/followers/" => (
                    200,
                    format!(r#"{{"users":[{}],"next_max_id":9}}"#, users(&["2"])),
                ),
                "GET friendships/1/following/" => {
                    (200, format!(r#"{{"users":[{}]}}"#, users(&["2", "3"])))
                }
                _ => friendship(true, false),
            }
        }
        let (plan, requests) = plan_with(route, &[], false).await;
        assert!(plan.entries.is_empty());
        assert!(
            !requests.iter().any(|r| r.contains("show/")),
            "{requests:?}"
        );
    }

    #[tokio::test]
    async fn a_follower_body_without_users_plans_nothing() {
        fn route(request: &str) -> (u16, String) {
            match request.split('?').next().unwrap() {
                "GET accounts/current_user/" => ok_user(),
                "GET friendships/1/followers/" => (200, r#"{"status":"ok"}"#.to_string()),
                "GET friendships/1/following/" => {
                    (200, format!(r#"{{"users":[{}]}}"#, users(&["3"])))
                }
                _ => friendship(true, false),
            }
        }
        let (plan, _) = plan_with(route, &[], false).await;
        assert!(plan.entries.is_empty());
    }

    #[tokio::test]
    async fn a_silently_short_follower_list_is_caught_by_confirmation() {
        fn route(request: &str) -> (u16, String) {
            match request.split('?').next().unwrap() {
                "GET accounts/current_user/" => ok_user(),
                "GET friendships/1/followers/" => (200, r#"{"users":[]}"#.to_string()),
                "GET friendships/1/following/" => {
                    (200, format!(r#"{{"users":[{}]}}"#, users(&["2", "3"])))
                }
                _ => friendship(true, true),
            }
        }
        let (plan, _) = plan_with(route, &[], false).await;
        assert!(plan.entries.is_empty());
    }

    #[tokio::test]
    async fn date_filters_skip_the_pass_without_reading_anything() {
        let (plan, requests) = plan_with(healthy_route, &[], true).await;
        assert!(plan.entries.is_empty());
        assert!(requests.is_empty(), "{requests:?}");
    }

    fn planned_unfollow(id: &str) -> PlannedDelete {
        PlannedDelete {
            target_id: "unfollow".into(),
            target_name: "Not following back".into(),
            item_id: id.into(),
            kind: ItemKind::Unfollow,
            ..PlannedDelete::default()
        }
    }

    #[tokio::test]
    async fn a_block_stops_the_remaining_unfollows() {
        fn route(request: &str) -> (u16, String) {
            match request.split('?').next().unwrap() {
                "GET accounts/current_user/" => ok_user(),
                _ => (400, r#"{"message":"challenge_required"}"#.to_string()),
            }
        }
        let (base_url, requests) = fake_instagram(route);
        let client = client_for(&base_url).await;
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = mpsc::channel::<CleanerEvent>(64);
        let plan = Plan {
            entries: vec![planned_unfollow("7"), planned_unfollow("8")],
            ..Plan::default()
        };

        let history = HistoryStore::with_root(temp_dir());
        let (stats, items) = execute_plan(&client, plan, false, &history, &tx, &cancel).await;

        assert!(stats.unfollow_blocked);
        assert!(items.succeeded.is_empty());
        assert_eq!(stats.messages_failed, 1);
        // the second account was never tried, which is not the same as refused
        assert_eq!(stats.messages_skipped, 0);
        assert_eq!(stats.plan_total, 2);
        assert_eq!(
            *requests.lock().unwrap(),
            ["POST friendships/destroy/7/".to_string()]
        );
    }

    #[tokio::test]
    async fn unfollow_only_counts_when_instagram_says_so() {
        fn route(request: &str) -> (u16, String) {
            match request.split('?').next().unwrap() {
                "GET accounts/current_user/" => ok_user(),
                "POST friendships/destroy/7/" => (
                    200,
                    r#"{"friendship_status":{"following":false},"status":"ok"}"#.to_string(),
                ),
                "POST friendships/destroy/8/" => (
                    200,
                    r#"{"friendship_status":{"following":true},"status":"ok"}"#.to_string(),
                ),
                _ => (200, r#"{"status":"ok"}"#.to_string()),
            }
        }
        let (base_url, _) = fake_instagram(route);
        let client = client_for(&base_url).await;
        assert!(client.user_unfollow("7").await.unwrap());
        assert!(!client.user_unfollow("8").await.unwrap());
        assert!(!client.user_unfollow("9").await.unwrap());
    }

    #[test]
    fn candidates_leave_out_followers_self_duplicates_and_protected() {
        let user = |pk: &str, username: &str| UserShort {
            pk: pk.into(),
            username: username.into(),
            full_name: String::new(),
        };
        let following = vec![
            user("1", "me"),
            user("2", "friend"),
            user("3", "brand"),
            user("3", "brand"),
            user("4", "Keeper"),
            user("5", "byid"),
            user("", "nopk"),
        ];
        let followers: HashSet<String> = ["2".to_string()].into();
        let protect = vec!["@keeper".to_string(), "5".to_string()];

        let (kept, protected) = unfollow_candidates(following, &followers, "1", &protect);

        let ids: Vec<&str> = kept.iter().map(|u| u.pk.as_str()).collect();
        assert_eq!(ids, ["3"]);
        assert_eq!(protected, 2);
    }

    #[test]
    fn friendship_must_state_both_sides() {
        let parse = |raw: &str| serde_json::from_str::<Friendship>(raw).unwrap();
        let status = |raw: &str| parse(raw).status();
        assert_eq!(
            status(r#"{"following":true,"followed_by":false}"#),
            FollowStatus::OneSided
        );
        assert_eq!(
            status(r#"{"following":true,"followed_by":true}"#),
            FollowStatus::FollowsBack
        );
        assert_eq!(status(r#"{"followed_by":true}"#), FollowStatus::FollowsBack);
        assert_eq!(
            status(r#"{"following":false,"followed_by":false}"#),
            FollowStatus::NotFollowing
        );
        assert_eq!(status(r#"{"following":true}"#), FollowStatus::Unknown);
        assert_eq!(status(r#"{"followed_by":false}"#), FollowStatus::Unknown);
        assert_eq!(status(r#"{"status":"ok"}"#), FollowStatus::Unknown);
    }

    #[test]
    fn follow_list_requires_users_and_accepts_numeric_cursors() {
        assert!(serde_json::from_str::<FollowListResponse>(r#"{"status":"ok"}"#).is_err());
        let page: FollowListResponse =
            serde_json::from_str(r#"{"users":[],"next_max_id":200}"#).unwrap();
        assert_eq!(page.next_max_id.as_deref(), Some("200"));
    }

    #[test]
    fn only_real_refusals_stop_unfollows() {
        let refused = IgError::FeedbackRequired {
            message: "slow down".into(),
        };
        assert!(blocks_further_unfollows(&refused));
        assert!(!blocks_further_unfollows(&IgError::Other("x".into())));
    }

    fn post_json(pk: u32) -> String {
        format!(
            r#"{{"pk":{pk},"id":"{pk}_{SELF_ID}","taken_at":1700000000,"media_type":1,"user":{{"pk":{SELF_ID},"username":"me"}}}}"#
        )
    }

    fn feed_json(pks: &[u32]) -> (u16, String) {
        let items: Vec<String> = pks.iter().map(|pk| post_json(*pk)).collect();
        (
            200,
            format!(
                r#"{{"items":[{}],"more_available":false,"status":"ok"}}"#,
                items.join(",")
            ),
        )
    }

    fn deleted_json() -> (u16, String) {
        (200, r#"{"did_delete":true,"status":"ok"}"#.to_string())
    }

    fn posts_config(history_dir: &std::path::Path) -> crate::cleaner::CleanerConfig {
        crate::cleaner::CleanerConfig {
            targets: vec![crate::cleaner::TargetResolution::Posts],
            content: ContentFlags::default(),
            before: None,
            after: None,
            limit: 0,
            dry_run: false,
            throttle_ms: 1,
            scan_throttle_ms: 1,
            concurrency: 2,
            hide_threads: false,
            protect_users: Vec::new(),
            dm_kind: crate::config::DmKind::All,
            dm_media_only: false,
            unsend_dms: false,
            history_dir: history_dir.to_str().unwrap().to_string(),
        }
    }

    async fn run_collecting(
        route: fn(&str) -> (u16, String),
        config: crate::cleaner::CleanerConfig,
    ) -> (crate::events::RunStats, Vec<CleanerEvent>) {
        let (stats, events, _requests) = run_logged(route, config).await;
        (stats, events)
    }

    async fn run_logged(
        route: fn(&str) -> (u16, String),
        config: crate::cleaner::CleanerConfig,
    ) -> (crate::events::RunStats, Vec<CleanerEvent>, Vec<String>) {
        let (base_url, requests) = fake_instagram(route);
        let dir = temp_dir();
        let session = dir.join("session.json");
        let cancel = Arc::new(AtomicBool::new(false));
        let limiter = Arc::new(RateLimiter::new(Duration::from_millis(1), cancel.clone()));
        let client = InstagramClient::new(
            &format!("{SELF_ID}%3Aabcdefghijklmnopqrstuvwxyz%3A1"),
            Some(&base_url),
            Some(session.to_str().unwrap()),
            limiter.clone(),
        )
        .await
        .expect("client against the fake server");
        let (tx, mut rx) = mpsc::channel::<CleanerEvent>(4096);
        let stats = crate::cleaner::run(client, config, limiter, tx, cancel).await;
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        let requests = requests.lock().unwrap().clone();
        (stats, events, requests)
    }

    // 11 deletes, 12 is always refused, 13 fails once and deletes on the retry.
    static DELETED_11: AtomicBool = AtomicBool::new(false);
    static DELETED_13: AtomicBool = AtomicBool::new(false);
    static ATTEMPTS_13: AtomicU64 = AtomicU64::new(0);

    fn mixed_outcomes_route(request: &str) -> (u16, String) {
        if request.contains("current_user") {
            return ok_user();
        }
        if request.starts_with("GET feed/user/") {
            let mut live = vec![12];
            if !DELETED_11.load(Ordering::Relaxed) {
                live.push(11);
            }
            if !DELETED_13.load(Ordering::Relaxed) {
                live.push(13);
            }
            return feed_json(&live);
        }
        if request.starts_with("POST media/11_") {
            DELETED_11.store(true, Ordering::Relaxed);
            return deleted_json();
        }
        if request.starts_with("POST media/12_") {
            return (403, r#"{"message":"not allowed"}"#.to_string());
        }
        if request.starts_with("POST media/13_") {
            if ATTEMPTS_13.fetch_add(1, Ordering::Relaxed) == 0 {
                return (400, r#"{"message":"try this one again"}"#.to_string());
            }
            DELETED_13.store(true, Ordering::Relaxed);
            return deleted_json();
        }
        (404, "{}".to_string())
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_whole_run_counts_each_item_once_and_the_screen_agrees() {
        let root = temp_dir();
        let (stats, events) = run_collecting(mixed_outcomes_route, posts_config(&root)).await;

        assert_eq!(stats.messages_scanned, 3, "re-scans do not count again");
        assert_eq!(stats.plan_total, 3, "retries are the same items");
        assert_eq!(stats.messages_deleted, 2);
        assert_eq!(stats.messages_skipped, 1);
        assert_eq!(stats.messages_failed, 0, "the failure was retried away");
        assert!(!stats.was_cancelled);

        // the history goes to the configured folder
        let records = HistoryStore::with_root(&root)
            .load_day(Utc::now().date_naive(), HistoryScope::Live, None)
            .unwrap();
        let deleted = records
            .iter()
            .filter(|r| r.outcome == DeleteOutcome::Deleted)
            .count();
        assert_eq!(deleted, 2);

        // what the screen counted live is what the run reports at the end
        let mut cfg = crate::config::Config::from_env();
        cfg.history_dir = root.to_str().unwrap().to_string();
        let mut app = crate::tui::state::AppState::new(cfg);
        app.run.status = crate::tui::state::RunStatus::Running;
        for event in events {
            app.apply_event(event);
        }
        let live = app.run.stats;
        assert_eq!(live.messages_scanned, stats.messages_scanned);
        assert_eq!(live.messages_deleted, stats.messages_deleted);
        assert_eq!(live.messages_skipped, stats.messages_skipped);
        assert_eq!(live.messages_failed, stats.messages_failed);
        assert_eq!(app.run.found, 3);
        assert_eq!(app.run.plan_done, 3);
        assert_eq!(app.run.not_attempted(), 0);
        assert!(app.run.warnings >= 1, "the refused post is still there");

        app.apply_event(CleanerEvent::Done { stats });
        assert_eq!(app.run.status, crate::tui::state::RunStatus::Completed);
        assert_eq!(app.run.stats.messages_deleted, 2);
    }

    static FEED_READS: AtomicU64 = AtomicU64::new(0);

    fn failed_rescan_route(request: &str) -> (u16, String) {
        if request.contains("current_user") {
            return ok_user();
        }
        if request.starts_with("GET feed/user/") {
            if FEED_READS.fetch_add(1, Ordering::Relaxed) == 0 {
                return feed_json(&[21]);
            }
            return (400, r#"{"message":"bad page"}"#.to_string());
        }
        deleted_json()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_rescan_does_not_claim_the_deletions_are_confirmed() {
        let (stats, events) = run_collecting(failed_rescan_route, posts_config(&temp_dir())).await;

        assert_eq!(stats.messages_deleted, 1);
        let mut warned = false;
        for event in events {
            match event {
                CleanerEvent::Warning { message } if message.contains("not confirmed") => {
                    warned = true;
                }
                CleanerEvent::Log { message, .. } => {
                    assert!(!message.contains("deletions confirmed"), "{message}");
                }
                _ => {}
            }
        }
        assert!(warned);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_run_with_an_unreadable_date_stops_instead_of_ignoring_the_filter() {
        let mut config = posts_config(&temp_dir());
        config.before = Some("not-a-date".into());
        let (stats, events) = run_collecting(failed_rescan_route, config).await;

        assert_eq!(stats.plan_total, 0);
        assert!(events
            .iter()
            .any(|event| matches!(event, CleanerEvent::Error { .. })));
    }

    // The client asks for compression by hand, and wreq only decodes it with its gzip
    // and deflate features: without them this builds fine and every real response breaks.
    #[tokio::test(flavor = "multi_thread")]
    async fn compressed_responses_are_decoded() {
        use base64::Engine;

        const GZIP: &str =
            "H4sIAAAAAAAC/6tWKi5JLCktVrJSys9W0lEqLU4tUrKqVirIBooYQgXyEnNTgdyS1OISoGxtLQAN1P/0NQAAAA==";
        const DEFLATE: &str =
            "eJyrViouSSwpLVayUsrPVtJRKi1OLVKyqlYqyFayUjKECuQl5qYqWSmVpBaXpBYp1dYCAMrOEU0=";

        for (encoding, body) in [("gzip", GZIP), ("deflate", DEFLATE)] {
            let body = base64::engine::general_purpose::STANDARD
                .decode(body)
                .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base_url = format!("http://{}/api/v1", listener.local_addr().unwrap());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    read_request(&mut stream);
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                         Content-Encoding: {encoding}\r\nContent-Length: {}\r\n\
                         Connection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(&body);
                }
            });

            let client = client_for(&base_url).await;
            assert_eq!(client.username(), "tester", "{encoding}");
        }
    }

    static T_CLEAN_GONE: AtomicBool = AtomicBool::new(false);
    static T_REFUSED_OTHER_GONE: AtomicBool = AtomicBool::new(false);
    static T_UNREAD_READS: AtomicU64 = AtomicU64::new(0);

    fn thread_json(id: &str, author: &str, items: &[&str]) -> (u16, String) {
        let items: Vec<String> = items
            .iter()
            .map(|item| {
                format!(
                    r#"{{"item_id":"{item}","user_id":"{author}","timestamp":1700000000000000,"item_type":"text","text":"hi"}}"#
                )
            })
            .collect();
        (
            200,
            format!(
                r#"{{"status":"ok","thread":{{"thread_id":"{id}","items":[{}],"has_older":false}}}}"#,
                items.join(",")
            ),
        )
    }

    // t_clean empties, t_refused keeps refusing, t_unread can't be read back,
    // t_lingers still shows a message Instagram said it unsent, t_idle has none of yours.
    fn hide_route(request: &str) -> (u16, String) {
        let ok = || (200, r#"{"status":"ok"}"#.to_string());
        if request.contains("current_user") {
            return ok_user();
        }
        if request.starts_with("GET direct_v2/threads/t_clean/") {
            let gone = T_CLEAN_GONE.load(Ordering::Relaxed);
            return thread_json("t_clean", SELF_ID, if gone { &[] } else { &["m1"] });
        }
        if request.starts_with("GET direct_v2/threads/t_refused/") {
            let gone = T_REFUSED_OTHER_GONE.load(Ordering::Relaxed);
            return thread_json(
                "t_refused",
                SELF_ID,
                if gone { &["m2"] } else { &["m2", "m6"] },
            );
        }
        if request.starts_with("GET direct_v2/threads/t_unread/") {
            if T_UNREAD_READS.fetch_add(1, Ordering::Relaxed) == 0 {
                return thread_json("t_unread", SELF_ID, &["m3"]);
            }
            return (400, r#"{"message":"bad page"}"#.to_string());
        }
        if request.starts_with("GET direct_v2/threads/t_lingers/") {
            return thread_json("t_lingers", SELF_ID, &["m4"]);
        }
        if request.starts_with("GET direct_v2/threads/t_idle/") {
            return thread_json("t_idle", "999", &["m5"]);
        }
        if request.contains("/items/m1/delete/") {
            T_CLEAN_GONE.store(true, Ordering::Relaxed);
            return ok();
        }
        if request.contains("/items/m6/delete/") {
            T_REFUSED_OTHER_GONE.store(true, Ordering::Relaxed);
            return ok();
        }
        if request.contains("/items/m2/delete/") {
            return (403, r#"{"message":"not allowed"}"#.to_string());
        }
        ok()
    }

    fn hide_config() -> crate::cleaner::CleanerConfig {
        let mut config = posts_config(&temp_dir());
        config.targets = vec![crate::cleaner::TargetResolution::Threads(
            ["t_clean", "t_refused", "t_unread", "t_lingers", "t_idle"]
                .map(String::from)
                .to_vec(),
        )];
        config.unsend_dms = true;
        config.hide_threads = true;
        config
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn each_thread_is_hidden_on_its_own_evidence() {
        let (stats, events, requests) = run_logged(hide_route, hide_config()).await;

        let hides: Vec<&String> = requests.iter().filter(|r| r.ends_with("/hide/")).collect();
        assert_eq!(
            hides,
            ["POST direct_v2/threads/t_clean/hide/"],
            "{requests:#?}"
        );
        assert_eq!(stats.messages_deleted, 4);
        assert_eq!(stats.messages_skipped, 1);
        assert!(events.iter().any(|event| matches!(
            event,
            CleanerEvent::Log { message, .. } if message.contains("keeping 3 thread(s)")
        )));

        let reads = |id: &str| {
            let path = format!("GET direct_v2/threads/{id}/");
            requests.iter().filter(|r| **r == path).count()
        };
        // the re-scan asks Instagram, instead of re-reading what it already held
        assert!(reads("t_clean") > 1, "{requests:#?}");
        // nothing of yours was there, so there is nothing to go back for
        assert_eq!(reads("t_idle"), 1, "{requests:#?}");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn filters_that_leave_messages_behind_hide_nothing() {
        for (before, limit, media_only, reason) in [
            (Some("2030-01-01"), 0, false, "date filters"),
            (None, 5, false, "item limit"),
            (None, 0, true, "Media only"),
        ] {
            let stats = crate::events::RunStats {
                dm_messages_deleted: 1,
                ..Default::default()
            };
            let mut config = hide_config();
            config.before = before.map(String::from);
            config.limit = limit;
            config.dm_media_only = media_only;
            let blocker = super::super::hide_blocker(&config, &stats).unwrap();
            assert!(blocker.contains(reason), "{blocker}");
        }

        let blocked = crate::events::RunStats {
            dm_messages_deleted: 1,
            unfollow_blocked: true,
            ..Default::default()
        };
        assert!(super::super::hide_blocker(&hide_config(), &blocked).is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn picks_obey_the_conversation_type_and_are_read_fresh() {
        let (base_url, _requests) = fake_instagram(|_| ok_user());
        let client = client_for(&base_url).await;
        let (tx, _rx) = mpsc::channel::<CleanerEvent>(64);
        let mut solo = super::thread("solo", false, &["2"]);
        solo.items = vec![super::dm("stale", SELF_ID, 1_700_000_000_000_000)];
        let group = super::thread("group", true, &["2", "3"]);

        let scope = super::super::resolve::resolve_targets(
            &client,
            &[crate::cleaner::TargetResolution::ThreadObjects(vec![
                solo, group,
            ])],
            ContentFlags::default(),
            &[],
            crate::config::DmKind::OneOnOne,
            true,
            &tx,
        )
        .await
        .unwrap();

        assert_eq!(scope.threads.len(), 1);
        assert_eq!(scope.threads[0].id(), "solo");
        assert!(scope.threads[0].items.is_empty(), "the old copy was kept");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dry_run_says_how_many_threads_it_would_hide() {
        let (base_url, requests) = fake_instagram(|_| ok_user());
        let client = client_for(&base_url).await;
        let (tx, mut rx) = mpsc::channel::<CleanerEvent>(64);
        let thread = crate::model::DirectThread {
            thread_id: "t1".into(),
            pk: None,
            thread_title: None,
            is_group: false,
            users: Vec::new(),
            items: Vec::new(),
            has_older: false,
            prev_cursor: None,
            oldest_cursor: None,
        };

        super::super::execute::hide_resolved_threads(&client, &[thread], true, &tx).await;

        drop(tx);
        let mut summary = String::new();
        while let Some(event) = rx.recv().await {
            if let CleanerEvent::Log { message, .. } = event {
                summary = message;
            }
        }
        assert!(summary.contains("would hide 1"), "{summary}");
        assert!(requests.lock().unwrap().is_empty(), "nothing was hidden");
    }
}
