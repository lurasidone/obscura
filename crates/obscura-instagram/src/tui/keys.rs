use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::action::{Action, Screen};

pub type KeyHint = (&'static str, &'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Text,
    Palette,
    Confirm,
    Help,
}

pub fn map(mode: InputMode, key: KeyEvent) -> Option<Action> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    match mode {
        InputMode::Text => map_text(key, ctrl, alt),
        InputMode::Palette => map_palette(key, ctrl, alt),
        InputMode::Confirm => match key.code {
            KeyCode::Char('c') if ctrl => Some(Action::Decline),
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') if !ctrl => {
                Some(Action::Confirm)
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Char('q') => {
                Some(Action::Decline)
            }
            _ => None,
        },
        InputMode::Help => match key.code {
            KeyCode::Char('c') if ctrl => Some(Action::Back),
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') => Some(Action::Back),
            _ => None,
        },
        InputMode::Normal => map_normal(key, ctrl, alt),
    }
}

fn map_text(key: KeyEvent, ctrl: bool, alt: bool) -> Option<Action> {
    if ctrl {
        return match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') => Some(Action::CancelEdit),
            KeyCode::Char('s') => Some(Action::CommitEdit),
            KeyCode::Char('u') => Some(Action::InputClear),
            KeyCode::Char('r') => Some(Action::Refresh),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Esc => Some(Action::CancelEdit),
        KeyCode::Enter => Some(Action::CommitEdit),
        KeyCode::Tab => Some(Action::CommitEdit),
        KeyCode::F(n @ 1..=4) => screen_for(n - 1),
        KeyCode::Backspace => Some(Action::InputBackspace),
        KeyCode::Delete => Some(Action::InputDelete),
        KeyCode::Left => Some(Action::InputLeft),
        KeyCode::Right => Some(Action::InputRight),
        KeyCode::Home => Some(Action::InputHome),
        KeyCode::End => Some(Action::InputEnd),
        KeyCode::Char(c) if !alt && !c.is_control() => Some(Action::Input(c)),
        _ => None,
    }
}

fn map_palette(key: KeyEvent, ctrl: bool, alt: bool) -> Option<Action> {
    if ctrl {
        return match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') => Some(Action::Back),
            KeyCode::Char('u') => Some(Action::InputClear),
            KeyCode::Char('p') => Some(Action::Palette),
            _ => None,
        };
    }
    match key.code {
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Enter => Some(Action::PaletteSelect),
        KeyCode::Up => Some(Action::PaletteUp),
        KeyCode::Down => Some(Action::PaletteDown),
        KeyCode::Backspace => Some(Action::InputBackspace),
        KeyCode::Delete => Some(Action::InputDelete),
        KeyCode::Left => Some(Action::InputLeft),
        KeyCode::Right => Some(Action::InputRight),
        KeyCode::Char(c) if !alt && !c.is_control() => Some(Action::Input(c)),
        _ => None,
    }
}

fn map_normal(key: KeyEvent, ctrl: bool, alt: bool) -> Option<Action> {
    if ctrl {
        return match key.code {
            KeyCode::Char('c') | KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Char('p') => Some(Action::Palette),
            KeyCode::Char('n') => Some(Action::CycleNext),
            KeyCode::Char('b') => Some(Action::CyclePrev),
            KeyCode::Char('l') => Some(Action::ClearLog),
            _ => None,
        };
    }
    if alt {
        return match key.code {
            KeyCode::Char(c @ '1'..='4') => screen_for(c as u8 - b'1'),
            _ => None,
        };
    }

    match key.code {
        KeyCode::Char(c @ '1'..='4') => screen_for(c as u8 - b'1'),
        KeyCode::F(n @ 1..=4) => screen_for(n - 1),
        KeyCode::Esc => Some(Action::Back),
        KeyCode::Char('q') => Some(Action::Quit),
        KeyCode::Char('?') => Some(Action::Help),
        KeyCode::Char(':') | KeyCode::Char('p') => Some(Action::Palette),
        KeyCode::Char('P') => Some(Action::ToggleProtect),
        KeyCode::Char('h') => Some(Action::PrevScreen),
        KeyCode::Char('l') => Some(Action::NextScreen),
        KeyCode::Tab => Some(Action::NextField),
        KeyCode::BackTab => Some(Action::PrevField),
        KeyCode::Down | KeyCode::Char('j') => Some(Action::Down),
        KeyCode::Up | KeyCode::Char('k') => Some(Action::Up),
        KeyCode::Enter => Some(Action::Activate),
        KeyCode::Char(' ') => Some(Action::Toggle),
        KeyCode::Char('i') | KeyCode::Char('e') => Some(Action::StartEdit),
        KeyCode::Char('a') => Some(Action::SelectAll),
        KeyCode::Char('u') => Some(Action::SelectNone),
        KeyCode::Char('v') => Some(Action::SelectInvert),
        KeyCode::Char('y') => Some(Action::Yank),
        KeyCode::Char('d') | KeyCode::Char('D') => Some(Action::ToggleDryRun),
        KeyCode::Char('R') => Some(Action::Probe),
        KeyCode::Char('[') | KeyCode::Left => Some(Action::CyclePrev),
        KeyCode::Char(']') | KeyCode::Right => Some(Action::CycleNext),
        KeyCode::F(5) => Some(Action::StartRun),
        KeyCode::F(6) => Some(Action::Back),
        _ => None,
    }
}

fn screen_for(index: u8) -> Option<Action> {
    Screen::ALL.get(index as usize).copied().map(Action::Goto)
}

