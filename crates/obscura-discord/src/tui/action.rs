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
    Token,
    Scope,
    Payload,
    Before,
    After,
    Limit,
    Throttle,
    ScanThrottle,
    Concurrency,
    DmKind,
    ContentFilter,
    KeepPinned,
    DryRun,
    CloseDms,
    Protect,
}

impl Field {
    pub const ALL: [Field; 15] = [
        Field::Token,
        Field::Scope,
        Field::Payload,
        Field::Before,
        Field::After,
        Field::Limit,
        Field::Throttle,
        Field::ScanThrottle,
        Field::Concurrency,
        Field::DmKind,
        Field::ContentFilter,
        Field::KeepPinned,
        Field::DryRun,
        Field::CloseDms,
        Field::Protect,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Token => "Discord token",
            Field::Scope => "scope",
            Field::Payload => "channel IDs",
            Field::Before => "before",
            Field::After => "after",
            Field::Limit => "limit",
            Field::Throttle => "throttle (ms)",
            Field::ScanThrottle => "scan throttle",
            Field::Concurrency => "scan parallel",
            Field::DmKind => "dm kind",
            Field::ContentFilter => "content filter",
            Field::KeepPinned => "keep pinned",
            Field::DryRun => "dry run",
            Field::CloseDms => "close emptied 1:1 DMs (never groups)",
            Field::Protect => "protect users",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Field::Token => "Discord user token (masked); DISCORD_TOKEN from env",
            Field::Scope => {
                "all / DMs / guilds / channels by ID / none; DM picks narrow all or DM scopes"
            }
            Field::Payload => "comma-separated channel IDs for channels scope",
            Field::Before => "only items older than this (RFC3339 or YYYY-MM-DD)",
            Field::After => "only items newer than this",
            Field::Limit => "maximum planned items across the run; 0 = unlimited",
            Field::Throttle => "minimum ms between DELETE requests (safe: 2000)",
            Field::ScanThrottle => "minimum ms between GET/scan requests (safe: 350)",
            Field::Concurrency => {
                "concurrent scan requests 1-8; divides scan throttle, so reads run Nx faster"
            }
            Field::DmKind => "DM filter: all / 1on1 / group",
            Field::ContentFilter => {
                "content filter: all / only attachments / has attachments / has links / text only"
            }
            Field::KeepPinned => "never delete pinned messages",
            Field::DryRun => "plan + archive only, never call DELETE",
            Field::CloseDms => {
                "close 1:1 DMs after a successful live run; group DMs are never left"
            }
            Field::Protect => {
                "never prune channels or DMs matching these IDs/usernames (comma-separated)"
            }
        }
    }

    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::Token
                | Field::Payload
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
    StartRun(crate::pruner::PrunerConfig),
    CancelRun,
    Yank(String),
}

pub const COMMANDS: [Command; 11] = [
    Command::new(
        "settings",
        "Open settings",
        "edit scope, filters and behaviour",
        Action::Goto(Screen::Settings),
    ),
    Command::new(
        "targets",
        "Open targets",
        "browse and select Discord DMs to prune",
        Action::Goto(Screen::Targets),
    ),
    Command::new(
        "probe",
        "Connect / refresh account",
        "fetch account identity and Discord DMs",
        Action::Probe,
    ),
    Command::new(
        "start",
        "Start prune run",
        "launch the pruner with current settings",
        Action::Activate,
    ),
    Command::new(
        "cancel",
        "Cancel run",
        "stop at the next delete checkpoint",
        Action::Back,
    ),
    Command::new(
        "open-run",
        "Open run screen",
        "live progress, per-target stats and log",
        Action::Goto(Screen::Run),
    ),
    Command::new(
        "archive",
        "Open archive",
        "browse the on-disk deletion history",
        Action::Goto(Screen::Archive),
    ),
    Command::new(
        "refresh-archive",
        "Reload archive",
        "re-read the history directory",
        Action::Refresh,
    ),
    Command::new(
        "dry-run",
        "Toggle dry run",
        "plan-only, never DELETE",
        Action::ToggleDryRun,
    ),
    Command::new("help", "Show key bindings", "help overlay", Action::Help),
    Command::new("quit", "Quit", "exit the TUI", Action::Quit),
];

pub fn commands() -> &'static [Command] {
    &COMMANDS
}
pub use obscura_core::tui::Screen;
