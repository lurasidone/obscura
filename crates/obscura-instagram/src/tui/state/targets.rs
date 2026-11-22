use std::collections::HashSet;

use crate::config::DmKind;
use crate::model::DirectThread;

#[derive(Debug, Clone, Default)]
pub struct TargetsState {
    pub threads: Vec<DirectThread>,
    pub selected_threads: HashSet<String>,
    pub protected_users: HashSet<String>,
    pub thread_cursor: usize,
    pub loading: bool,
}

impl TargetsState {
    pub fn filtered_threads(&self, kind: DmKind) -> impl Iterator<Item = &DirectThread> {
        self.threads
            .iter()
            .filter(move |t| kind.matches(t.is_group))
    }

    /// Reads an entry the way the run does, so the list never disagrees with it.
    fn guards(entry: &str, thread: &DirectThread) -> bool {
        let entry = entry.trim_start_matches('@');
        entry == thread.id()
            || thread
                .users
                .iter()
                .any(|u| entry == u.pk || entry.eq_ignore_ascii_case(&u.username))
    }

    pub fn is_protected(&self, thread: &DirectThread) -> bool {
        self.protected_users
            .iter()
            .any(|entry| Self::guards(entry, thread))
    }

    /// Returns whether the thread ends up protected.
    pub fn toggle_protection(&mut self, thread: &DirectThread) -> bool {
        if self.is_protected(thread) {
            self.protected_users
                .retain(|entry| !Self::guards(entry, thread));
            false
        } else {
            self.protected_users.insert(thread.id());
            true
        }
    }

    pub fn protect_list(&self) -> String {
        let mut entries: Vec<&str> = self.protected_users.iter().map(String::as_str).collect();
        entries.sort_unstable();
        entries.join(", ")
    }

    pub fn clamp_cursor(&mut self, kind: DmKind) {
        let total = self.filtered_threads(kind).count();
        self.thread_cursor = self.thread_cursor.min(total.saturating_sub(1));
    }
}
