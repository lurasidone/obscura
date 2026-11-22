use std::env;

#[derive(Debug, Clone)]
pub struct Config {
    pub sessionid: String,
    pub base_url: String,
    pub session_path: String,
    pub content: Selection,
    pub threads: String,
    pub before: String,
    pub after: String,
    pub limit: String,
    pub throttle_ms: String,
    pub scan_throttle_ms: String,
    pub concurrency: String,
    pub dry_run: bool,
    pub hide_threads: bool,
    pub protect_users: Vec<String>,
    pub dm_kind: DmKind,
    pub dm_media_only: bool,
    pub history_dir: String,
}

/// Everything a run can delete, as chosen with `--content` or in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Selection {
    pub posts: bool,
    pub dms: bool,
    pub activity: ContentFlags,
}

impl Selection {
    const ALL: Self = Self {
        posts: true,
        dms: true,
        activity: ContentFlags::ALL,
    };

    /// Enables one class; false if the token is not a known class name.
    fn enable(&mut self, token: &str) -> bool {
        match token {
            "posts" => self.posts = true,
            "dms" => self.dms = true,
            "reactions" => self.activity.dm_reactions = true,
            "likes" => self.activity.likes = true,
            "saved" => self.activity.saved = true,
            "comments" => self.activity.comments = true,
            "archived" => self.activity.archived = true,
            "reposts" => self.activity.reposts = true,
            "all" => {
                // unfollowing is never implied, so `all` must not clear it either
                let unfollow = self.activity.unfollow;
                *self = Self::ALL;
                self.activity.unfollow = unfollow;
            }
            "unfollow" => self.activity.unfollow = true,
            _ => return false,
        }
        true
    }

    pub fn any(self) -> bool {
        self.posts || self.dms || self.activity.any()
    }

    /// True when the run has to walk DM threads.
    pub fn touches_threads(self) -> bool {
        self.dms || self.activity.dm_reactions
    }

    pub fn label(self) -> String {
        let mut on = Vec::new();
        if self.posts {
            on.push("posts");
        }
        if self.dms {
            on.push("dms");
        }
        on.extend(self.activity.names());
        if on.is_empty() {
            "nothing".to_string()
        } else {
            on.join(", ")
        }
    }
}

/// The classes the cleaner scans on top of posts and DM messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContentFlags {
    pub likes: bool,
    pub saved: bool,
    pub comments: bool,
    pub archived: bool,
    pub reposts: bool,
    pub dm_reactions: bool,
    /// Unfollow accounts that don't follow you back.
    pub unfollow: bool,
}

impl ContentFlags {
    const NAMES: [&'static str; 7] = [
        "reactions",
        "likes",
        "saved",
        "comments",
        "archived",
        "reposts",
        "unfollow",
    ];

    const ALL: Self = Self {
        likes: true,
        saved: true,
        comments: true,
        archived: true,
        reposts: true,
        dm_reactions: true,
        // `all` means your content; following nobody back has to be asked for by name
        unfollow: false,
    };

    fn as_array(self) -> [bool; 7] {
        [
            self.dm_reactions,
            self.likes,
            self.saved,
            self.comments,
            self.archived,
            self.reposts,
            self.unfollow,
        ]
    }

    fn names(self) -> Vec<&'static str> {
        Self::NAMES
            .iter()
            .zip(self.as_array())
            .filter(|(_, enabled)| *enabled)
            .map(|(name, _)| *name)
            .collect()
    }

    pub fn any(self) -> bool {
        self.as_array().iter().any(|on| *on)
    }

    pub fn count(self) -> usize {
        self.as_array().iter().filter(|on| **on).count()
    }

    pub fn label(self) -> String {
        let names = self.names();
        if names.is_empty() {
            "none".to_string()
        } else {
            names.join(", ")
        }
    }
}

