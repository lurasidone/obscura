use std::collections::HashSet;

use crate::events::LogLevel;

use super::super::action::{self, Action, Effect, Field, Overlay, Screen};
use super::{AppState, ArchiveFocus, Connection};

impl AppState {
    pub fn apply_action(&mut self, action: Action) -> Vec<Effect> {
        match action {
            Action::Quit => vec![Effect::Quit],
            Action::Help => {
                self.overlay = if self.overlay == Overlay::Help {
                    Overlay::None
                } else {
                    Overlay::Help
                };
                Vec::new()
            }
            Action::Palette => {
                self.overlay = if self.overlay == Overlay::Palette {
                    Overlay::None
                } else {
                    self.palette.query.clear();
                    self.palette.update_matches();
                    self.palette.cursor = 0;
                    Overlay::Palette
                };
                Vec::new()
            }
            Action::PaletteDown => {
                if !self.palette.matches.is_empty() {
                    self.palette.cursor = (self.palette.cursor + 1) % self.palette.matches.len();
                }
                Vec::new()
            }
            Action::PaletteUp => {
                if !self.palette.matches.is_empty() {
                    self.palette.cursor = (self.palette.cursor + self.palette.matches.len() - 1)
                        % self.palette.matches.len();
                }
                Vec::new()
            }
            Action::PaletteSelect => {
                if let Some(&cmd_idx) = self.palette.matches.get(self.palette.cursor) {
                    let cmd = action::commands()[cmd_idx].action.clone();
                    self.overlay = Overlay::None;
                    self.apply_action(cmd)
                } else {
                    self.overlay = Overlay::None;
                    Vec::new()
                }
            }
            Action::Back => {
                if self.overlay != Overlay::None {
                    self.overlay = Overlay::None;
                    Vec::new()
                } else if self.settings.editing {
                    self.settings.cancel_edit();
                    Vec::new()
                } else if self.run.is_running() {
                    self.run.cancelling = true;
                    vec![Effect::CancelRun]
                } else {
                    Vec::new()
                }
            }
            Action::PrevScreen => {
                self.screen = self.screen.prev();
                Vec::new()
            }
            Action::NextScreen => {
                self.screen = self.screen.next();
                Vec::new()
            }
            Action::Goto(screen) => {
                // otherwise the keys of the next screen keep typing into the hidden field
                self.commit_edit();
                self.screen = screen;
                Vec::new()
            }
            Action::NextField => {
                match self.screen {
                    Screen::Settings => self.settings.move_cursor(true),
                    Screen::Archive => self.archive.focus = self.archive.focus.next(),
                    Screen::Targets | Screen::Run => {}
                }
                Vec::new()
            }
            Action::PrevField => {
                match self.screen {
                    Screen::Settings => self.settings.move_cursor(false),
                    Screen::Archive => self.archive.focus = self.archive.focus.prev(),
                    Screen::Targets | Screen::Run => {}
                }
                Vec::new()
            }
            Action::Down => {
                match self.screen {
                    Screen::Settings => self.settings.move_cursor(true),
                    Screen::Targets => {
                        let total = self.targets.filtered_threads(self.form.dm_kind).count();
                        if total > 0 {
                            self.targets.thread_cursor =
                                (self.targets.thread_cursor + 1).min(total - 1);
                        }
                    }
                    Screen::Run => self.run.scroll_log(false),
                    Screen::Archive => self.step_archive(true),
                }
                Vec::new()
            }
            Action::Up => {
                match self.screen {
                    Screen::Settings => self.settings.move_cursor(false),
                    Screen::Targets => {
                        self.targets.thread_cursor = self.targets.thread_cursor.saturating_sub(1);
                    }
                    Screen::Run => self.run.scroll_log(true),
                    Screen::Archive => self.step_archive(false),
                }
                Vec::new()
            }
            Action::CycleNext => {
                if self.screen == Screen::Settings {
                    self.cycle_focused(true);
                } else if self.screen == Screen::Archive {
                    self.archive.cycle_scope(true);
                }
                Vec::new()
            }
            Action::CyclePrev => {
                if self.screen == Screen::Settings {
                    self.cycle_focused(false);
                } else if self.screen == Screen::Archive {
                    self.archive.cycle_scope(false);
                }
                Vec::new()
            }
            Action::Toggle => {
                if self.screen == Screen::Settings {
                    let field = self.settings.field();
                    self.form.toggle(field);
                    self.refresh_errors();
                } else if self.screen == Screen::Targets {
                    if let Some(id) = self.focused_thread_id() {
                        if self.targets.selected_threads.contains(&id) {
                            self.targets.selected_threads.remove(&id);
                        } else {
                            self.targets.selected_threads.insert(id);
                        }
                    }
                }
                Vec::new()
            }
            Action::ToggleProtect => {
                if self.screen == Screen::Targets {
                    let focused = self
                        .targets
                        .filtered_threads(self.form.dm_kind)
                        .nth(self.targets.thread_cursor)
                        .cloned();
                    if let Some(thread) = focused {
                        if self.targets.toggle_protection(&thread) {
                            self.set_toast("protected thread", LogLevel::Success);
                        } else {
                            self.set_toast("unprotected thread", LogLevel::Info);
                        }
                        self.form.protect_users = self.targets.protect_list();
                    }
                }
                Vec::new()
            }
            Action::ToggleDryRun => {
                self.form.dry_run = !self.form.dry_run;
                let state = if self.form.dry_run { "ON" } else { "OFF" };
                let scope = if self.run.is_running() {
                    " for the next run, this one is unchanged"
                } else {
                    ""
                };
                self.set_toast(format!("dry run {state}{scope}"), LogLevel::Info);
                Vec::new()
            }
            Action::StartEdit => {
                if self.screen == Screen::Settings {
                    let field = self.settings.field();
                    if field.is_text() {
                        let val = self.form.get_text(field);
                        self.settings.start_edit(&val);
                    }
                }
                Vec::new()
            }
            Action::CommitEdit => {
                self.commit_edit();
                Vec::new()
            }
            Action::CancelEdit => {
                self.settings.cancel_edit();
                Vec::new()
            }
            Action::Input(c) => {
                if self.overlay == Overlay::Palette {
                    self.palette.query.push(c);
                    self.palette.update_matches();
                } else if self.settings.editing {
                    self.settings.input(c);
                }
                Vec::new()
            }
            Action::InputBackspace => {
                if self.overlay == Overlay::Palette {
                    self.palette.query.pop();
                    self.palette.update_matches();
                } else if self.settings.editing {
                    self.settings.backspace();
                }
                Vec::new()
            }
            Action::InputDelete => {
                if self.settings.editing {
                    self.settings.delete();
                }
                Vec::new()
            }
            Action::InputLeft => {
                if self.settings.editing && self.settings.buffer_cursor > 0 {
                    self.settings.buffer_cursor -= 1;
                }
                Vec::new()
            }
            Action::InputRight => {
                if self.settings.editing
                    && self.settings.buffer_cursor < self.settings.buffer.chars().count()
                {
                    self.settings.buffer_cursor += 1;
                }
                Vec::new()
            }
            Action::InputHome => {
                if self.settings.editing {
                    self.settings.buffer_cursor = 0;
                }
                Vec::new()
            }
            Action::InputEnd => {
                if self.settings.editing {
                    self.settings.buffer_cursor = self.settings.buffer.chars().count();
                }
                Vec::new()
            }
            Action::InputClear => {
                if self.overlay == Overlay::Palette {
                    self.palette.query.clear();
                    self.palette.update_matches();
                } else if self.settings.editing {
                    self.settings.buffer.clear();
                    self.settings.buffer_cursor = 0;
                }
                Vec::new()
            }
            Action::SelectAll => {
                if self.screen == Screen::Targets {
                    let ids: Vec<String> = self
                        .targets
                        .filtered_threads(self.form.dm_kind)
                        .map(|t| t.thread_id.clone())
                        .collect();
                    for id in ids {
                        self.targets.selected_threads.insert(id);
                    }
                }
                Vec::new()
            }
            Action::SelectNone => {
                if self.screen == Screen::Targets {
                    self.targets.selected_threads.clear();
                }
                Vec::new()
            }
            Action::SelectInvert => {
                if self.screen == Screen::Targets {
                    let all_ids: Vec<String> = self
                        .targets
                        .filtered_threads(self.form.dm_kind)
                        .map(|t| t.thread_id.clone())
                        .collect();
                    let mut inverted = HashSet::new();
                    for id in all_ids {
                        if !self.targets.selected_threads.contains(&id) {
                            inverted.insert(id);
                        }
                    }
                    self.targets.selected_threads = inverted;
                }
                Vec::new()
            }
            Action::Activate => {
                if self.screen == Screen::Archive {
                    Vec::new()
                } else {
                    self.request_run()
                }
            }
            Action::StartRun => self.request_run(),
            Action::Confirm => {
                self.overlay = Overlay::None;
                self.start_run_effects()
            }
            Action::Decline => {
                self.overlay = Overlay::None;
                Vec::new()
            }
            Action::Probe => {
                self.connection = Connection::Connecting;
                self.targets.loading = true;
                vec![Effect::Probe]
            }
            Action::ReloadArchive => {
                self.archive.reload();
                self.set_toast("archive reloaded", LogLevel::Info);
                Vec::new()
            }
            Action::Refresh => {
                if self.screen == Screen::Archive {
                    self.apply_action(Action::ReloadArchive)
                } else {
                    self.targets.loading = true;
                    vec![Effect::LoadTargets]
                }
            }
            Action::Yank => {
                if self.screen == Screen::Archive {
                    if let Some(record) = self.archive.records.get(self.archive.record_cursor) {
                        let json = serde_json::to_string_pretty(record).unwrap_or_default();
                        self.set_toast("record copied to clipboard", LogLevel::Success);
                        return vec![Effect::Yank(json)];
                    }
                }
                Vec::new()
            }
            Action::ClearLog => {
                self.run.log.clear();
                self.run.log_scroll = 0;
                Vec::new()
            }
        }
    }

