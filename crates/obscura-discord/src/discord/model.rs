use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct User {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub username: String,
}

/// Discord channel types we care about:
/// 0 = guild text, 1 = DM, 3 = group DM, 5 = announcement, 10-12 = threads,
// 13 = stage, 15 = forum, etc. Anything else is treated as non-text.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Channel {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub kind: i32,
    #[serde(default, alias = "type")]
    pub channel_type: i32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
    #[serde(default)]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub thread_metadata: Option<ThreadMetadata>,
    #[serde(default)]
    pub recipients: Vec<User>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ThreadMetadata {
    #[serde(default)]
    pub archive_timestamp: Option<String>,
}

impl Channel {
    pub fn channel_type(&self) -> i32 {
        if self.channel_type != 0 || self.kind == 0 {
            self.channel_type
        } else {
            self.kind
        }
    }

    pub fn is_dm(&self) -> bool {
        self.channel_type() == 1
    }

    pub fn is_group_dm(&self) -> bool {
        self.channel_type() == 3
    }

    pub fn is_dm_like(&self) -> bool {
        matches!(self.channel_type(), 1 | 3)
    }

    pub fn is_guild_text_like(&self) -> bool {
        matches!(self.channel_type(), 0 | 5 | 10 | 11 | 12)
    }

    pub fn is_thread_parent(&self) -> bool {
        matches!(self.channel_type(), 0 | 5 | 15 | 16)
    }

    pub fn id(&self) -> String {
        self.id.clone()
    }

    pub fn display_name(&self) -> String {
        if self.is_dm_like() {
            return self.participant_label();
        }
        if let Some(name) = &self.name {
            let t = name.trim();
            if !t.is_empty() {
                return format!("#{t}");
            }
        }
        format!("channel:{}", self.id)
    }

    pub fn participant_label(&self) -> String {
        if self.recipients.is_empty() {
            if let Some(name) = &self.name {
                if !name.trim().is_empty() {
                    return name.clone();
                }
            }
            return format!("channel:{}", self.id);
        }
        let labels: Vec<String> = self
            .recipients
            .iter()
            .map(|u| {
                if !u.username.is_empty() {
                    format!("@{}", u.username)
                } else {
                    u.id.clone()
                }
            })
            .filter(|s| !s.is_empty())
            .collect();
        match labels.len() {
            0 => format!("channel:{}", self.id),
            1 => labels[0].clone(),
            2 => format!("{}, {}", labels[0], labels[1]),
            n => format!("{}, {} +{}", labels[0], labels[1], n - 2),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct MessageAuthor {
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct Attachment {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub filename: String,
    #[serde(default)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize, Default, PartialEq, Eq)]
pub struct Embed {
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default, alias = "type")]
    pub kind: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Message {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub channel_id: String,
    #[serde(default)]
    pub author: Option<MessageAuthor>,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub timestamp: String,
    #[serde(default, alias = "type")]
    pub message_type: i32,
    #[serde(default)]
    pub attachments: Vec<Attachment>,
    #[serde(default)]
    pub embeds: Vec<Embed>,
    #[serde(default)]
    pub pinned: bool,
}

impl Message {
    pub fn message_id(&self) -> String {
        self.id.clone()
    }

    pub fn author_id(&self) -> String {
        self.author
            .as_ref()
            .map(|a| a.id.clone())
            .unwrap_or_default()
    }

    pub fn has_attachments(&self) -> bool {
        !self.attachments.is_empty()
    }

    pub fn has_links(&self) -> bool {
        if !self.embeds.is_empty() {
            return true;
        }
        let lower = self.content.to_ascii_lowercase();
        lower.contains("http://") || lower.contains("https://")
    }

    pub fn is_only_attachments(&self) -> bool {
        self.has_attachments() && self.content.trim().is_empty()
    }

    pub fn is_text_only(&self) -> bool {
        !self.content.trim().is_empty() && self.attachments.is_empty() && self.embeds.is_empty()
    }

    /// Discord marks group membership, call-start, and thread-starter messages
    /// as non-deletable.
    pub fn is_system(&self) -> bool {
        matches!(self.message_type, 1 | 2 | 3 | 21)
    }

    pub fn is_deletable_by_pruner(&self) -> bool {
        !self.is_system()
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Guild {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ThreadListResponse {
    #[serde(default)]
    pub threads: Vec<Channel>,
    #[serde(default)]
    pub has_more: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dm_label_uses_recipients() {
        let ch: Channel = serde_json::from_value(serde_json::json!({
            "id": "1", "type": 1,
            "recipients": [{"id": "2", "username": "alice"}]
        }))
        .unwrap();
        assert!(ch.is_dm());
        assert_eq!(ch.display_name(), "@alice");
    }

    #[test]
    fn system_messages_detected() {
        let m = Message {
            message_type: 1,
            ..Default::default()
        };
        assert!(m.is_system());
        let m2 = Message {
            message_type: 0,
            ..Default::default()
        };
        assert!(!m2.is_system());
    }
}
