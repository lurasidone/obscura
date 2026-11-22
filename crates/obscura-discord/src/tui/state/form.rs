use std::collections::HashMap;

use crate::config::{channel_id_list, Config, ContentFilter, DmKind, ScopeForm, ScopeKind};
use crate::pruner::{dates, PrunerConfig, TargetResolution};

use super::super::action::Field;

#[derive(Debug, Clone)]
pub struct Form {
    pub token: String,
    pub scope: ScopeForm,
    pub before: String,
    pub after: String,
    pub limit: String,
    pub throttle_ms: String,
    pub scan_throttle_ms: String,
    pub concurrency: String,
    pub dm_kind: DmKind,
    pub content_filter: ContentFilter,
    pub keep_pinned: bool,
    pub dry_run: bool,
    pub close_dms: bool,
    pub protect_users: String,
}

impl Form {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            token: cfg.token.clone(),
            scope: cfg.scope_form.clone(),
            before: cfg.before.clone(),
            after: cfg.after.clone(),
            limit: cfg.limit.clone(),
            throttle_ms: cfg.throttle_ms.clone(),
            scan_throttle_ms: cfg.scan_throttle_ms.clone(),
            concurrency: cfg.concurrency.clone(),
            dm_kind: cfg.dm_kind,
            content_filter: cfg.content_filter,
            keep_pinned: cfg.keep_pinned,
            dry_run: cfg.dry_run,
            close_dms: cfg.close_dms,
            protect_users: cfg.protect_ids.join(", "),
        }
    }

    pub fn get_text(&self, field: Field) -> String {
        match field {
            Field::Token => self.token.clone(),
            Field::Payload => self.scope.channels.clone(),
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
            Field::Token => self.token = value.trim().to_string(),
            Field::Payload => self.scope.channels = value,
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

    pub fn toggle(&mut self, field: Field) {
        self.toggle_flag(field);
    }
    pub fn cycle_next(&mut self, field: Field) {
        self.cycle(field, true);
    }
    pub fn cycle_prev(&mut self, field: Field) {
        self.cycle(field, false);
    }

    fn cycle(&mut self, field: Field, forward: bool) {
        match field {
            Field::Scope => {
                if forward {
                    self.scope.next()
                } else {
                    self.scope.prev()
                }
            }
            Field::DmKind => {
                self.dm_kind = if forward {
                    self.dm_kind.next()
                } else {
                    self.dm_kind.prev()
                }
            }
            Field::ContentFilter => {
                self.content_filter = if forward {
                    self.content_filter.next()
                } else {
                    self.content_filter.prev()
                }
            }
            _ => self.toggle_flag(field),
        }
    }

    fn toggle_flag(&mut self, field: Field) {
        match field {
            Field::KeepPinned => self.keep_pinned = !self.keep_pinned,
            Field::DryRun => self.dry_run = !self.dry_run,
            Field::CloseDms => self.close_dms = !self.close_dms,
            _ => {}
        }
    }

    pub fn validate(&self) -> HashMap<Field, String> {
        let mut errs = HashMap::new();
        if self.token.trim().is_empty() {
            errs.insert(Field::Token, "Discord user token is required".to_string());
        }
        if self.scope.kind == ScopeKind::Channels
            && channel_id_list(&self.scope.channels).is_empty()
        {
            errs.insert(
                Field::Payload,
                "at least one channel ID is required".to_string(),
            );
        }
        for (field, value) in [(Field::Before, &self.before), (Field::After, &self.after)] {
            if !value.trim().is_empty() && dates::validate_date_input(value).is_err() {
                errs.insert(
                    field,
                    "invalid date: expected YYYY-MM-DD or RFC3339".to_string(),
                );
            }
        }
        if self.limit.trim().parse::<u64>().is_err() {
            errs.insert(Field::Limit, "must be a non-negative integer".to_string());
        }
        if let Ok(value) = self.throttle_ms.trim().parse::<u64>() {
            if value < 200 {
                errs.insert(Field::Throttle, "must be at least 200ms".to_string());
            }
        } else {
            errs.insert(Field::Throttle, "must be a positive integer".to_string());
        }
        if let Ok(value) = self.scan_throttle_ms.trim().parse::<u64>() {
            if value < 100 {
                errs.insert(Field::ScanThrottle, "must be at least 100ms".to_string());
            }
        } else {
            errs.insert(
                Field::ScanThrottle,
                "must be a positive integer".to_string(),
            );
        }
        match self.concurrency.trim().parse::<u64>() {
            Ok(n) if (1..=u64::from(crate::config::MAX_SCAN_CONCURRENCY)).contains(&n) => {}
            Ok(_) => {
                errs.insert(
                    Field::Concurrency,
                    format!("must be 1-{}", crate::config::MAX_SCAN_CONCURRENCY),
                );
            }
            Err(_) => {
                errs.insert(Field::Concurrency, "must be a positive integer".to_string());
            }
        }
        errs
    }

    pub fn to_pruner_config(&self) -> Result<PrunerConfig, String> {
        let targets = match self.scope.kind {
            ScopeKind::None => vec![],
            ScopeKind::Dms => vec![TargetResolution::AllDms],
            ScopeKind::Guilds => vec![TargetResolution::AllGuilds],
            ScopeKind::All => vec![TargetResolution::AllDms, TargetResolution::AllGuilds],
            ScopeKind::Channels => {
                let ids = channel_id_list(&self.scope.channels);
                if ids.is_empty() {
                    return Err("channels scope requires at least one channel ID".into());
                }
                vec![TargetResolution::Channels(ids)]
            }
        };
        let parse_date = |s: &str| (!s.trim().is_empty()).then(|| s.trim().to_string());
        let limit = self
            .limit
            .trim()
            .parse::<usize>()
            .map_err(|_| "invalid limit")?;
        let throttle_ms = self
            .throttle_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| "invalid throttle")?;
        let scan_throttle_ms = self
            .scan_throttle_ms
            .trim()
            .parse::<u64>()
            .map_err(|_| "invalid scan throttle")?;
        let concurrency = crate::config::clamp_scan_concurrency(
            self.concurrency
                .trim()
                .parse::<u64>()
                .map_err(|_| "invalid scan concurrency")?,
        );
        Ok(PrunerConfig {
            targets,
            before: parse_date(&self.before),
            after: parse_date(&self.after),
            limit,
            dry_run: self.dry_run,
            throttle_ms,
            scan_throttle_ms,
            concurrency,
            close_dms: self.close_dms,
            protect_ids: channel_id_list(&self.protect_users),
            dm_kind: self.dm_kind,
            content_filter: self.content_filter,
            keep_pinned: self.keep_pinned,
        })
    }
}
