use chrono::NaiveDate;
pub use obscura_core::tui::ArchiveFocus;

use crate::history::{ChannelRef, DeletedRecord, HistoryScope, HistoryStore};

fn step(cursor: usize, len: usize, forward: bool) -> usize {
    if forward {
        (cursor + 1).min(len.saturating_sub(1))
    } else {
        cursor.saturating_sub(1)
    }
}

#[derive(Debug, Clone)]
pub struct ArchiveState {
    pub store: HistoryStore,
    pub scope: HistoryScope,
    pub focus: ArchiveFocus,
    pub days: Vec<NaiveDate>,
    pub day_cursor: usize,
    pub channels: Vec<ChannelRef>,
    pub channel_cursor: usize,
    pub records: Vec<DeletedRecord>,
    pub record_cursor: usize,
}

impl Default for ArchiveState {
    fn default() -> Self {
        let store = HistoryStore::with_root("./history");
        let days = store.list_days(HistoryScope::All).unwrap_or_default();
        let channels = days
            .first()
            .and_then(|d| store.list_channels(*d, HistoryScope::All).ok())
            .unwrap_or_default();
        let records = match (days.first(), channels.first()) {
            (Some(d), Some(c)) => store
                .load_day(*d, HistoryScope::All, Some(&c.id))
                .unwrap_or_default(),
            (Some(d), None) => store
                .load_day(*d, HistoryScope::All, None)
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        Self {
            store,
            scope: HistoryScope::All,
            focus: ArchiveFocus::Days,
            days,
            day_cursor: 0,
            channels,
            channel_cursor: 0,
            records,
            record_cursor: 0,
        }
    }
}

impl ArchiveState {
    /// Stays on the selected day and target if they still exist: a new day appearing
    /// at the top of the list must not move the selection to another one.
    pub fn reload(&mut self) {
        let day = self.days.get(self.day_cursor).copied();
        let target = self
            .channels
            .get(self.channel_cursor)
            .map(|channel| channel.id.clone());

        self.days = self.store.list_days(self.scope).unwrap_or_default();
        self.day_cursor = day
            .and_then(|day| self.days.iter().position(|d| *d == day))
            .unwrap_or_else(|| self.day_cursor.min(self.days.len().saturating_sub(1)));
        self.read_channels();
        self.channel_cursor = target
            .and_then(|id| self.channels.iter().position(|c| c.id == id))
            .unwrap_or_else(|| {
                self.channel_cursor
                    .min(self.channels.len().saturating_sub(1))
            });
        self.reload_records();
    }

    fn read_channels(&mut self) {
        self.channels = match self.days.get(self.day_cursor) {
            Some(&day) => self
                .store
                .list_channels(day, self.scope)
                .unwrap_or_default(),
            None => Vec::new(),
        };
    }

    /// Another day holds other targets, so the selection starts over.
    pub fn step_day(&mut self, forward: bool) {
        self.day_cursor = step(self.day_cursor, self.days.len(), forward);
        self.channel_cursor = 0;
        self.record_cursor = 0;
        self.read_channels();
        self.reload_records();
    }

    /// Another target holds other records, so the selection starts over.
    pub fn step_channel(&mut self, forward: bool) {
        self.channel_cursor = step(self.channel_cursor, self.channels.len(), forward);
        self.record_cursor = 0;
        self.reload_records();
    }

    pub fn step_record(&mut self, forward: bool) {
        self.record_cursor = step(self.record_cursor, self.records.len(), forward);
    }

    pub fn reload_records(&mut self) {
        if let Some(&day) = self.days.get(self.day_cursor) {
            if let Some(ch) = self.channels.get(self.channel_cursor) {
                self.records = self
                    .store
                    .load_day(day, self.scope, Some(&ch.id))
                    .unwrap_or_default();
            } else {
                self.records = self
                    .store
                    .load_day(day, self.scope, None)
                    .unwrap_or_default();
            }
        } else {
            self.records.clear();
        }
        self.record_cursor = self.record_cursor.min(self.records.len().saturating_sub(1));
    }

    pub fn cycle_scope(&mut self, forward: bool) {
        self.scope = if forward {
            self.scope.next()
        } else {
            self.scope.prev()
        };
        self.reload();
    }
}
