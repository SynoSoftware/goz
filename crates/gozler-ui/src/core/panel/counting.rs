//! Counts one accepted row snapshot and its selection without inspecting controls.

use super::*;

#[derive(Debug)]
pub(crate) struct Counting {
    rows: Arc<FolderRows>,
    selected: Arc<Vec<Range<u64>>>,
    selected_count: u64,
    up: u64,
}

pub(crate) struct Counted {
    input: Counting,
    pub(super) counts: Counts,
}

impl Counting {
    pub(super) fn new(panel: &Panel) -> Self {
        Self {
            rows: Arc::clone(&panel.rows),
            selected: Arc::clone(&panel.selected),
            selected_count: panel.selected_count,
            up: panel.up_rows(),
        }
    }

    pub(crate) fn items(&self) -> usize {
        self.rows.items().saturating_add(self.selected.len())
    }

    pub(super) fn cost(&self) -> usize {
        if self.selected.is_empty() {
            self.rows.counting_cost()
        } else {
            usize::try_from(self.selected_count)
                .unwrap_or(usize::MAX)
                .saturating_add(self.selected.len())
        }
    }

    pub(crate) fn run(self) -> Counted {
        let items = self.rows.shown_len() as u64;
        let mut picked = 0;
        let chosen = Totals::of(
            self.selected
                .iter()
                .flat_map(Clone::clone)
                .filter_map(|row| self.rows.entry(row, self.up))
                .inspect(|_| picked += 1),
        );
        let (selected, size) = if picked == 0 {
            let size = self.rows.shown_totals();
            (None, (items > 0).then_some(size))
        } else {
            (Some(picked), Some(chosen))
        };
        Counted {
            input: self,
            counts: Counts {
                items,
                selected,
                size,
            },
        }
    }
}

impl Counted {
    pub(super) fn accept(self, panel: &mut Panel) -> bool {
        let current = Arc::ptr_eq(&self.input.rows, &panel.rows)
            && Arc::ptr_eq(&self.input.selected, &panel.selected)
            && self.input.up == panel.up_rows();
        if current {
            panel.counted = CountState::Ready(self.counts);
        }
        let items = self.input.items();
        panel.retired.push(self, items);
        current
    }
}
