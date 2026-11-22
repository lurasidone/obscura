use std::collections::HashSet;

use crate::config::DmKind;
use crate::model::Channel;

#[derive(Debug, Clone, Default)]
pub struct TargetsState {
    pub threads: Vec<Channel>,
    pub selected_threads: HashSet<String>,
    pub protected_users: HashSet<String>,
    pub thread_cursor: usize,
    pub loading: bool,
}

impl TargetsState {
    pub fn filtered_threads(&self, kind: DmKind) -> impl Iterator<Item = &Channel> {
        self.threads
            .iter()
            .filter(move |t| kind.matches(t.is_group_dm()))
    }

    pub fn is_protected(&self, thread: &Channel) -> bool {
        if self.protected_users.contains(&thread.id) {
            return true;
        }
        thread.recipients.iter().any(|u| {
            self.protected_users.contains(&u.id) || self.protected_users.contains(&u.username)
        })
    }
}
