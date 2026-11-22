use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub token: String,
    pub base_url: String,
    pub session_path: String,
    pub scope: ScopeMode,
    pub scope_form: ScopeForm,
    pub before: String,
    pub after: String,
    pub limit: String,
    pub throttle_ms: String,
    pub scan_throttle_ms: String,
    pub concurrency: String,
    pub dry_run: bool,
    pub close_dms: bool,
    pub protect_ids: Vec<String>,
    pub dm_kind: DmKind,
    pub content_filter: ContentFilter,
    pub keep_pinned: bool,
    pub history_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScopeMode {
    None,
    Dms,
    Guilds,
    All,
    Channels(String),
}

impl ScopeMode {
    pub fn kind(&self) -> ScopeKind {
        match self {
            Self::None => ScopeKind::None,
            Self::Dms => ScopeKind::Dms,
            Self::Guilds => ScopeKind::Guilds,
            Self::All => ScopeKind::All,
            Self::Channels(_) => ScopeKind::Channels,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScopeKind {
    None,
    Dms,
    Guilds,
    #[default]
    All,
    Channels,
}

impl ScopeKind {
    pub const ALL: [ScopeKind; 5] = [
        ScopeKind::None,
        ScopeKind::Dms,
        ScopeKind::Guilds,
        ScopeKind::All,
        ScopeKind::Channels,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScopeKind::None => "none",
            ScopeKind::Dms => "all DMs",
            ScopeKind::Guilds => "all guilds",
            ScopeKind::All => "all (DMs + guilds)",
            ScopeKind::Channels => "channels by ID",
        }
    }

    pub fn has_payload(self) -> bool {
        self == ScopeKind::Channels
    }

    fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScopeForm {
    pub kind: ScopeKind,
    pub channels: String,
}

impl ScopeForm {
    pub fn from_scope_mode(mode: &ScopeMode) -> Self {
        Self {
            kind: mode.kind(),
            channels: match mode {
                ScopeMode::Channels(ids) => ids.clone(),
                _ => String::new(),
            },
        }
    }

    pub fn summary(&self) -> String {
        match self.kind {
            ScopeKind::None => "none".to_string(),
            ScopeKind::Dms => "DMs".to_string(),
            ScopeKind::Guilds => "guilds".to_string(),
            ScopeKind::All => "all".to_string(),
            ScopeKind::Channels => {
                let trimmed = self.channels.trim();
                if trimmed.is_empty() {
                    "channels: (none)".to_string()
                } else {
                    format!("channels: {trimmed}")
                }
            }
        }
    }

    pub fn next(&mut self) {
        self.kind = self.kind.next();
    }

    pub fn prev(&mut self) {
        self.kind = self.kind.prev();
    }
}

pub fn channel_id_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in raw.split([',', ' ', '\t', '\n']) {
        let trimmed = part.trim();
        if !trimmed.is_empty() && !out.iter().any(|kept| kept == trimmed) {
            out.push(trimmed.to_string());
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DmKind {
    #[default]
    All,
    OneOnOne,
    Group,
}

impl DmKind {
    pub fn matches(&self, is_group: bool) -> bool {
        match self {
            Self::All => true,
            Self::OneOnOne => !is_group,
            Self::Group => is_group,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::OneOnOne => "1on1",
            Self::Group => "group",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::All => Self::OneOnOne,
            Self::OneOnOne => Self::Group,
            Self::Group => Self::All,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::All => Self::Group,
            Self::OneOnOne => Self::All,
            Self::Group => Self::OneOnOne,
        }
    }
}

/// Message content filter: which of your own messages are planned for deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ContentFilter {
    /// Every deletable own message (default).
    #[default]
    All,
    /// Messages whose content is empty and that have at least one attachment.
    OnlyAttachments,
    /// Messages carrying one or more attachments (text allowed).
    HasAttachments,
    /// Messages with links in the content or with embeds.
    HasLinks,
    /// Plain text messages without attachments or embeds.
    TextOnly,
}

impl ContentFilter {
    pub const ALL: [ContentFilter; 5] = [
        ContentFilter::All,
        ContentFilter::OnlyAttachments,
        ContentFilter::HasAttachments,
        ContentFilter::HasLinks,
        ContentFilter::TextOnly,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::OnlyAttachments => "only attachments",
            Self::HasAttachments => "has attachments",
            Self::HasLinks => "has links",
            Self::TextOnly => "text only",
        }
    }
}

impl std::str::FromStr for ContentFilter {
    type Err = ();

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Ok(match raw.trim().to_ascii_lowercase().as_str() {
            "all" | "any" => Self::All,
            "only-attachments" | "onlyattachments" | "only_attachments" | "only" => {
                Self::OnlyAttachments
            }
            "has-attachments" | "hasattachments" | "has_attachments" | "attachments" => {
                Self::HasAttachments
            }
            "has-links" | "haslinks" | "has_links" | "links" => Self::HasLinks,
            "text-only" | "textonly" | "text_only" | "text" => Self::TextOnly,
            _ => return Err(()),
        })
    }
}

impl ContentFilter {
    fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// Whether a message passes this filter. Pinned handling is separate
    /// (`keep_pinned`), applied by the pruner itself.
    pub fn matches(self, msg: &crate::model::Message) -> bool {
        match self {
            Self::All => true,
            Self::OnlyAttachments => msg.is_only_attachments(),
            Self::HasAttachments => msg.has_attachments(),
            Self::HasLinks => msg.has_links(),
            Self::TextOnly => msg.is_text_only(),
        }
    }
}

impl Config {
    pub fn from_env() -> Self {
        let scope = parse_scope_mode("DISCORD_SCOPE");
        let scope_form = ScopeForm::from_scope_mode(&scope);
        let token = trimmed_env("DISCORD_TOKEN")
            .or_else(|| env::var("DISCORD_TOKEN").ok())
            .unwrap_or_default();
        Self {
            token,
            base_url: trimmed_env("DISCORD_BASE_URL")
                .map(|url| url.trim_end_matches('/').to_string())
                .unwrap_or_else(|| crate::device::API_BASE.to_string()),
            session_path: trimmed_env("DISCORD_SESSION_PATH")
                .unwrap_or_else(|| crate::device::DEFAULT_SESSION_PATH.to_string()),
            scope,
            scope_form,
            before: env::var("DISCORD_BEFORE")
                .unwrap_or_default()
                .trim()
                .to_string(),
            after: env::var("DISCORD_AFTER")
                .unwrap_or_default()
                .trim()
                .to_string(),
            limit: parse_string_int("DISCORD_LIMIT", "0"),
            throttle_ms: parse_string_int("DISCORD_THROTTLE_MS", "2000"),
            scan_throttle_ms: parse_string_int("DISCORD_SCAN_THROTTLE_MS", "350"),
            concurrency: parse_string_int("DISCORD_SCAN_CONCURRENCY", "4"),
            dry_run: parse_bool("DISCORD_DRY_RUN", true),
            close_dms: parse_bool("DISCORD_CLOSE_DMS", true),
            protect_ids: Vec::new(),
            dm_kind: parse_dm_kind(),
            content_filter: parse_content_filter(),
            keep_pinned: parse_bool("DISCORD_KEEP_PINNED", false),
            history_dir: env::var("PRUNER_HISTORY_DIR")
                .unwrap_or_else(|_| "./data/discord/history".to_string()),
        }
    }
}

/// Environment value with whitespace stripped; `None` when unset or blank.
fn trimmed_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn dm_kind_from_str(raw: &str) -> Option<DmKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Some(DmKind::All),
        "1on1" | "1-on-1" | "dm" => Some(DmKind::OneOnOne),
        "group" | "grp" => Some(DmKind::Group),
        _ => None,
    }
}