pub fn thread_id_list(raw: &str) -> Vec<String> {
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

impl Config {
    pub fn from_env() -> Self {
        Self {
            sessionid: env::var("INSTAGRAM_SESSIONID").unwrap_or_default(),
            base_url: trimmed_env("INSTAGRAM_BASE_URL")
                .map(|url| url.trim_end_matches('/').to_string())
                .unwrap_or_else(|| crate::device::API_BASE.to_string()),
            session_path: trimmed_env("INSTAGRAM_SESSION_PATH")
                .unwrap_or_else(|| crate::device::DEFAULT_SESSION_PATH.to_string()),
            content: selection_from_env(),
            threads: thread_id_list(&env::var("INSTAGRAM_THREADS").unwrap_or_default()).join(","),
            before: trimmed_env("INSTAGRAM_BEFORE").unwrap_or_default(),
            after: trimmed_env("INSTAGRAM_AFTER").unwrap_or_default(),
            limit: parse_string_int("INSTAGRAM_LIMIT", "0"),
            throttle_ms: parse_string_int("INSTAGRAM_DELETE_DELAY_MS", "2000"),
            scan_throttle_ms: parse_string_int("INSTAGRAM_SCAN_DELAY_MS", "350"),
            concurrency: parse_string_int("INSTAGRAM_SCAN_CONCURRENCY", "4"),
            dry_run: parse_bool("INSTAGRAM_DRY_RUN", true),
            hide_threads: parse_bool("INSTAGRAM_HIDE_THREADS", false),
            protect_users: Vec::new(),
            dm_kind: dm_kind_from_env(),
            dm_media_only: parse_bool("INSTAGRAM_DM_MEDIA_ONLY", false),
            history_dir: trimmed_env("INSTAGRAM_HISTORY_DIR")
                .unwrap_or_else(|| "./data/instagram/history".to_string()),
        }
    }
}

/// Variables from earlier versions, with what to use instead. Ignoring them
/// could change what a run deletes, so a set one stops the program.
const REPLACED_VARS: [(&str, &str); 6] = [
    (
        "INSTAGRAM_SCOPE",
        "list posts and dms in INSTAGRAM_CONTENT, and thread IDs in INSTAGRAM_THREADS",
    ),
    (
        "INSTAGRAM_DM_REACTIONS_ONLY",
        "list reactions without dms in INSTAGRAM_CONTENT",
    ),
    ("INSTAGRAM_DM_PHOTOS_ONLY", "use INSTAGRAM_DM_MEDIA_ONLY"),
    ("INSTAGRAM_THROTTLE_MS", "use INSTAGRAM_DELETE_DELAY_MS"),
    ("INSTAGRAM_SCAN_THROTTLE_MS", "use INSTAGRAM_SCAN_DELAY_MS"),
    ("PRUNER_HISTORY_DIR", "use INSTAGRAM_HISTORY_DIR"),
];

pub fn check_replaced_vars() -> Result<(), String> {
    let found: Vec<String> = REPLACED_VARS
        .iter()
        .filter(|(key, _)| trimmed_env(key).is_some())
        .map(|(key, fix)| format!("  {key}: {fix}"))
        .collect();
    if found.is_empty() {
        return Ok(());
    }
    Err(format!(
        "these settings are no longer supported, update your .env or shell:\n{}",
        found.join("\n")
    ))
}

/// Environment value with whitespace stripped; `None` when unset or blank.
pub(crate) fn trimmed_env(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn content_tokens(raw: &str) -> impl Iterator<Item = String> + '_ {
    raw.split([',', ' ', '\t', '\n'])
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| t.to_ascii_lowercase())
}

const CONTENT_CHOICES: &str =
    "posts, dms, reactions, likes, saved, comments, archived, reposts, all or unfollow";

fn selection_from_env() -> Selection {
    let raw = env::var("INSTAGRAM_CONTENT").unwrap_or_default();
    let mut selection = Selection::default();
    for token in content_tokens(&raw) {
        if !selection.enable(&token) {
            warn(
                "INSTAGRAM_CONTENT",
                &token,
                &format!("expected {CONTENT_CHOICES}; ignoring it"),
            );
        }
    }
    selection
}

pub(crate) fn parse_content_value(raw: &str) -> Result<Selection, String> {
    let mut selection = Selection::default();
    for token in content_tokens(raw) {
        if !selection.enable(&token) {
            return Err(format!(
                "invalid --content '{token}': expected {CONTENT_CHOICES}"
            ));
        }
    }
    Ok(selection)
}

fn dm_kind_from_str(raw: &str) -> Option<DmKind> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "all" => Some(DmKind::All),
        "1on1" => Some(DmKind::OneOnOne),
        "group" => Some(DmKind::Group),
        _ => None,
    }
}

fn dm_kind_from_env() -> DmKind {
    let Ok(raw) = env::var("INSTAGRAM_DM_KIND") else {
        return DmKind::All;
    };
    dm_kind_from_str(&raw).unwrap_or_else(|| {
        warn(
            "INSTAGRAM_DM_KIND",
            &raw,
            "expected all, 1on1 or group; using all",
        );
        DmKind::All
    })
}

pub(crate) fn parse_dm_kind_value(raw: &str) -> Result<DmKind, String> {
    dm_kind_from_str(raw)
        .ok_or_else(|| format!("invalid --dm-kind '{raw}': expected all, 1on1 or group"))
}

