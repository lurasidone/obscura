/// The four shared screens available in every platform TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Settings,
    Targets,
    Run,
    Archive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArchiveFocus {
    #[default]
    Days,
    Channels,
    Records,
}

impl ArchiveFocus {
    const ALL: [ArchiveFocus; 3] = [
        ArchiveFocus::Days,
        ArchiveFocus::Channels,
        ArchiveFocus::Records,
    ];

    fn step(self, forward: bool) -> Self {
        let index = Self::ALL
            .iter()
            .position(|focus| *focus == self)
            .unwrap_or(0);
        let len = Self::ALL.len();
        Self::ALL[if forward {
            (index + 1) % len
        } else {
            (index + len - 1) % len
        }]
    }

    pub fn next(self) -> Self {
        self.step(true)
    }

    pub fn prev(self) -> Self {
        self.step(false)
    }
}

impl Screen {
    pub const ALL: [Screen; 4] = [
        Screen::Settings,
        Screen::Targets,
        Screen::Run,
        Screen::Archive,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Settings => "settings",
            Screen::Targets => "targets",
            Screen::Run => "run",
            Screen::Archive => "archive",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|screen| *screen == self)
            .unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::{ArchiveFocus, Screen};

    #[test]
    fn navigation_wraps_in_both_directions() {
        assert_eq!(Screen::Settings.prev(), Screen::Archive);
        assert_eq!(Screen::Archive.next(), Screen::Settings);
        assert_eq!(Screen::Targets.title(), "targets");
        assert_eq!(ArchiveFocus::Days.prev(), ArchiveFocus::Records);
        assert_eq!(ArchiveFocus::Records.next(), ArchiveFocus::Days);
    }
}
