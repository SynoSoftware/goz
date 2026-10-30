use super::*;
use crate::core::selection::push_rows;
use gozler_core::query::{MaskScratch, NameMask};
use gozler_core::wtf8;

pub(crate) struct Selecting {
    rows: Arc<FolderRows>,
    selected: Arc<Vec<Range<u64>>>,
    epoch: u64,
    revision: u64,
    up: u64,
    mask: NameMask,
    select: bool,
    folders: bool,
}

pub(crate) struct Selected {
    input: Selecting,
    selection: RowSelection,
}

impl Selecting {
    pub(super) fn new(panel: &Panel, mask: NameMask, select: bool, folders: bool) -> Self {
        Self {
            rows: Arc::clone(&panel.rows),
            selected: Arc::clone(&panel.selected),
            epoch: panel.epoch,
            revision: panel.interaction_revision,
            up: panel.up_rows(),
            mask,
            select,
            folders,
        }
    }

    pub(crate) fn items(&self) -> usize {
        self.rows.items().saturating_add(self.selected.len())
    }

    pub(crate) fn run(self) -> Selected {
        let mut scratch = MaskScratch::default();
        let mut name = Vec::new();
        let mut was = self.selected.iter().peekable();
        let mut selected = Vec::new();
        for row in 0..self.rows.count(self.up) {
            while was.next_if(|rows| rows.end <= row).is_some() {}
            let mut is = was.peek().is_some_and(|rows| rows.contains(&row));
            if let Some(entry) = self.rows.entry(row, self.up)
                && (self.folders || !entry.is_folder())
            {
                name.clear();
                wtf8::from_utf16(&entry.name, &mut name);
                if self.mask.matches(&name, &mut scratch) {
                    is = self.select;
                }
            }
            if is {
                push_rows(&mut selected, row..row + 1);
            }
        }
        Selected {
            input: self,
            selection: selected.into(),
        }
    }
}

impl Selected {
    pub(super) fn accept(self, panel: &mut Panel) -> bool {
        let current = matches!(panel.view, View::Folder)
            && !panel.rows_pending()
            && self.input.epoch == panel.epoch
            && self.input.revision == panel.interaction_revision
            && Arc::ptr_eq(&self.input.rows, &panel.rows);
        let items = self.input.items();
        if current {
            panel.select_shared(self.selection);
            panel.quick.invalidate();
        } else {
            let count = self.selection.positions.len();
            panel.retired.push(self.selection, count);
        }
        panel.retired.push(self.input, items);
        current
    }
}