fn parse_content_filter() -> ContentFilter {
    const KEY: &str = "DISCORD_CONTENT_FILTER";
    if let Ok(raw) = env::var(KEY) {
        return raw.parse::<ContentFilter>().unwrap_or_else(|_| {
            warn(
                KEY,
                &raw,
                "expected all|only-attachments|has-attachments|has-links|text-only; using all",
            );
            ContentFilter::All
        });
    }
    ContentFilter::All
}

pub(crate) fn parse_dm_kind() -> DmKind {
    const KEY: &str = "DISCORD_DM_KIND";
    if let Ok(raw) = env::var(KEY) {
        return dm_kind_from_str(&raw).unwrap_or_else(|| {
            warn(KEY, &raw, "expected all|1on1|group; using all");
            DmKind::All
        });
    }
    DmKind::All
}

pub(crate) fn parse_scope(raw: &str) -> Result<ScopeMode, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("all") {
        return Ok(ScopeMode::All);
    }
    if trimmed.eq_ignore_ascii_case("none") {
        return Ok(ScopeMode::None);
    }
    if trimmed.eq_ignore_ascii_case("dms")
        || trimmed.eq_ignore_ascii_case("dm")
        || trimmed.eq_ignore_ascii_case("all_dms")
    {
        return Ok(ScopeMode::Dms);
    }
    if trimmed.eq_ignore_ascii_case("guilds")
        || trimmed.eq_ignore_ascii_case("guild")
        || trimmed.eq_ignore_ascii_case("servers")
    {
        return Ok(ScopeMode::Guilds);
    }
    for prefix in ["channels:", "channels ", "channel:"] {
        if let Some(rest) = stripped_prefix_ci(trimmed, prefix) {
            let ids = channel_id_list(rest);
            if ids.is_empty() {
                return Err(format!("scope '{prefix}' requires at least one channel id"));
            }
            return Ok(ScopeMode::Channels(ids.join(",")));
        }
    }
    let ids = channel_id_list(trimmed);
    if !ids.is_empty() && ids.iter().all(|id| id.chars().all(|c| c.is_ascii_digit())) {
        return Ok(ScopeMode::Channels(ids.join(",")));
    }
    Err(format!(
        "unrecognised scope '{raw}': expected dms, guilds, all, none, or channels:<id>,<id>"
    ))
}

