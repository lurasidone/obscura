use std::collections::HashMap;

use crate::cleaner::{dates, CleanerConfig, TargetResolution};
use crate::config::{thread_id_list, Config, DmKind, Selection};

use super::super::action::Field;

#[derive(Debug, Clone)]
pub struct Form {
    pub sessionid: String,
    pub content: Selection,
    pub threads: String,
    pub before: String,
    pub after: String,
    pub limit: String,
    pub throttle_ms: String,
    pub scan_throttle_ms: String,
    pub concurrency: String,
    pub dm_kind: DmKind,
    pub dm_media_only: bool,
    pub dry_run: bool,
    pub hide_threads: bool,
    pub protect_users: String,
    pub history_dir: String,
}

impl Form {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            sessionid: cfg.sessionid.clone(),
            content: cfg.content,
            threads: cfg.threads.clone(),
            before: cfg.before.clone(),
            after: cfg.after.clone(),
            limit: cfg.limit.clone(),
            throttle_ms: cfg.throttle_ms.clone(),
            scan_throttle_ms: cfg.scan_throttle_ms.clone(),
            concurrency: cfg.concurrency.clone(),
            dm_kind: cfg.dm_kind,
            dm_media_only: cfg.dm_media_only,
            dry_run: cfg.dry_run,
            hide_threads: cfg.hide_threads,
            protect_users: cfg.protect_users.join(", "),
            history_dir: cfg.history_dir.clone(),
        }
    }

    pub fn get_text(&self, field: Field) -> String {
        match field {
            Field::SessionId => self.sessionid.clone(),
            Field::Threads => self.threads.clone(),
            Field::Before => self.before.clone(),
            Field::After => self.after.clone(),
            Field::Limit => self.limit.clone(),
            Field::Throttle => self.throttle_ms.clone(),
            Field::ScanThrottle => self.scan_throttle_ms.clone(),
            Field::Concurrency => self.concurrency.clone(),
            Field::Protect => self.protect_users.clone(),
            _ => String::new(),
        }
    }

    pub fn set_text(&mut self, field: Field, value: String) {
        match field {
            Field::SessionId => self.sessionid = value.trim().to_string(),
            Field::Threads => self.threads = value,
            Field::Before => self.before = value.trim().to_string(),
            Field::After => self.after = value.trim().to_string(),
            Field::Limit => self.limit = value.trim().to_string(),
            Field::Throttle => self.throttle_ms = value.trim().to_string(),
            Field::ScanThrottle => self.scan_throttle_ms = value.trim().to_string(),
            Field::Concurrency => self.concurrency = value.trim().to_string(),
            Field::Protect => self.protect_users = value,
            _ => {}
        }
    }

    pub fn cycle_next(&mut self, field: Field) {
        self.cycle(field, true);
    }

    pub fn cycle_prev(&mut self, field: Field) {
        self.cycle(field, false);
    }

    fn cycle(&mut self, field: Field, forward: bool) {
        if field == Field::DmKind {
            self.dm_kind = if forward {
                self.dm_kind.next()
            } else {
                self.dm_kind.prev()
            };
        } else {
            self.toggle(field);
        }
    }

    /// The current value of an on/off field; `None` for any other field.
    pub fn flag(&self, field: Field) -> Option<bool> {
        let activity = self.content.activity;
        Some(match field {
            Field::DryRun => self.dry_run,
            Field::HideThreads => self.hide_threads,
            Field::DmMediaOnly => self.dm_media_only,
            Field::ContentPosts => self.content.posts,
            Field::ContentDms => self.content.dms,
            Field::ContentDmReactions => activity.dm_reactions,
            Field::ContentLikes => activity.likes,
            Field::ContentSaved => activity.saved,
            Field::ContentComments => activity.comments,
            Field::ContentArchived => activity.archived,
            Field::ContentReposts => activity.reposts,
            Field::ContentUnfollow => activity.unfollow,
            _ => return None,
        })
    }

    fn flag_mut(&mut self, field: Field) -> Option<&mut bool> {
        let activity = &mut self.content.activity;
        Some(match field {
            Field::DryRun => &mut self.dry_run,
            Field::HideThreads => &mut self.hide_threads,
            Field::DmMediaOnly => &mut self.dm_media_only,
            Field::ContentPosts => &mut self.content.posts,
            Field::ContentDms => &mut self.content.dms,
            Field::ContentDmReactions => &mut activity.dm_reactions,
            Field::ContentLikes => &mut activity.likes,
            Field::ContentSaved => &mut activity.saved,
            Field::ContentComments => &mut activity.comments,
            Field::ContentArchived => &mut activity.archived,
            Field::ContentReposts => &mut activity.reposts,
            Field::ContentUnfollow => &mut activity.unfollow,
            _ => return None,
        })
    }

    pub fn toggle(&mut self, field: Field) {
        if let Some(flag) = self.flag_mut(field) {
            *flag = !*flag;
        }
    }

    pub fn validate(&self) -> HashMap<Field, String> {
        let mut errs = HashMap::new();
        if self.sessionid.trim().is_empty() {
            errs.insert(
                Field::SessionId,
                "the sessionid cookie is required".to_string(),
            );
        }
        if !self.content.any() {
            errs.insert(
                Field::ContentPosts,
                "select at least one thing to delete".to_string(),
            );
        }
        if !thread_id_list(&self.threads).is_empty() && !self.content.touches_threads() {
            errs.insert(
                Field::Threads,
                "turn on DM messages or DM reactions to use this".to_string(),
            );
        }
        if !self.before.trim().is_empty() && dates::validate_date_input(&self.before).is_err() {
            errs.insert(Field::Before, "expected YYYY-MM-DD or RFC 3339".to_string());
        }
        if !self.after.trim().is_empty() && dates::validate_date_input(&self.after).is_err() {
            errs.insert(Field::After, "expected YYYY-MM-DD or RFC 3339".to_string());
        }
        if self.limit.trim().parse::<u64>().is_err() {
            errs.insert(
                Field::Limit,
                "must be 0 or a positive whole number".to_string(),
            );
        }
        if let Ok(throttle) = self.throttle_ms.trim().parse::<u64>() {
            if throttle < 200 {
                errs.insert(Field::Throttle, "must be at least 200".to_string());
            }
        } else {
            errs.insert(
                Field::Throttle,
                "must be a positive whole number".to_string(),
            );
        }
        if let Ok(scan) = self.scan_throttle_ms.trim().parse::<u64>() {
            if scan < 100 {
                errs.insert(Field::ScanThrottle, "must be at least 100".to_string());
            }
        } else {
            errs.insert(
                Field::ScanThrottle,
                "must be a positive whole number".to_string(),
            );
        }
        match self.concurrency.trim().parse::<u64>() {
            Ok(n) if (1..=u64::from(crate::config::MAX_SCAN_CONCURRENCY)).contains(&n) => {}
            Ok(_) => {
                errs.insert(
                    Field::Concurrency,
                    format!("must be 1 to {}", crate::config::MAX_SCAN_CONCURRENCY),
                );
            }
            Err(_) => {
                errs.insert(
                    Field::Concurrency,
                    "must be a positive whole number".to_string(),
                );
            }
        }
        errs
    }

    pub fn to_cleaner_config(&self) -> Result<CleanerConfig, String> {
        let mut targets = Vec::new();
        if self.content.posts {
            targets.push(TargetResolution::Posts);
        }
        if self.content.touches_threads() {
            let ids = thread_id_list(&self.threads);
            targets.push(if ids.is_empty() {
                TargetResolution::AllDms
            } else {
                TargetResolution::Threads(ids)
            });
        }

        let before = if self.before.trim().is_empty() {
            None
        } else {
            Some(self.before.trim().to_string())
        };
        let after = if self.after.trim().is_empty() {
            None
        } else {
            Some(self.after.trim().to_string())
        };

        let limit = self
            .limit
            .trim()
            .parse::<usize>()
            .map_err(|_| "invalid limit".to_string())?;
        let throttle_ms = self
            .throttle_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| "invalid throttle".to_string())?;
        let scan_throttle_ms = self
            .scan_throttle_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| "invalid scan throttle".to_string())?;
        let concurrency = crate::config::clamp_scan_concurrency(
            self.concurrency
                .trim()
                .parse::<u64>()
                .map_err(|_| "invalid scan concurrency".to_string())?,
        );
        let protect_users = thread_id_list(&self.protect_users);

        Ok(CleanerConfig {
            targets,
            content: self.content.activity,
            before,
            after,
            limit,
            dry_run: self.dry_run,
            throttle_ms,
            scan_throttle_ms,
            concurrency,
            hide_threads: self.hide_threads,
            protect_users,
            dm_kind: self.dm_kind,
            dm_media_only: self.dm_media_only,
            unsend_dms: self.content.dms,
            history_dir: self.history_dir.clone(),
        })
    }
}
