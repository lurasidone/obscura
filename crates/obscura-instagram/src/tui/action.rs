#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: &'static str,
    pub label: &'static str,
    pub hint: &'static str,
    pub action: Action,
}

impl Command {
    pub const fn new(
        id: &'static str,
        label: &'static str,
        hint: &'static str,
        action: Action,
    ) -> Self {
        Self {
            id,
            label,
            hint,
            action,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Quit,
    Help,
    Palette,
    PaletteDown,
    PaletteUp,
    PaletteSelect,
    Back,
    PrevScreen,
    NextScreen,
    Goto(Screen),
    Down,
    Up,
    CycleNext,
    CyclePrev,
    NextField,
    PrevField,
    Activate,
    StartRun,
    Toggle,
    ToggleProtect,
    ToggleDryRun,
    StartEdit,
    CommitEdit,
    CancelEdit,
    Input(char),
    InputBackspace,
    InputDelete,
    InputLeft,
    InputRight,
    InputHome,
    InputEnd,
    InputClear,
    SelectAll,
    SelectNone,
    SelectInvert,
    Probe,
    Refresh,
    ReloadArchive,
    Confirm,
    Decline,
    Yank,
    ClearLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Help,
    Palette,
    Confirm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    SessionId,
    ContentPosts,
    ContentDms,
    ContentDmReactions,
    ContentLikes,
    ContentSaved,
    ContentComments,
    ContentArchived,
    ContentReposts,
    ContentUnfollow,
    Threads,
    DmKind,
    DmMediaOnly,
    HideThreads,
    Protect,
    After,
    Before,
    Limit,
    DryRun,
    Throttle,
    ScanThrottle,
    Concurrency,
}

impl Field {
    pub const ALL: [Field; 22] = [
        Field::SessionId,
        Field::ContentPosts,
        Field::ContentDms,
        Field::ContentDmReactions,
        Field::ContentLikes,
        Field::ContentSaved,
        Field::ContentComments,
        Field::ContentArchived,
        Field::ContentReposts,
        Field::ContentUnfollow,
        Field::Threads,
        Field::DmKind,
        Field::DmMediaOnly,
        Field::HideThreads,
        Field::Protect,
        Field::After,
        Field::Before,
        Field::Limit,
        Field::DryRun,
        Field::Throttle,
        Field::ScanThrottle,
        Field::Concurrency,
    ];

    pub fn section(self) -> &'static str {
        match self {
            Field::SessionId => "Account",
            Field::ContentPosts
            | Field::ContentDms
            | Field::ContentDmReactions
            | Field::ContentLikes
            | Field::ContentSaved
            | Field::ContentComments
            | Field::ContentArchived
            | Field::ContentReposts
            | Field::ContentUnfollow => "What to delete",
            Field::Threads
            | Field::DmKind
            | Field::DmMediaOnly
            | Field::HideThreads
            | Field::Protect => "DMs and reactions",
            Field::After | Field::Before | Field::Limit => "Filters",
            Field::DryRun | Field::Throttle | Field::ScanThrottle | Field::Concurrency => "Run",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Field::SessionId => "Session ID",
            Field::ContentPosts => "Posts",
            Field::ContentDms => "DM messages",
            Field::ContentDmReactions => "DM reactions",
            Field::ContentLikes => "Likes",
            Field::ContentSaved => "Saved posts",
            Field::ContentComments => "Comments",
            Field::ContentArchived => "Archived posts",
            Field::ContentReposts => "Reposts",
            Field::ContentUnfollow => "Not following back",
            Field::Threads => "Only these threads",
            Field::DmKind => "Conversation type",
            Field::DmMediaOnly => "Media only",
            Field::HideThreads => "Hide threads",
            Field::Protect => "Protected",
            Field::After => "Newer than",
            Field::Before => "Older than",
            Field::Limit => "Item limit",
            Field::DryRun => "Dry run",
            Field::Throttle => "Delete delay (ms)",
            Field::ScanThrottle => "Scan delay (ms)",
            Field::Concurrency => "Parallel scans",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Field::SessionId => "The sessionid cookie from a browser where you are logged in.",
            Field::ContentPosts => "Delete your posts.",
            Field::ContentDms => "Unsend your DM messages. Other people's messages stay.",
            Field::ContentDmReactions => {
                "Remove your emoji reactions in DMs. Your messages stay unless DM messages is on."
            }
            Field::ContentLikes => "Unlike every post in your liked feed.",
            Field::ContentSaved => "Remove every post from your saved items.",
            Field::ContentComments => {
                "Delete your comments on your own posts. Not yet verified: see the README."
            }
            Field::ContentArchived => "Permanently delete your archived posts.",
            Field::ContentReposts => {
                "Remove native reposts by media-note ID. Instagram does not reliably expose these IDs for a cleanup scan."
            }
            Field::ContentUnfollow => {
                "Unfollow accounts that don't follow you back, except Protected ones. Never part of all."
            }
            Field::Threads => {
                "Thread IDs, comma-separated. Empty uses your Targets picks or the whole inbox."
            }
            Field::DmKind => "Limit the inbox and your picks to all, 1on1 or group. Typed thread IDs skip this.",
            Field::DmMediaOnly => "Keep text messages, Reels and shared posts. Unsend your other messages.",
            Field::HideThreads => {
                "After a live run, hide threads left with none of your messages. Skipped with Media only, dates or a limit."
            }
            Field::Protect => "Usernames, user IDs or thread IDs whose DMs are never touched. Never unfollowed either.",
            Field::After => "Only items newer than this: YYYY-MM-DD or RFC 3339, UTC.",
            Field::Before => "Only items older than this: YYYY-MM-DD or RFC 3339, UTC.",
            Field::Limit => "Stop after planning this many items. 0 means no limit.",
            Field::DryRun => "Plan and record what would go, without deleting anything.",
            Field::Throttle => "Minimum wait between delete requests. Minimum 200, recommended 2000.",
            Field::ScanThrottle => "Minimum wait between read requests. Minimum 100, recommended 350.",
            Field::Concurrency => "Parallel reads, 1 to 8. The scan delay is divided by this number.",
        }
    }

    /// How to change this field, shown under its description.
    pub fn how_to_change(self) -> &'static str {
        if self.is_text() {
            "Press i or e to edit."
        } else if self == Field::DmKind {
            "Press [ or ] to change."
        } else {
            "Press Space to toggle."
        }
    }

    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::SessionId
                | Field::Threads
                | Field::Before
                | Field::After
                | Field::Limit
                | Field::Throttle
                | Field::ScanThrottle
                | Field::Concurrency
                | Field::Protect
        )
    }
}

