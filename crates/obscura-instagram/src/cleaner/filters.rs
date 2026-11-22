use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::config::DmKind;
use crate::model::{DirectMessage, DirectThread, Media, OwnReaction, UserShort};

use super::dates;

pub(crate) fn first_protected_user(
    thread: &DirectThread,
    protect: &HashSet<&str>,
) -> Option<String> {
    let tid = thread.id();
    for p in protect {
        let p = p.trim_start_matches('@');
        if p == tid.as_str() || p == thread.thread_id.as_str() {
            return Some(thread.display_name());
        }
        for user in &thread.users {
            let uname = user.username.trim_start_matches('@');
            if p == user.pk.as_str() || p.eq_ignore_ascii_case(uname) {
                return Some(if user.username.is_empty() {
                    user.pk.clone()
                } else {
                    user.username.clone()
                });
            }
        }
    }
    None
}

pub(crate) fn user_is_protected(user: &UserShort, protect: &[String]) -> bool {
    protect.iter().any(|p| {
        let p = p.trim_start_matches('@');
        p == user.pk || p.eq_ignore_ascii_case(user.username.trim_start_matches('@'))
    })
}

/// Accounts you follow that are missing from `followers`, without duplicates.
/// Also returns how many were left out because they are protected.
pub(crate) fn unfollow_candidates(
    following: Vec<UserShort>,
    followers: &HashSet<String>,
    self_uid: &str,
    protect: &[String],
) -> (Vec<UserShort>, usize) {
    let mut seen = HashSet::new();
    let mut protected = 0;
    let candidates = following
        .into_iter()
        .filter(|user| {
            if user.pk.is_empty()
                || user.pk == self_uid
                || followers.contains(&user.pk)
                || !seen.insert(user.pk.clone())
            {
                return false;
            }
            if user_is_protected(user, protect) {
                protected += 1;
                return false;
            }
            true
        })
        .collect();
    (candidates, protected)
}

pub(crate) fn thread_matches_kind(thread: &DirectThread, kind: DmKind) -> bool {
    kind.matches(thread.is_group)
}

pub(crate) fn in_date_window(
    ts: Option<DateTime<Utc>>,
    before: Option<DateTime<Utc>>,
    after: Option<DateTime<Utc>>,
) -> bool {
    let Some(ts) = ts else {
        return false;
    };
    if let Some(b) = before {
        if ts >= b {
            return false;
        }
    }
    if let Some(a) = after {
        if ts <= a {
            return false;
        }
    }
    true
}

pub(crate) fn should_delete_media(
    media: &Media,
    self_user_id: &str,
    before: Option<DateTime<Utc>>,
    after: Option<DateTime<Utc>>,
) -> bool {
    let author = media.author_id();
    if self_user_id.is_empty() || author.is_empty() || author != self_user_id {
        return false;
    }
    in_date_window(dates::media_taken_at(media.taken_at), before, after)
}

fn is_non_unsendable_dm_type(item_type: Option<&str>) -> bool {
    let t = item_type.unwrap_or("").trim();
    if t.is_empty() {
        return false;
    }
    matches!(
        t,
        "action_log"
            | "video_call_event"
            | "voice_call_event"
            | "call"
            | "video_call"
            | "temporary_status"
            | "roll_call"
    ) || t.ends_with("_log")
}