fn parse_scope_mode(key: &str) -> ScopeMode {
    let Ok(raw) = env::var(key) else {
        return ScopeMode::All;
    };
    match parse_scope(&raw) {
        Ok(mode) => mode,
        Err(_) => {
            warn(key, &raw, "unrecognised scope; using 'all'");
            ScopeMode::All
        }
    }
}

fn stripped_prefix_ci<'a>(haystack: &'a str, prefix: &str) -> Option<&'a str> {
    haystack
        .get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .and_then(|_| haystack.get(prefix.len()..))
}

pub(crate) fn parse_bool(key: &str, default: bool) -> bool {
    if let Ok(raw) = env::var(key) {
        return match raw.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" => true,
            "0" | "false" | "no" | "off" | "" => false,
            _ => {
                warn(
                    key,
                    &raw,
                    &format!("expected bool (true/false); using {default}"),
                );
                default
            }
        };
    }
    default
}

pub const MAX_SCAN_CONCURRENCY: u32 = 8;

pub fn clamp_scan_concurrency(raw: u64) -> u32 {
    raw.clamp(1, u64::from(MAX_SCAN_CONCURRENCY)) as u32
}

pub(crate) fn parse_string_int(key: &str, default: &str) -> String {
    if let Ok(raw) = env::var(key) {
        let trimmed = raw.trim();
        if trimmed.parse::<u64>().is_ok() {
            return trimmed.to_string();
        }
        warn(
            key,
            &raw,
            &format!("expected non-negative integer; using {default}"),
        );
        return default.to_string();
    }
    default.to_string()
}

