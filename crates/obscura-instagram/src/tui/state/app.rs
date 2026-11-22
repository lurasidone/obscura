use std::time::Instant;

use crate::cleaner::TargetResolution;
use crate::config::Config;
use crate::events::{LogLevel, Phase, RunStats};
use crate::history::HistoryStore;
use crate::model::DirectThread;

use super::super::action::{Effect, Overlay, Screen};
use super::super::keys::InputMode;
use super::{
    ArchiveState, Connection, Form, PaletteState, RunState, RunStatus, SettingsState, TargetsState,
    Toast,
};

#[derive(Debug)]
pub struct AppState {
    pub screen: Screen,
    pub overlay: Overlay,
    pub connection: Connection,
    pub identity: Option<String>,
    pub toast: Option<Toast>,
    pub form: Form,
    pub settings: SettingsState,
    pub targets: TargetsState,
    pub run: RunState,
    pub archive: ArchiveState,
    pub palette: PaletteState,
    pub config: Config,
}

impl AppState {
    pub fn new(cfg: Config) -> Self {
        let form = Form::from_config(&cfg);
        let mut archive = ArchiveState {
            store: HistoryStore::with_root(&cfg.history_dir),
            ..ArchiveState::default()
        };
        archive.reload();

        let mut targets = TargetsState::default();
        for u in &cfg.protect_users {
            targets.protected_users.insert(u.clone());
        }

        Self {
            screen: Screen::Settings,
            overlay: Overlay::None,
            connection: Connection::Idle,
            identity: None,
            toast: None,
            form,
            settings: SettingsState::default(),
            targets,
            run: RunState::default(),
            archive,
            palette: PaletteState::default(),
            config: cfg,
        }
    }

    /// True when requests being sent right now change nothing, or would not.
    pub fn header_dry_run(&self) -> bool {
        if self.run.is_running() {
            self.run.dry_run
        } else {
            self.form.dry_run
        }
    }

    /// What the Run screen is showing: the last run, or the next one before any.
    pub fn shown_dry_run(&self) -> bool {
        if self.run.status == RunStatus::Idle {
            self.form.dry_run
        } else {
            self.run.dry_run
        }
    }

    pub fn input_mode(&self) -> InputMode {
        match self.overlay {
            Overlay::Help => InputMode::Help,
            Overlay::Palette => InputMode::Palette,
            Overlay::Confirm => InputMode::Confirm,
            Overlay::None => {
                if self.settings.editing {
                    InputMode::Text
                } else {
                    InputMode::Normal
                }
            }
        }
    }

    pub fn set_toast(&mut self, text: impl Into<String>, level: LogLevel) {
        self.toast = Some(Toast::new(text, level));
    }

    pub fn picked_threads_len(&self) -> usize {
        self.targets
            .threads
            .iter()
            .filter(|t| self.targets.selected_threads.contains(&t.thread_id))
            .count()
    }

    /// Picks the run will actually touch: the conversation type and Protected drop the rest.
    pub fn picks_in_scope_len(&self) -> usize {
        self.targets
            .filtered_threads(self.form.dm_kind)
            .filter(|t| self.targets.selected_threads.contains(&t.thread_id))
            .filter(|t| !self.targets.is_protected(t))
            .count()
    }

    // picks narrow DMs and reactions only; typed thread IDs always win
    pub fn has_picker_override(&self) -> bool {
        self.form.content.touches_threads()
            && crate::config::thread_id_list(&self.form.threads).is_empty()
            && self.picked_threads_len() > 0
    }

    fn picked_threads(&self) -> Vec<DirectThread> {
        self.targets
            .threads
            .iter()
            .filter(|t| self.targets.selected_threads.contains(&t.thread_id))
            .cloned()
            .collect()
    }

    fn picker_override(&self) -> Option<Vec<DirectThread>> {
        if self.has_picker_override() {
            Some(self.picked_threads())
        } else {
            None
        }
    }

    pub(super) fn start_run_effects(&mut self) -> Vec<Effect> {
        if self.run.engine_active {
            self.set_toast("the last run is still stopping", LogLevel::Warning);
            return Vec::new();
        }
        match self.form.to_cleaner_config() {
            Ok(mut cleaner_cfg) => {
                self.run.log.clear();
                self.run.log_scroll = 0;
                if let Some(picked) = self.picker_override() {
                    let count = picked.len();
                    cleaner_cfg
                        .targets
                        .retain(|target| !matches!(target, TargetResolution::AllDms));
                    cleaner_cfg
                        .targets
                        .push(TargetResolution::ThreadObjects(picked));
                    self.run.push_log(
                        LogLevel::Info,
                        format!("DMs limited to {count} picked thread(s)"),
                    );
                }
                self.run.status = RunStatus::Running;
                self.run.engine_active = true;
                self.run.phase = Phase::Scan;
                self.run.dry_run = cleaner_cfg.dry_run;
                self.run.cancelling = false;
                self.run.verifying = false;
                self.run.started_at = Some(Instant::now());
                self.run.finished_at = None;
                self.run.found = 0;
                self.run.plan_total = 0;
                self.run.plan_done = 0;
                self.run.warnings = 0;
                self.run.rate_limiter = Default::default();
                self.run.sparkline_data.clear();
                self.run.stats = RunStats::default();
                self.run.channels.clear();
                self.run.current_progress = None;
                self.run.log_scroll = 0;

                self.run.push_log(
                    LogLevel::Info,
                    format!("selected: {}", self.form.content.label()),
                );
                self.screen = Screen::Run;
                vec![Effect::StartRun(cleaner_cfg)]
            }
            Err(e) => {
                self.set_toast(format!("settings error: {e}"), LogLevel::Error);
                Vec::new()
            }
        }
    }
}
