use std::collections::HashSet;

use chrono::{DateTime, Utc};

use crate::config::{ContentFilter, DmKind};
use crate::model::{Channel, Message};

use super::dates;

pub(crate) fn first_protected_channel(
    channel: &Channel,
    protect: &HashSet<&str>,
) -> Option<String> {
    let cid = channel.id();
    for p in protect {
        let p = p.trim_start_matches(['@', '#']);
        if p == cid.as_str() {
            return Some(channel.display_name());
        }
        for user in &channel.recipients {
            let uname = user.username.trim_start_matches('@');
            if p == user.id.as_str() || p.eq_ignore_ascii_case(uname) {
                return Some(if user.username.is_empty() {
                    user.id.clone()
                } else {
                    user.username.clone()
                });
            }
        }
        if let Some(name) = &channel.name {
            if p.eq_ignore_ascii_case(name.trim().trim_start_matches('#')) {
                return Some(channel.display_name());
            }
        }
    }
    None
}

pub(crate) fn channel_matches_kind(channel: &Channel, kind: DmKind) -> bool {
    if channel.is_guild_text_like() {
        return true;
    }
    if !channel.is_dm_like() {
        return false;
    }
    kind.matches(channel.is_group_dm())
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

pub(crate) fn should_delete_message(
    msg: &Message,
    self_user_id: &str,
    before: Option<DateTime<Utc>>,
    after: Option<DateTime<Utc>>,
    content_filter: ContentFilter,
    keep_pinned: bool,
) -> bool {
    if self_user_id.is_empty() || msg.author_id() != self_user_id {
        return false;
    }
    if !msg.is_deletable_by_pruner() {
        return false;
    }
    if msg.id.trim().is_empty() {
        return false;
    }
    if keep_pinned && msg.pinned {
        return false;
    }
    if !content_filter.matches(msg) {
        return false;
    }
    in_date_window(dates::message_time(msg), before, after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Channel, Message, MessageAuthor, User};

    fn channel(id: &str, kind: i32, users: &[&str]) -> Channel {
        Channel {
            id: id.into(),
            kind,
            channel_type: kind,
            name: None,
            guild_id: None,
            recipients: users
                .iter()
                .map(|u| User {
                    id: (*u).into(),
                    username: format!("u{u}"),
                })
                .collect(),
        }
    }

    fn msg(id: &str, user: &str) -> Message {
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

    #[test]
    fn message_requires_self_author() {
        let m = msg("1", "other");
        assert!(!should_delete_message(
            &m,
            "me",
            None,
            None,
            ContentFilter::All,
            false
        ));
        let mine = msg("2", "me");
        assert!(should_delete_message(
            &mine,
            "me",
            None,
            None,
            ContentFilter::All,
            false
        ));
    }

    #[test]
    fn message_fails_closed_without_authenticated_user_id() {
        let message = msg("1", "");
        assert!(!should_delete_message(
            &message,
            "",
            None,
            None,
            ContentFilter::All,
            false
        ));
    }

    #[test]
    fn system_messages_not_planned() {
        let mut m = msg("1", "me");
        assert!(should_delete_message(
            &m,
            "me",
            None,
            None,
            ContentFilter::All,
            false
        ));
        for event_type in [1, 2, 3, 21] {
            m.message_type = event_type;
            assert!(
                !should_delete_message(&m, "me", None, None, ContentFilter::All, false),
                "message type {event_type} must be skipped"
            );
        }
        for event_type in [
            0, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 17, 18, 19, 20, 22, 23, 24, 25, 26, 27,
            28, 29, 31, 32, 36, 37, 38, 39, 44, 46,
        ] {
            m.message_type = event_type;
            assert!(
                should_delete_message(&m, "me", None, None, ContentFilter::All, false),
                "message type {event_type} should be included"
            );
        }
    }

    #[test]
    fn unparseable_timestamp_fails_closed() {
        let mut m = msg("notanid", "me");
        m.timestamp = String::new();
        assert!(!should_delete_message(
            &m,
            "me",
            None,
            None,
            ContentFilter::All,
            false
        ));
    }

    fn msg_with(id: &str, content: &str, attachments: usize, embeds: usize) -> Message {
        Message {
            attachments: (0..attachments)
                .map(|i| crate::model::Attachment {
                    id: format!("a{i}"),
                    filename: format!("f{i}.png"),
                    url: format!("https://cdn/{i}"),
                })
                .collect(),
            embeds: (0..embeds)
                .map(|_| crate::model::Embed::default())
                .collect(),
            content: content.into(),
            ..msg(id, "me")
        }
    }

    #[test]
    fn content_filter_matches_message_shapes() {
        let plain = msg_with("1", "hello", 0, 0);
        let only_att = msg_with("2", "  ", 2, 0);
        let att_and_text = msg_with("3", "look", 1, 0);
        let linked = msg_with("4", "see https://x.com", 0, 0);
        let embedded = msg_with("5", "", 0, 1);
        let empty = msg_with("6", "", 0, 0);

        for m in [&plain, &only_att, &att_and_text, &linked, &embedded, &empty] {
            assert!(ContentFilter::All.matches(m), "all matches {m:?}");
        }
        assert!(!ContentFilter::OnlyAttachments.matches(&plain));
        assert!(ContentFilter::OnlyAttachments.matches(&only_att));
        assert!(!ContentFilter::OnlyAttachments.matches(&att_and_text));

        assert!(!ContentFilter::HasAttachments.matches(&plain));
        assert!(ContentFilter::HasAttachments.matches(&only_att));
        assert!(ContentFilter::HasAttachments.matches(&att_and_text));

        assert!(!ContentFilter::HasLinks.matches(&plain));
        assert!(ContentFilter::HasLinks.matches(&linked));
        assert!(ContentFilter::HasLinks.matches(&embedded));
        assert!(!ContentFilter::HasLinks.matches(&only_att));

        assert!(ContentFilter::TextOnly.matches(&plain));
        assert!(!ContentFilter::TextOnly.matches(&att_and_text));
        assert!(!ContentFilter::TextOnly.matches(&embedded));
        assert!(!ContentFilter::TextOnly.matches(&empty));
    }

    #[test]
    fn content_filter_is_applied_by_should_delete() {
        let only_att = msg_with("1", "", 1, 0);
        let plain = msg_with("2", "text", 0, 0);
        assert!(should_delete_message(
            &only_att,
            "me",
            None,
            None,
            ContentFilter::OnlyAttachments,
            false
        ));
        assert!(!should_delete_message(
            &plain,
            "me",
            None,
            None,
            ContentFilter::OnlyAttachments,
            false
        ));
    }

    #[test]
    fn keep_pinned_skips_pinned_messages() {
        let mut m = msg("1", "me");
        m.pinned = true;
        assert!(!should_delete_message(
            &m,
            "me",
            None,
            None,
            ContentFilter::All,
            true
        ));
        assert!(should_delete_message(
            &m,
            "me",
            None,
            None,
            ContentFilter::All,
            false
        ));
    }

    #[test]
    fn content_filter_cycles_through_all_variants() {
        let sequence: Vec<ContentFilter> = {
            let mut v = vec![ContentFilter::All];
            for _ in 0..4 {
                v.push(v.last().unwrap().next());
            }
            v
        };
        assert_eq!(sequence[1], ContentFilter::OnlyAttachments);
        assert_eq!(sequence[2], ContentFilter::HasAttachments);
        assert_eq!(sequence[3], ContentFilter::HasLinks);
        assert_eq!(sequence[4], ContentFilter::TextOnly);
        assert_eq!(sequence[4].next(), ContentFilter::All);
        assert_eq!(ContentFilter::All.prev(), ContentFilter::TextOnly);
    }

    #[test]
    fn content_filter_parses_env_aliases() {
        for (raw, want) in [
            ("all", ContentFilter::All),
            ("ONLY-attachments", ContentFilter::OnlyAttachments),
            ("links", ContentFilter::HasLinks),
            ("text_only", ContentFilter::TextOnly),
            ("attachments", ContentFilter::HasAttachments),
        ] {
            assert_eq!(raw.parse::<ContentFilter>().ok(), Some(want));
        }
        assert_eq!("bogus".parse::<ContentFilter>().ok(), None);
    }

    #[test]
    fn channel_kind_filter() {
        let one = channel("1", 1, &[]);
        assert!(channel_matches_kind(&one, DmKind::OneOnOne));
        assert!(!channel_matches_kind(&one, DmKind::Group));
        let grp = channel("2", 3, &[]);
        assert!(channel_matches_kind(&grp, DmKind::Group));
        assert!(!channel_matches_kind(&grp, DmKind::OneOnOne));
        let guild = channel("3", 0, &[]);
        assert!(channel_matches_kind(&guild, DmKind::Group));
        assert!(channel_matches_kind(&guild, DmKind::OneOnOne));
    }
}