// single source for the footer strip and the help overlay
pub fn footer_hints(screen: Screen, running: bool) -> &'static [KeyHint] {
    if running {
        return &[
            ("Esc", "cancel run"),
            ("1-4", "screens"),
            ("j/k", "log"),
            ("?", "help"),
        ];
    }
    match screen {
        Screen::Settings => &[
            ("1-4", "screens"),
            ("j/k", "move"),
            ("Space", "toggle"),
            ("i/e", "edit"),
            ("F5", "run"),
            ("?", "help"),
        ],
        Screen::Targets => &[
            ("Space", "pick"),
            ("P", "protect"),
            ("a/u/v", "all/none/invert"),
            ("R", "reconnect"),
            ("F5", "run"),
            ("?", "help"),
        ],
        Screen::Run => &[
            ("1-4", "screens"),
            ("F5", "run"),
            ("Esc", "cancel"),
            ("j/k", "scroll log"),
            ("Ctrl-L", "clear log"),
        ],
        Screen::Archive => &[
            ("1-4", "screens"),
            ("Tab", "column"),
            ("j/k", "move"),
            ("y", "copy"),
            ("Right", "filter"),
        ],
    }
}

pub const HELP_SECTIONS: &[(&str, &[KeyHint])] = &[
    (
        "Navigation",
        &[
            ("1 - 4", "go to Settings, Targets, Run or Archive"),
            ("h / l", "previous or next screen"),
            ("Tab / Shift-Tab", "next or previous field or column"),
            ("j / k, Down / Up", "move through rows or scroll the log"),
        ],
    ),
    (
        "Settings",
        &[
            ("Space", "turn the focused option on or off"),
            ("[ / ], Left / Right", "change the conversation type"),
            ("i / e", "edit the focused text field"),
            ("Enter / Esc", "save or discard the edit"),
        ],
    ),
    (
        "Targets",
        &[
            ("Space", "pick a thread for DMs and reactions"),
            ("a / u / v", "pick all, none or invert"),
            ("P", "protect or unprotect the focused thread"),
            ("R", "reconnect and reload threads"),
        ],
    ),
    (
        "Run and Archive",
        &[
            ("F5 / Enter", "start a run; in Archive only F5"),
            ("Esc / F6", "cancel at the next checkpoint"),
            ("d", "turn dry run on or off"),
            ("Ctrl-L", "clear the log"),
            ("[ / ]", "show all, live or dry-run records"),
            ("y", "copy the record as JSON"),
        ],
    ),
    (
        "Anywhere",
        &[
            (": / p", "open the command palette"),
            ("?", "show this help"),
            ("q / Ctrl-C", "quit, or close the open dialog"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::action::Screen;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn normal_navigation_keys() {
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('j'))),
            Some(Action::Down)
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Down)),
            Some(Action::Down)
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Enter)),
            Some(Action::Activate)
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char(' '))),
            Some(Action::Toggle)
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('q'))),
            Some(Action::Quit)
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('?'))),
            Some(Action::Help)
        );
    }

    #[test]
    fn digit_and_f_keys_switch_screens() {
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('1'))),
            Some(Action::Goto(Screen::Settings))
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('2'))),
            Some(Action::Goto(Screen::Targets))
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('3'))),
            Some(Action::Goto(Screen::Run))
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::Char('4'))),
            Some(Action::Goto(Screen::Archive))
        );

        assert_eq!(
            map(InputMode::Normal, key(KeyCode::F(1))),
            Some(Action::Goto(Screen::Settings))
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::F(2))),
            Some(Action::Goto(Screen::Targets))
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::F(3))),
            Some(Action::Goto(Screen::Run))
        );
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::F(4))),
            Some(Action::Goto(Screen::Archive))
        );
    }

    #[test]
    fn text_mode_commit_and_cancel() {
        assert_eq!(
            map(InputMode::Text, key(KeyCode::Enter)),
            Some(Action::CommitEdit)
        );
        assert_eq!(
            map(InputMode::Text, key(KeyCode::Esc)),
            Some(Action::CancelEdit)
        );
        assert_eq!(
            map(InputMode::Text, key(KeyCode::Tab)),
            Some(Action::CommitEdit)
        );
        assert_eq!(
            map(InputMode::Text, key(KeyCode::Char('x'))),
            Some(Action::Input('x'))
        );
    }

    #[test]
    fn confirm_dialog_keys() {
        assert_eq!(
            map(InputMode::Confirm, key(KeyCode::Enter)),
            Some(Action::Confirm)
        );
        assert_eq!(
            map(InputMode::Confirm, key(KeyCode::Char('y'))),
            Some(Action::Confirm)
        );
        assert_eq!(
            map(InputMode::Confirm, key(KeyCode::Char('n'))),
            Some(Action::Decline)
        );
        assert_eq!(
            map(InputMode::Confirm, key(KeyCode::Esc)),
            Some(Action::Decline)
        );
    }

    #[test]
    fn ctrl_c_closes_every_dialog_and_never_confirms() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(map(InputMode::Confirm, ctrl_c), Some(Action::Decline));
        assert_eq!(map(InputMode::Help, ctrl_c), Some(Action::Back));
        assert_eq!(map(InputMode::Palette, ctrl_c), Some(Action::Back));
        assert_eq!(map(InputMode::Text, ctrl_c), Some(Action::CancelEdit));
        assert_eq!(map(InputMode::Normal, ctrl_c), Some(Action::Quit));
    }

    #[test]
    fn f5_starts_a_run_from_every_screen() {
        assert_eq!(
            map(InputMode::Normal, key(KeyCode::F(5))),
            Some(Action::StartRun)
        );
    }
}
