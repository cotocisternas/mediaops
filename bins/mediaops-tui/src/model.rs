//! UI navigation state. Mutations are a closed enum.

use crate::cache::ObjectKey;

pub const MIN_COLS: u16 = 60;
pub const MIN_ROWS: u16 = 16;
pub const SPLIT_COLS: u16 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Overview,
    Wants,
    Jobs,
    Holds,
    Titles,
    Nodes,
    BoxListing,
}

impl Screen {
    pub const ALL: [Screen; 7] = [
        Self::Overview,
        Self::Wants,
        Self::Jobs,
        Self::Holds,
        Self::Titles,
        Self::Nodes,
        Self::BoxListing,
    ];

    pub fn from_digit(n: u8) -> Option<Self> {
        Self::ALL.get((n.saturating_sub(1)) as usize).copied()
    }

    pub fn number(self) -> u8 {
        match self {
            Self::Overview => 1,
            Self::Wants => 2,
            Self::Jobs => 3,
            Self::Holds => 4,
            Self::Titles => 5,
            Self::Nodes => 6,
            Self::BoxListing => 7,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::Wants => "Wants",
            Self::Jobs => "Jobs",
            Self::Holds => "Holds",
            Self::Titles => "Titles",
            Self::Nodes => "Nodes",
            Self::BoxListing => "Box",
        }
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.number() as usize) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = (self.number() as usize + Self::ALL.len() - 2) % Self::ALL.len();
        Self::ALL[i]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    Connecting,
    Synchronizing,
    Current,
    Stale,
}

impl SyncState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "reconnecting",
            Self::Synchronizing => "Synchronizing",
            Self::Current => "Current",
            Self::Stale => "NOT CURRENT",
        }
    }

    pub fn writes_allowed(self) -> bool {
        matches!(self, Self::Current)
    }
}

#[derive(Debug, Clone)]
pub struct UiModel {
    pub screen: Screen,
    pub in_detail: bool,
    pub help: bool,
    pub selected: usize,
    pub table_offset: usize,
    pub detail_offset: u16,
    pub cols: u16,
    pub rows: u16,
    pub mutation_pending: bool,
    pub message: Option<String>,
    pub selected_key: Option<ObjectKey>,
    pub selected_uid: Option<String>,
    pub selected_rv: Option<i64>,
    pub identity_clipped: bool,
    pub rendered_target: Option<crate::actions::MutationTarget>,
}

impl Default for UiModel {
    fn default() -> Self {
        Self {
            screen: Screen::Overview,
            in_detail: false,
            help: false,
            selected: 0,
            table_offset: 0,
            detail_offset: 0,
            cols: 80,
            rows: 24,
            mutation_pending: false,
            message: None,
            selected_key: None,
            selected_uid: None,
            selected_rv: None,
            identity_clipped: false,
            rendered_target: None,
        }
    }
}

impl UiModel {
    pub fn undersize(&self) -> bool {
        self.cols < MIN_COLS || self.rows < MIN_ROWS
    }

    pub fn split_detail(&self) -> bool {
        self.cols >= SPLIT_COLS && !self.undersize()
    }

    pub fn mutations_enabled(&self, sync: SyncState) -> bool {
        sync.writes_allowed()
            && self.in_detail
            && !self.help
            && !self.undersize()
            && !self.identity_clipped
            && !self.mutation_pending
            && self.selected_key.is_some()
    }

    pub fn clear_action_selection(&mut self) {
        self.in_detail = false;
        self.detail_offset = 0;
        self.rendered_target = None;
    }

    pub fn select_row(&mut self, index: usize, len: usize) {
        self.selected_key = None;
        self.selected_uid = None;
        self.selected_rv = None;
        self.rendered_target = None;
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = index.min(len - 1);
        self.detail_offset = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digits_and_tabs_cover_seven_screens() {
        assert_eq!(Screen::from_digit(1), Some(Screen::Overview));
        assert_eq!(Screen::from_digit(7), Some(Screen::BoxListing));
        assert_eq!(Screen::Wants.next(), Screen::Jobs);
        assert_eq!(Screen::Overview.prev(), Screen::BoxListing);
    }

    #[test]
    fn mutations_require_current_detail_and_full_identity() {
        let mut ui = UiModel {
            in_detail: true,
            cols: 80,
            rows: 24,
            selected_key: Some(ObjectKey::new(mediaops_core::Kind::Want, "movie:tmdb:1")),
            ..UiModel::default()
        };
        assert!(ui.mutations_enabled(SyncState::Current));
        ui.identity_clipped = true;
        assert!(!ui.mutations_enabled(SyncState::Current));
        ui.identity_clipped = false;
        assert!(!ui.mutations_enabled(SyncState::Stale));
        ui.cols = 40;
        assert!(ui.undersize());
    }
}
