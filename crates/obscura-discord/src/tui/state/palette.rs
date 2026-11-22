use super::super::action;

#[derive(Debug, Clone)]
pub struct PaletteState {
    pub query: String,
    pub matches: Vec<usize>,
    pub cursor: usize,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            query: String::new(),
            matches: (0..action::commands().len()).collect(),
            cursor: 0,
        }
    }
}

impl PaletteState {
    pub fn update_matches(&mut self) {
        self.matches = obscura_core::tui::widgets::palette_matches(
            &self.query,
            action::commands()
                .iter()
                .enumerate()
                .map(|(i, command)| (i, command.id, command.label, command.hint)),
        );
        self.cursor = self.cursor.min(self.matches.len().saturating_sub(1));
    }
}