#[derive(Debug)]
pub enum Effect {
    Quit,
    Probe,
    LoadTargets,
    StartRun(crate::cleaner::CleanerConfig),
    CancelRun,
    Yank(String),
}

pub const COMMANDS: [Command; 11] = [
    Command::new(
        "settings",
        "Open settings",
        "choose what to delete and how",
        Action::Goto(Screen::Settings),
    ),
    Command::new(
        "targets",
        "Open targets",
        "browse and pick DM threads",
        Action::Goto(Screen::Targets),
    ),
    Command::new(
        "probe",
        "Reconnect",
        "reload your account and DM threads",
        Action::Probe,
    ),
    Command::new(
        "start",
        "Start run",
        "run with the current settings",
        Action::StartRun,
    ),
    Command::new(
        "cancel",
        "Cancel run",
        "stop at the next checkpoint",
        Action::Back,
    ),
    Command::new(
        "open-run",
        "Open run screen",
        "live progress, targets and log",
        Action::Goto(Screen::Run),
    ),
    Command::new(
        "archive",
        "Open archive",
        "browse the deletion history",
        Action::Goto(Screen::Archive),
    ),
    Command::new(
        "refresh-archive",
        "Reload archive",
        "read the history folder again",
        Action::ReloadArchive,
    ),
    Command::new(
        "dry-run",
        "Toggle dry run",
        "plan without deleting",
        Action::ToggleDryRun,
    ),
    Command::new("help", "Show shortcuts", "keyboard help", Action::Help),
    Command::new("quit", "Quit", "exit the app", Action::Quit),
];

pub fn commands() -> &'static [Command] {
    &COMMANDS
}
pub use obscura_core::tui::Screen;
