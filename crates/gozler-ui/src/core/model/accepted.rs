//! The values and ordered actions of one accepted transition. The window
//! applies structure before values and delivers actions after them.

use super::{Column, DaemonFailure, ExportError, Fetch, FixWork, List, PathBuf, Purpose, Side};
use crate::core::fetch::ChunkAsk;
use crate::core::inspector::AgeTimer;
use crate::core::search_list::Deselect;
use gozler_fileops::OperationId;
use std::ops::{BitOr, BitOrAssign, Range};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Areas(u16);

impl Areas {
    pub(crate) const NONE: Self = Self(0);
    pub(crate) const TITLE: Self = Self(1);
    pub(crate) const SEARCH: Self = Self(1 << 1);
    pub(crate) const FILTERS: Self = Self(1 << 2);
    pub(crate) const LOCATIONS: Self = Self(1 << 3);
    pub(crate) const INSPECTOR: Self = Self(1 << 4);
    pub(crate) const STATUS: Self = Self(1 << 5);
    pub(crate) const LEFT_PANEL: Self = Self(1 << 6);
    pub(crate) const RIGHT_PANEL: Self = Self(1 << 7);
    pub(crate) const KEY_BAR: Self = Self(1 << 8);

    pub(crate) fn contains(self, area: Self) -> bool {
        self.0 & area.0 == area.0
    }

    pub(crate) fn panel(side: Side) -> Self {
        match side {
            Side::Left => Self::LEFT_PANEL,
            Side::Right => Self::RIGHT_PANEL,
        }
    }

    pub(crate) fn list(list: List) -> Self {
        match list {
            List::Search => Self::SEARCH | Self::INSPECTOR,
            List::Panel(side) => Self::panel(side),
        }
    }
}

impl BitOr for Areas {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl BitOrAssign for Areas {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Accepted {
    pub(crate) touched: Areas,
    pub(crate) effects: Vec<Effect>,
}

#[derive(Debug, PartialEq)]
pub(crate) enum Effect {
    ShowColumn {
        index: usize,
        column: Column,
    },
    HideColumn {
        index: usize,
        column: Column,
    },
    ClearSelection(List),
    SetCount {
        list: List,
        count: u64,
    },
    Rows {
        list: List,
        rows: Range<u64>,
        count: u64,
    },
    Redraw {
        list: List,
        rows: Range<u64>,
    },
    Deselect {
        list: List,
        rows: Deselect,
    },
    PreparePanel(Side),
    Layout,
    Fetch(Fetch),
    SearchAgain(Vec<List>),
    SearchSoon(List),
    NameSearchSoon(List),
    ReadSelectionSoon(List),
    Cancel(Purpose),
    AskLatestRelease,
    RunFix {
        generation: super::Generation,
        work: FixWork,
    },
    ConfirmSetup,
    ExplainNoSetup,
    StopFix,
    CancelOperation(OperationId),
    RunSetup(PathBuf),
    FirstRow {
        list: List,
        generation: super::Generation,
    },
    ReleaseOperations,
    FetchVisible(List),
    FillFirstRows(List),
    EnterFirst(ChunkAsk),
    ReadMemory,
    PostSettings,
    TotalCommander(Result<Option<String>, DaemonFailure>),
    FolderSizes(Side),
    ExportFailed {
        path: PathBuf,
        error: ExportError,
    },
    Menu,
    Tray,
    SwitchPanels,
    HomeFocus,
    PaneFocus,
    InspectAge,
    Age(AgeTimer),
    Polling,
    NoteSavedState,
    SaveSoon,
    ListPasses,
}

impl Effect {
    pub(crate) fn structural(&self) -> bool {
        matches!(
            self,
            Self::ShowColumn { .. }
                | Self::HideColumn { .. }
                | Self::ClearSelection(_)
                | Self::SetCount { .. }
                | Self::Rows { .. }
                | Self::Redraw { .. }
                | Self::Deselect { .. }
                | Self::PreparePanel(_)
                | Self::Layout
                | Self::ReadMemory
        )
    }

    pub(crate) fn committed(&self) -> bool {
        matches!(self, Self::CancelOperation(_))
            || matches!(self, Self::Fetch(fetch) if fetch.purpose() == Purpose::Export)
    }
}

/// A nested update supersedes presentation work, but cannot retract a file command.
pub(crate) fn may_apply(expected: u64, current: u64, effect: &Effect) -> bool {
    expected == current || effect.committed()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_nested_update_supersedes_presentation_but_keeps_a_file_command_once() {
        let effects = [
            Effect::HomeFocus,
            Effect::CancelOperation(OperationId::for_tests(7)),
        ];
        let mut applied = Vec::new();
        for effect in effects {
            if may_apply(1, 2, &effect) {
                applied.push(effect);
            }
        }
        assert_eq!(
            applied,
            vec![Effect::CancelOperation(OperationId::for_tests(7))]
        );
    }
}