fn warn(key: &str, value: &str, hint: &str) {
    eprintln!("[discord-pruner] {key}={value:?}: {hint}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_key(prefix: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!("PRUNER_TEST_{prefix}_{}", N.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn scope_all_aliases() {
        for v in ["", "all", "ALL"] {
            let k = unique_key("scope");
            env::set_var(&k, v);
            assert!(matches!(parse_scope_mode(&k), ScopeMode::All));
            env::remove_var(&k);
        }
    }

    #[test]
    fn scope_dms_and_guilds() {
        let k = unique_key("scope");
        env::set_var(&k, "dms");
        assert!(matches!(parse_scope_mode(&k), ScopeMode::Dms));
        env::set_var(&k, "guilds");
        assert!(matches!(parse_scope_mode(&k), ScopeMode::Guilds));
        env::remove_var(&k);
    }

    #[test]
    fn scope_unknown_kind_warns_and_defaults_to_all() {
        let k = unique_key("scope");
        env::set_var(&k, "posts");
        assert!(matches!(parse_scope_mode(&k), ScopeMode::All));
        env::remove_var(&k);
    }

    #[test]
    fn scope_channels_prefix_and_bare() {
        let k = unique_key("scope");
        env::set_var(&k, "channels:111,222");
        assert!(matches!(&parse_scope_mode(&k), ScopeMode::Channels(s) if s == "111,222"));
        env::set_var(&k, "channel:111");
        assert!(matches!(&parse_scope_mode(&k), ScopeMode::Channels(s) if s == "111"));
        env::set_var(&k, "1,2,3");
        assert!(matches!(&parse_scope_mode(&k), ScopeMode::Channels(s) if s == "1,2,3"));
        env::set_var(&k, "987654321");
        assert!(matches!(&parse_scope_mode(&k), ScopeMode::Channels(s) if s == "987654321"));
        env::remove_var(&k);
    }

    #[test]
    fn scope_none_is_empty() {
        let k = unique_key("scope");
        env::set_var(&k, "none");
        assert!(matches!(parse_scope_mode(&k), ScopeMode::None));
        env::remove_var(&k);
    }

    #[test]
    fn scan_concurrency_clamps_into_supported_range() {
        assert_eq!(clamp_scan_concurrency(0), 1);
        assert_eq!(clamp_scan_concurrency(1), 1);
        assert_eq!(clamp_scan_concurrency(4), 4);
        assert_eq!(
            clamp_scan_concurrency(u64::from(MAX_SCAN_CONCURRENCY)),
            MAX_SCAN_CONCURRENCY
        );
        assert_eq!(clamp_scan_concurrency(999), MAX_SCAN_CONCURRENCY);
    }

    #[test]
    fn dry_run_defaults_to_enabled() {
        let key = unique_key("dry_run");
        env::remove_var(&key);
        assert!(parse_bool(&key, true));
    }

    #[test]
    fn scope_form_cycling() {
        let mut form = ScopeForm {
            kind: ScopeKind::Dms,
            channels: String::new(),
        };
        assert_eq!(form.kind, ScopeKind::Dms);
        form.next();
        assert_eq!(form.kind, ScopeKind::Guilds);
        form.next();
        assert_eq!(form.kind, ScopeKind::All);
        form.next();
        assert_eq!(form.kind, ScopeKind::Channels);
        form.next();
        assert_eq!(form.kind, ScopeKind::None);
        form.next();
        assert_eq!(form.kind, ScopeKind::Dms);
        form.prev();
        assert_eq!(form.kind, ScopeKind::None);
        form.prev();
        assert_eq!(form.kind, ScopeKind::Channels);
    }

    #[test]
    fn scope_form_mirrors_scope_mode() {
        let form = ScopeForm::from_scope_mode(&ScopeMode::Dms);
        assert_eq!(form.kind, ScopeKind::Dms);
        assert!(form.channels.is_empty());

        let form = ScopeForm::from_scope_mode(&ScopeMode::Channels("7,8".to_string()));
        assert_eq!(form.kind, ScopeKind::Channels);
        assert_eq!(form.channels, "7,8");
        assert_eq!(form.summary(), "channels: 7,8");
    }

    #[test]
    fn none_scope_summary_is_none() {
        let form = ScopeForm::from_scope_mode(&ScopeMode::None);
        assert_eq!(form.summary(), "none");
        assert!(!form.kind.has_payload());
    }
}
