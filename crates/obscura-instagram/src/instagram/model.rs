use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Deserialize)]
pub struct User {
    #[serde(default, deserialize_with = "de_id_string")]
    pub pk: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub full_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Media {
    #[serde(default, deserialize_with = "de_id_string")]
    pub pk: String,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub id: Option<String>,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub taken_at: i64,
    #[serde(default)]
    pub media_type: i32,
    #[serde(default)]
    pub product_type: Option<String>,
    #[serde(default)]
    pub caption: Option<Caption>,
    #[serde(default)]
    pub user: Option<UserShort>,
    #[serde(default)]
    pub image_versions2: Option<ImageVersions>,
    #[serde(default)]
    pub thumbnail_url: Option<String>,
}

impl Media {
    pub fn media_id(&self) -> String {
        if let Some(id) = &self.id {
            if !id.is_empty() {
                if id.contains('_') {
                    return id.clone();
                }
                if let Some(user) = self.user.as_ref().filter(|user| !user.pk.is_empty()) {
                    return format!("{}_{}", id, user.pk);
                }
                return id.clone();
            }
        }
        if let Some(user) = &self.user {
            if !user.pk.is_empty() && !self.pk.is_empty() {
                return format!("{}_{}", self.pk, user.pk);
            }
        }
        self.pk.clone()
    }

    pub fn caption_text(&self) -> String {
        self.caption
            .as_ref()
            .map(|c| c.text.clone())
            .unwrap_or_default()
    }

    pub fn author_username(&self) -> String {
        self.user
            .as_ref()
            .map(|u| u.username.clone())
            .unwrap_or_default()
    }

    pub fn author_id(&self) -> String {
        self.user.as_ref().map(|u| u.pk.clone()).unwrap_or_default()
    }

    pub fn preview_url(&self) -> Option<String> {
        if let Some(url) = &self.thumbnail_url {
            if !url.is_empty() {
                return Some(url.clone());
            }
        }
        self.image_versions2
            .as_ref()
            .and_then(|iv| iv.candidates.first())
            .map(|c| c.url.clone())
    }