    fn request_run(&mut self) -> Vec<Effect> {
        if self.run.is_running() {
            self.set_toast("a run is already active", LogLevel::Warning);
            return Vec::new();
        }
        self.settings.errors = self.form.validate();
        if !self.settings.errors.is_empty() {
            self.set_toast("fix the errors in Settings first", LogLevel::Error);
            self.screen = Screen::Settings;
            return Vec::new();
        }
        if !self.form.dry_run {
            self.overlay = Overlay::Confirm;
            return Vec::new();
        }
        self.start_run_effects()
    }

    fn step_archive(&mut self, forward: bool) {
        match self.archive.focus {
            ArchiveFocus::Days => self.archive.step_day(forward),
            ArchiveFocus::Channels => self.archive.step_channel(forward),
            ArchiveFocus::Records => self.archive.step_record(forward),
        }
    }

    fn focused_thread_id(&self) -> Option<String> {
        self.targets
            .filtered_threads(self.form.dm_kind)
            .nth(self.targets.thread_cursor)
            .map(|t| t.thread_id.clone())
    }

    fn cycle_focused(&mut self, forward: bool) {
        let field = self.settings.field();
        if forward {
            self.form.cycle_next(field);
        } else {
            self.form.cycle_prev(field);
        }
        self.targets.clamp_cursor(self.form.dm_kind);
        self.refresh_errors();
    }

    fn commit_edit(&mut self) {
        if !self.settings.editing {
            return;
        }
        let field = self.settings.field();
        let val = self.settings.commit_edit();
        self.form.set_text(field, val);
        if field == Field::Protect {
            self.targets.protected_users = crate::config::thread_id_list(&self.form.protect_users)
                .into_iter()
                .collect();
        }
        self.settings.errors = self.form.validate();
    }

    /// An error stays on screen until fixed, so it is rechecked once one is showing.
    fn refresh_errors(&mut self) {
        if !self.settings.errors.is_empty() {
            self.settings.errors = self.form.validate();
        }
    }
}