pub(crate) fn parse_bool(key: &str, default: bool) -> bool {
    let Ok(raw) = env::var(key) else {
        return default;
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => true,
        "0" | "false" | "no" | "off" => false,
        // a blank value says nothing, and must never turn the dry run off
        "" => default,
        _ => {
            warn(
                key,
                &raw,
                &format!("expected true or false; using {default}"),
            );
            default
        }
    }
}

pub const MAX_SCAN_CONCURRENCY: u32 = 8;

pub fn clamp_scan_concurrency(raw: u64) -> u32 {
    raw.clamp(1, u64::from(MAX_SCAN_CONCURRENCY)) as u32
}

pub(crate) fn parse_string_int(key: &str, default: &str) -> String {
    let Ok(raw) = env::var(key) else {
        return default.to_string();
    };
    let trimmed = raw.trim();
    if trimmed.parse::<u64>().is_ok() {
        return trimmed.to_string();
    }
    warn(
        key,
        &raw,
        &format!("expected a non-negative integer; using {default}"),
    );
    default.to_string()
}

fn warn(key: &str, value: &str, hint: &str) {
    eprintln!("[instagram-cleaner] {key}={value:?}: {hint}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_key(prefix: &str) -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        format!(
            "CLEANER_TEST_{prefix}_{}",
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    #[test]
    fn selection_label_names_every_enabled_class() {
        assert_eq!(Selection::default().label(), "nothing");

        let selection = parse_content_value("reposts, posts, reactions").unwrap();
        assert_eq!(selection.label(), "posts, reactions, reposts");
        assert_eq!(selection.activity.label(), "reactions, reposts");
        assert_eq!(selection.activity.count(), 2);
    }

    #[test]
    fn content_parses_every_class_and_rejects_typos() {
        assert_eq!(parse_content_value("").unwrap(), Selection::default());

        let posts = parse_content_value("POSTS").unwrap();
        assert!(posts.posts && !posts.dms && !posts.activity.any());

        let both = parse_content_value("likes, saved").unwrap();
        assert!(both.activity.likes && both.activity.saved && both.activity.count() == 2);
        assert!(!both.touches_threads());

        let all = parse_content_value("all").unwrap();
        assert_eq!(all, Selection::ALL);
        assert_eq!(all.activity.count(), ContentFlags::NAMES.len() - 1);

        assert!(parse_content_value("likez").is_err());
        assert!(parse_content_value("dm-reactions").is_err());
    }

    #[test]
    fn unfollow_is_only_on_when_named() {
        assert!(!parse_content_value("all").unwrap().activity.unfollow);

        let alone = parse_content_value("unfollow").unwrap();
        assert!(alone.activity.unfollow && alone.any() && !alone.touches_threads());
        assert_eq!(alone.label(), "unfollow");

        for raw in ["all, unfollow", "unfollow, all"] {
            let both = parse_content_value(raw).unwrap();
            assert!(both.posts && both.activity.unfollow, "{raw}");
        }
    }

    #[test]
    fn reactions_and_dms_both_touch_threads() {
        assert!(parse_content_value("dms").unwrap().touches_threads());
        assert!(parse_content_value("reactions").unwrap().touches_threads());
        assert!(!parse_content_value("posts").unwrap().touches_threads());
    }

    #[test]
    fn nothing_is_selected_by_default() {
        assert!(!Selection::default().any());
    }

    #[test]
    fn replaced_vars_stop_the_program_and_name_the_fix() {
        assert!(REPLACED_VARS
            .iter()
            .all(|(_, fix)| fix.contains("INSTAGRAM_")));

        env::set_var("INSTAGRAM_DM_PHOTOS_ONLY", "true");
        let err = check_replaced_vars().unwrap_err();
        env::remove_var("INSTAGRAM_DM_PHOTOS_ONLY");
        assert!(err.contains("INSTAGRAM_DM_MEDIA_ONLY"), "{err}");
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
    fn a_blank_value_keeps_the_default() {
        let key = unique_key("blank_bool");
        env::set_var(&key, "  ");
        assert!(parse_bool(&key, true));
        assert!(!parse_bool(&key, false));
        env::remove_var(&key);
    }

    #[test]
    fn dry_run_defaults_to_enabled() {
        let key = unique_key("dry_run");
        env::remove_var(&key);
        assert!(parse_bool(&key, true));
    }

    #[test]
    fn parse_dm_kind_variants() {
        assert_eq!(parse_dm_kind_value("all").unwrap(), DmKind::All);
        assert_eq!(parse_dm_kind_value("1on1").unwrap(), DmKind::OneOnOne);
        assert_eq!(parse_dm_kind_value("group").unwrap(), DmKind::Group);
        assert!(parse_dm_kind_value("invalid").is_err());
    }
}