    pub fn kind_label(&self) -> &'static str {
        match (self.media_type, self.product_type.as_deref()) {
            (1, _) => "photo",
            (2, Some("clips")) => "reel",
            (2, Some("igtv")) => "igtv",
            (2, _) => "video",
            (8, _) => "album",
            _ => "media",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Caption {
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct UserShort {
    #[serde(default, deserialize_with = "de_id_string")]
    pub pk: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub full_name: String,
}

/// One page of `friendships/{id}/followers/` or `.../following/`. `users` is
/// required: a body without it must fail rather than read as an empty list.
#[derive(Debug, Clone, Deserialize)]
pub struct FollowListResponse {
    pub users: Vec<UserShort>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub next_max_id: Option<String>,
}

/// `friendships/show/{id}/`; also nested in the `friendships/destroy/{id}/` reply.
#[derive(Debug, Clone, Copy, Deserialize, Default)]
pub struct Friendship {
    #[serde(default)]
    pub following: Option<bool>,
    #[serde(default)]
    pub followed_by: Option<bool>,
}

/// What `friendships/show/{id}/` says about an account you may unfollow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowStatus {
    OneSided,
    FollowsBack,
    NotFollowing,
    /// Instagram left out a fact the decision needs.
    Unknown,
}

impl Friendship {
    pub fn status(self) -> FollowStatus {
        match (self.following, self.followed_by) {
            // checked first: someone who follows you is safe whatever else is missing
            (_, Some(true)) => FollowStatus::FollowsBack,
            (Some(true), Some(false)) => FollowStatus::OneSided,
            (Some(false), Some(false)) => FollowStatus::NotFollowing,
            _ => FollowStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct FriendshipDestroyResponse {
    #[serde(default)]
    pub friendship_status: Option<Friendship>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ImageVersions {
    #[serde(default)]
    pub candidates: Vec<ImageCandidate>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ImageCandidate {
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FeedResponse {
    #[serde(default)]
    pub items: Vec<Media>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub next_max_id: Option<String>,
    #[serde(default)]
    pub more_available: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirectThread {
    #[serde(default, deserialize_with = "de_id_string")]
    pub thread_id: String,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub pk: Option<String>,
    #[serde(default)]
    pub thread_title: Option<String>,
    #[serde(default)]
    pub is_group: bool,
    #[serde(default)]
    pub users: Vec<UserShort>,
    #[serde(default)]
    pub items: Vec<DirectMessage>,
    #[serde(default)]
    pub has_older: bool,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub prev_cursor: Option<String>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub oldest_cursor: Option<String>,
}

impl DirectThread {
    pub fn id(&self) -> String {
        if !self.thread_id.is_empty() {
            return self.thread_id.clone();
        }
        self.pk.clone().unwrap_or_default()
    }

    pub fn display_name(&self) -> String {
        if let Some(title) = &self.thread_title {
            let t = title.trim();
            if !t.is_empty() {
                return t.to_string();
            }
        }
        self.participant_label()
    }

    pub fn participant_label(&self) -> String {
        if self.users.is_empty() {
            return format!("thread:{}", self.id());
        }
        if !self.is_group {
            let u = &self.users[0];
            if !u.username.is_empty() {
                return format!("@{}", u.username);
            }
            if !u.full_name.is_empty() {
                return u.full_name.clone();
            }
            return format!("user:{}", u.pk);
        }

        let labels: Vec<String> = self
            .users
            .iter()
            .map(|u| {
                if !u.username.is_empty() {
                    format!("@{}", u.username)
                } else if !u.full_name.is_empty() {
                    u.full_name.clone()
                } else {
                    u.pk.clone()
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        match labels.len() {
            0 => format!("group:{}", self.id()),
            1 => labels[0].clone(),
            2 => format!("{}, {}", labels[0], labels[1]),
            n => format!("{}, {} +{}", labels[0], labels[1], n - 2),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct DirectMessage {
    #[serde(default, deserialize_with = "de_id_string")]
    pub item_id: String,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub user_id: Option<String>,
    #[serde(default, deserialize_with = "de_timestamp_us")]
    pub timestamp: i64,
    #[serde(default)]
    pub item_type: Option<String>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub client_context: Option<String>,
    /// Unmodeled attachment payloads returned by Instagram's private API.
    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// The double-tap heart, which Instagram may list under `reactions.likes` with no emoji.
const DEFAULT_REACTION_EMOJI: &str = "❤️";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnReaction {
    pub emoji: String,
    /// When the reaction itself was made, in microseconds, if Instagram sent it.
    pub timestamp_us: Option<i64>,
}

// Instagram sends ids and timestamps as either JSON strings or integers.
fn json_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn reaction_from<'a>(reactions: &'a Value, list: &str, self_id: &str) -> Option<&'a Value> {
    reactions.get(list)?.as_array()?.iter().find(|reaction| {
        reaction.get("sender_id").is_some_and(|sender| {
            sender.as_str() == Some(self_id)
                || sender.as_i64().map(|id| id.to_string()).as_deref() == Some(self_id)
        })
    })
}

impl DirectMessage {
    pub fn own_reaction(&self, self_id: &str) -> Option<OwnReaction> {
        if self_id.is_empty() {
            return None;
        }
        let reactions = self.extra.get("reactions")?;
        let reaction = reaction_from(reactions, "emojis", self_id)
            .or_else(|| reaction_from(reactions, "likes", self_id))?;
        let emoji = reaction
            .get("emoji")
            .and_then(Value::as_str)
            .filter(|emoji| !emoji.is_empty())
            .unwrap_or(DEFAULT_REACTION_EMOJI)
            .to_string();
        let timestamp_us = reaction
            .get("timestamp")
            .and_then(json_i64)
            .filter(|ts| *ts > 0);
        Some(OwnReaction {
            emoji,
            timestamp_us,
        })
    }

    /// True for everything the media-only filter deletes: every message type
    /// except text, Reels and shared posts. Unsends can't be undone, so a missing
    /// type or an unrecognized share (`xma*`), which may be a post, counts as kept.
    pub fn is_media_attachment(&self) -> bool {
        let item_type = self.item_type.as_deref().map_or("", str::trim);
        match item_type {
            "" | "text" | "link" | "like" | "placeholder" => false,
            "reel_share" | "clip" | "media_share" | "felix_share" => false,
            "xma_story_share" | "xma_profile" => true,
            other if other.starts_with("xma") || other.ends_with("_xma") => false,
            "media" | "visual_media" => !self.attachment_is_reel(),
            _ => true,
        }
    }

    fn attachment_is_reel(&self) -> bool {
        ["visual_media", "media"]
            .iter()
            .filter_map(|key| self.extra.get(*key))
            .any(|v| {
                let media = v.get("media").unwrap_or(v);
                let product_type = media.get("product_type").and_then(Value::as_str);
                matches!(product_type, Some("clips" | "reel"))
                    || media.get("clips_metadata").is_some_and(|v| !v.is_null())
                    || media.get("is_clips").and_then(Value::as_bool) == Some(true)
            })
    }
}

impl DirectMessage {
    pub fn message_id(&self) -> String {
        if !self.item_id.is_empty() {
            return self.item_id.clone();
        }
        self.id.clone().unwrap_or_default()
    }

    pub fn timestamp_rfc3339(&self) -> String {
        if self.timestamp <= 0 {
            return String::new();
        }

        let secs = self.timestamp / 1_000_000;
        let nsecs = ((self.timestamp % 1_000_000) * 1000) as u32;
        chrono::DateTime::<chrono::Utc>::from_timestamp(secs, nsecs)
            .map(|dt| dt.to_rfc3339())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct InboxResponse {
    #[serde(default)]
    pub inbox: InboxBody,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct InboxBody {
    #[serde(default)]
    pub threads: Vec<DirectThread>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub oldest_cursor: Option<String>,
    #[serde(default)]
    pub has_older: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThreadResponse {
    #[serde(default)]
    pub thread: Option<DirectThread>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct UserInfoResponse {
    #[serde(default)]
    pub user: Option<User>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CurrentUserResponse {
    #[serde(default)]
    pub user: Option<User>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeleteMediaResponse {
    #[serde(default)]
    pub did_delete: Option<bool>,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StatusResponse {
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MediaItem {
    Wrapped { media: Media },
    Plain(Media),
}

impl MediaItem {
    pub fn into_media(self) -> Media {
        match self {
            Self::Wrapped { media } => media,
            Self::Plain(media) => media,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActivityFeedResponse {
    #[serde(default)]
    pub items: Vec<MediaItem>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub next_max_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub max_id: Option<String>,
}

impl ActivityFeedResponse {
    pub fn next_cursor(&self) -> Option<String> {
        self.next_max_id
            .clone()
            .or_else(|| self.max_id.clone())
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Collection {
    #[serde(default, deserialize_with = "de_id_string")]
    pub collection_id: String,
    #[serde(default)]
    pub collection_name: String,
    #[serde(default)]
    pub collection_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CollectionsResponse {
    #[serde(default)]
    pub items: Vec<Collection>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub next_max_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub max_id: Option<String>,
    #[serde(default)]
    pub more_available: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Comment {
    #[serde(default, deserialize_with = "de_id_string")]
    pub pk: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub user: Option<UserShort>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub parent_comment_id: Option<String>,
}

impl Comment {
    pub fn author_id(&self) -> String {
        self.user.as_ref().map(|u| u.pk.clone()).unwrap_or_default()
    }

    pub fn author_username(&self) -> String {
        self.user
            .as_ref()
            .map(|u| u.username.clone())
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommentsResponse {
    #[serde(default)]
    pub comments: Vec<Comment>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub next_max_id: Option<String>,
    #[serde(default, deserialize_with = "de_opt_id_string")]
    pub next_min_id: Option<String>,
    #[serde(default)]
    pub has_more_comments: bool,
    #[serde(default)]
    pub has_more_headload_comments: bool,
}

// ids come back as either a JSON number (occasionally 128-bit) or a string; keep the raw digits
fn de_id_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(de_opt_id_string(deserializer)?.unwrap_or_default())
}

fn de_opt_id_string<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde_json::Value;
    match Value::deserialize(deserializer)? {
        Value::Null | Value::Bool(_) => Ok(None),
        Value::String(s) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                Ok(Some(t.to_string()))
            }
        }
        Value::Number(n) => Ok(Some(n.to_string())),
        _ => Ok(None),
    }
}

fn de_timestamp_us<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{self, Visitor};
    use std::fmt;

    struct TsVisitor;
    impl<'de> Visitor<'de> for TsVisitor {
        type Value = i64;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("timestamp as int or string")
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<i64, E> {
            Ok(v)
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<i64, E> {
            Ok(v as i64)
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> Result<i64, E> {
            Ok(v as i64)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<i64, E> {
            let s = v.trim();
            Ok(s.parse::<i64>()
                .ok()
                .or_else(|| s.parse::<f64>().ok().map(|f| f as i64))
                .unwrap_or(0))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<i64, E> {
            self.visit_str(&v)
        }

        fn visit_unit<E: de::Error>(self) -> Result<i64, E> {
            Ok(0)
        }

        fn visit_bool<E: de::Error>(self, _v: bool) -> Result<i64, E> {
            Ok(0)
        }
    }

    deserializer.deserialize_any(TsVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct TestMsg {
        #[serde(default, deserialize_with = "de_id_string")]
        id: String,
        #[serde(default, deserialize_with = "de_timestamp_us")]
        ts: i64,
    }

    #[test]
    fn deserializes_128bit_id_and_various_timestamps() {
        let json = r#"{"id": 340282366841710301281152737272051356071, "ts": "1710000000.5"}"#;
        let msg: TestMsg = serde_json::from_str(json).unwrap();
        assert_eq!(msg.id, "340282366841710301281152737272051356071");
        assert_eq!(msg.ts, 1710000000);

        let json_empty = r#"{"id": null, "ts": ""}"#;
        let msg_empty: TestMsg = serde_json::from_str(json_empty).unwrap();
        assert_eq!(msg_empty.id, "");
        assert_eq!(msg_empty.ts, 0);

        let json_bool = r#"{"id": false, "ts": false}"#;
        let msg_bool: TestMsg = serde_json::from_str(json_bool).unwrap();
        assert_eq!(msg_bool.id, "");
        assert_eq!(msg_bool.ts, 0);

        let json_null = r#"{"id": 1, "ts": null}"#;
        let msg_null: TestMsg = serde_json::from_str(json_null).unwrap();
        assert_eq!(msg_null.ts, 0);

        let json_int = r#"{"id": 1, "ts": 1710000000000000}"#;
        let msg_int: TestMsg = serde_json::from_str(json_int).unwrap();
        assert_eq!(msg_int.ts, 1710000000000000);
    }
}