pub(crate) fn should_delete_dm(
    msg: &DirectMessage,
    self_user_id: &str,
    before: Option<DateTime<Utc>>,
    after: Option<DateTime<Utc>>,
) -> bool {
    if self_user_id.is_empty() || msg.user_id.as_deref() != Some(self_user_id) {
        return false;
    }
    if is_non_unsendable_dm_type(msg.item_type.as_deref()) {
        return false;
    }
    in_date_window(dates::dm_timestamp_us(msg.timestamp), before, after)
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct DmRules {
    pub unsend: bool,
    pub media_only: bool,
    pub reactions: bool,
    pub before: Option<DateTime<Utc>>,
    pub after: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DmAction {
    Unsend,
    RemoveReaction(OwnReaction),
}

/// At most one write per message: unsending also takes your reaction with it.
pub(crate) fn dm_action(
    msg: &DirectMessage,
    self_user_id: &str,
    rules: &DmRules,
) -> Option<DmAction> {
    if rules.unsend
        && should_delete_dm(msg, self_user_id, rules.before, rules.after)
        && (!rules.media_only || msg.is_media_attachment())
    {
        return Some(DmAction::Unsend);
    }
    if !rules.reactions {
        return None;
    }
    let reaction = msg.own_reaction(self_user_id)?;
    let reacted_at = reaction.timestamp_us.unwrap_or(msg.timestamp);
    in_date_window(
        dates::dm_timestamp_us(reacted_at),
        rules.before,
        rules.after,
    )
    .then_some(DmAction::RemoveReaction(reaction))
}

pub(crate) fn should_delete_comment(
    comment: &crate::model::Comment,
    self_user_id: &str,
    media_id: &str,
    before: Option<DateTime<Utc>>,
    after: Option<DateTime<Utc>>,
) -> bool {
    if self_user_id.is_empty() || comment.pk.trim().is_empty() || media_id.trim().is_empty() {
        return false;
    }
    if comment.author_id() != self_user_id {
        return false;
    }
    if comment.parent_comment_id.is_some() {
        return false;
    }
    in_date_window(dates::media_taken_at(comment.created_at), before, after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Caption, DirectMessage, UserShort};

    fn media(pk: &str, author: &str, taken_at: i64) -> Media {
        Media {
            pk: pk.into(),
            id: Some(format!("{pk}_{author}")),
            code: None,
            taken_at,
            media_type: 1,
            product_type: None,
            caption: Some(Caption { text: "hi".into() }),
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

    #[test]
    fn media_filters_other_authors() {
        let m = media("1", "other", 1_700_000_000);
        assert!(!should_delete_media(&m, "me", None, None));
    }

    #[test]
    fn media_fails_closed_when_owner_is_missing() {
        let m = media("1", "", 1_700_000_000);
        assert!(!should_delete_media(&m, "me", None, None));
        assert!(!should_delete_media(&m, "", None, None));
    }

    #[test]
    fn media_allows_self() {
        let m = media("1", "me", 1_700_000_000);
        assert!(should_delete_media(&m, "me", None, None));
    }

    #[test]
    fn media_before_after_window() {
        let m = media("1", "me", 1_700_000_000);
        let before =
            dates::parse_to_datetime("2024-01-01", crate::cleaner::dates::DateBound::After)
                .unwrap();
        let after = dates::parse_to_datetime("2023-01-01", crate::cleaner::dates::DateBound::After)
            .unwrap();
        assert!(should_delete_media(&m, "me", Some(before), Some(after)));
        let too_old_after =
            dates::parse_to_datetime("2023-12-01", crate::cleaner::dates::DateBound::After)
                .unwrap();
        assert!(!should_delete_media(&m, "me", None, Some(too_old_after)));
        let too_new_before =
            dates::parse_to_datetime("2023-01-01", crate::cleaner::dates::DateBound::After)
                .unwrap();
        assert!(!should_delete_media(&m, "me", Some(too_new_before), None));
    }

    #[test]
    fn dm_requires_self_author() {
        let m = dm("i1", "other", 1_700_000_000_000_000);
        assert!(!should_delete_dm(&m, "me", None, None));
        let mut mine = dm("i2", "me", 1_700_000_000_000_000);
        mine.item_type = Some("media".into());
        mine.extra.insert("visual_media".into(), serde_json::json!({
            "media": { "media_type": 1, "image_versions2": { "candidates": [{ "url": "https://example.invalid/photo.jpg" }] } }
        }));
        assert!(should_delete_dm(&mine, "me", None, None));
        mine.user_id = Some("other".into());
        assert!(!should_delete_dm(&mine, "me", None, None));
    }

    #[test]
    fn media_filter_accepts_photos_videos_and_carousels_but_rejects_reels_and_shares() {
        let mut m = dm("i1", "me", 1_700_000_000_000_000);
        m.item_type = Some("media".into());
        m.extra.insert("visual_media".into(), serde_json::json!({
            "media": { "media_type": 1, "image_versions2": { "candidates": [{ "url": "photo.jpg" }] } }
        }));
        assert!(m.is_media_attachment());

        m.extra.insert("visual_media".into(), serde_json::json!({
            "media": { "media_type": 2, "image_versions2": { "candidates": [{ "url": "video-thumb.jpg" }] } }
        }));
        assert!(m.is_media_attachment()); // The item type itself identifies this as media.
        m.extra.insert(
            "visual_media".into(),
            serde_json::json!({ "media": { "new_attachment_shape": { "id": "unknown" } } }),
        );
        assert!(m.is_media_attachment());

        m.extra.insert(
            "visual_media".into(),
            serde_json::json!({
                "media": { "media_type": 2, "video_versions": [{ "url": "video.mp4" }] }
            }),
        );
        assert!(m.is_media_attachment());

        m.extra.insert(
            "visual_media".into(),
            serde_json::json!({
                "media": { "media_type": 8, "carousel_media": [{ "media_type": 1 }] }
            }),
        );
        assert!(m.is_media_attachment());

        m.extra.insert("visual_media".into(), serde_json::json!({
            "media": { "media_type": 2, "product_type": "clips", "video_versions": [{ "url": "reel.mp4" }] }
        }));
        assert!(!m.is_media_attachment());

        m.item_type = Some("media_share".into());
        m.extra.insert("media_share".into(), serde_json::json!({
            "media": { "media_type": 1, "image_versions2": { "candidates": [{ "url": "shared.jpg" }] } }
        }));
        assert!(!m.is_media_attachment());
    }

    #[test]
    fn dm_fails_closed_without_authenticated_user_id() {
        let message = dm("i1", "", 1_700_000_000_000_000);
        assert!(!should_delete_dm(&message, "", None, None));
    }

    #[test]
    fn comments_fail_closed_without_authenticated_user_id() {
        let comment = crate::model::Comment {
            pk: "comment-1".into(),
            text: "text".into(),
            created_at: 1_700_000_000,
            user: None,
            parent_comment_id: None,
        };
        assert!(!should_delete_comment(&comment, "", "media-1", None, None));
    }

    #[test]
    fn action_log_not_planned() {
        let mut m = DirectMessage {
            item_id: "1".into(),
            id: None,
            user_id: Some("me".into()),
            timestamp: 1_700_000_000_000_000,
            item_type: Some("action_log".into()),
            text: None,
            client_context: None,
            extra: Default::default(),
        };
        assert!(!should_delete_dm(&m, "me", None, None));
        m.item_type = Some("text".into());
        m.text = Some("hi".into());
        assert!(should_delete_dm(&m, "me", None, None));

        m.item_type = None;
        assert!(should_delete_dm(&m, "me", None, None));
    }

    #[test]
    fn unparseable_timestamp_fails_closed() {
        let m = media("1", "me", 0);
        assert!(!should_delete_media(&m, "me", None, None));
    }

    #[test]
    fn thread_kind_filter() {
        let mut t = DirectThread {
            thread_id: "1".into(),
            pk: None,
            thread_title: None,
            is_group: false,
            users: vec![],
            items: vec![],
            has_older: false,
            prev_cursor: None,
            oldest_cursor: None,
        };
        assert!(thread_matches_kind(&t, DmKind::OneOnOne));
        assert!(!thread_matches_kind(&t, DmKind::Group));
        t.is_group = true;
        assert!(thread_matches_kind(&t, DmKind::Group));
        assert!(!thread_matches_kind(&t, DmKind::OneOnOne));
    }
}
