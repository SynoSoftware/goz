//! Notification bookkeeping moves through one owned pass. Native controls
//! confirm the resulting ranges; an invalid cache requires a fresh native read.

use super::{RowSelection, RowsChange, ToldRows, push_rows, row_count};
use crate::core::work::Retirement;
use std::collections::{LinkedList, VecDeque};

const PENDING: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CaptureKey {
    incarnation: u64,
    revision: u64,
    count: u64,
}

impl CaptureKey {
    pub(crate) fn same_list(self, other: Self) -> bool {
        self.incarnation == other.incarnation
    }
}

enum Cache {
    Ready(ToldRows),
    Running { key: CaptureKey, retain: bool },
    Invalid,
}

pub(crate) struct Capturing {
    incarnation: u64,
    revision: u64,
    cache: Cache,
    changes: VecDeque<RowsChange>,
    selected: Option<(CaptureKey, RowSelection)>,
    retired: Retirement,
}

impl Default for Capturing {
    fn default() -> Self {
        Self {
            incarnation: 0,
            revision: 0,
            cache: Cache::Ready(ToldRows::default()),
            changes: VecDeque::new(),
            selected: None,
            retired: Retirement::default(),
        }
    }
}

pub(crate) struct Telling {
    key: CaptureKey,
    input: Input,
}

enum Input {
    Changes {
        cache: ToldRows,
        changes: VecDeque<RowsChange>,
    },
    Read {
        chunks: LinkedList<Vec<std::ops::Range<u64>>>,
        items: usize,
    },
    Selected(RowSelection),
}

pub(crate) struct Told {
    key: CaptureKey,
    cache: ToldRows,
    selected: RowSelection,
}

impl Telling {
    pub(crate) fn items(&self) -> usize {
        match &self.input {
            Input::Changes { cache, changes } => cache.runs.len() + changes.len(),
            Input::Read { items, .. } => *items,
            Input::Selected(selected) => selected.positions.len(),
        }
    }

    pub(crate) fn run(self) -> Told {
        let (cache, positions) = match self.input {
            Input::Changes { mut cache, changes } => {
                for change in changes {
                    cache.change(change);
                }
                let positions = cache.rows(self.key.count);
                (cache, positions)
            }
            Input::Read { chunks, .. } => {
                let mut positions = Vec::new();
                for range in chunks.into_iter().flatten() {
                    push_rows(&mut positions, range);
                }
                (ToldRows::read(&positions), positions)
            }
            Input::Selected(selected) => {
                return Told {
                    key: self.key,
                    cache: ToldRows::read(&selected.positions),
                    selected,
                };
            }
        };
        let count = row_count(&positions);
        Told {
            key: self.key,
            cache,
            selected: RowSelection::new(positions, count),
        }
    }
}

impl Capturing {
    pub(crate) fn seed(&mut self, count: u64, selected: RowSelection) -> Telling {
        self.reset();
        self.known(count, selected)
    }

    pub(crate) fn known(&mut self, count: u64, selected: RowSelection) -> Telling {
        self.invalidate();
        let key = self.key(count);
        self.cache = Cache::Running { key, retain: true };
        self.selected = Some((key, selected.clone()));
        Telling {
            key,
            input: Input::Selected(selected),
        }
    }
    pub(crate) fn key(&self, count: u64) -> CaptureKey {
        CaptureKey {
            incarnation: self.incarnation,
            revision: self.revision,
            count,
        }
    }

    pub(crate) fn selected(&self, count: u64) -> Option<RowSelection> {
        let (key, selected) = self.selected.as_ref()?;
        (*key == self.key(count)).then(|| selected.clone())
    }

    pub(crate) fn changed(&mut self, change: RowsChange) {
        self.revision += 1;
        self.forget_selected();
        if matches!(
            self.cache,
            Cache::Invalid | Cache::Running { retain: false, .. }
        ) {
            return;
        }
        if self.changes.len() == PENDING {
            self.invalidate();
        } else {
            self.changes.push_back(change);
        }
    }

    pub(crate) fn reset(&mut self) {
        self.incarnation += 1;
        self.revision += 1;
        self.invalidate();
        self.cache = Cache::Ready(ToldRows::default());
    }

    pub(crate) fn invalidate(&mut self) {
        self.forget_selected();
        let old = std::mem::replace(&mut self.cache, Cache::Invalid);
        match old {
            Cache::Ready(cache) => {
                let items = cache.runs.len();
                self.retired.push(cache, items);
            }
            Cache::Running { key, .. } => self.cache = Cache::Running { key, retain: false },
            Cache::Invalid => {}
        }
        let changes = std::mem::take(&mut self.changes);
        let items = changes.len();
        self.retired.push(changes, items);
    }

    fn forget_selected(&mut self) {
        if let Some((_, selected)) = self.selected.take() {
            let items = selected.positions.len();
            self.retired.push(selected, items);
        }
    }

    pub(crate) fn needs_native(&self) -> bool {
        matches!(self.cache, Cache::Invalid)
    }

    pub(crate) fn take(&mut self, count: u64) -> Option<Telling> {
        if self.selected(count).is_some() {
            return None;
        }
        let key = self.key(count);
        let cache = match std::mem::replace(&mut self.cache, Cache::Running { key, retain: true }) {
            Cache::Ready(cache) => cache,
            other => {
                self.cache = other;
                return None;
            }
        };
        Some(Telling {
            key,
            input: Input::Changes {
                cache,
                changes: std::mem::take(&mut self.changes),
            },
        })
    }

    pub(crate) fn read(
        &mut self,
        key: CaptureKey,
        chunks: LinkedList<Vec<std::ops::Range<u64>>>,
        items: usize,
    ) -> Option<Telling> {
        if self.key(key.count) != key || !self.needs_native() {
            self.retired.push(chunks, items);
            return None;
        }
        self.cache = Cache::Running { key, retain: true };
        Some(Telling {
            key,
            input: Input::Read { chunks, items },
        })
    }

    pub(crate) fn accept(&mut self, answer: Told) -> bool {
        let Cache::Running { key, retain } = self.cache else {
            self.retire_answer(answer);
            return false;
        };
        if key != answer.key {
            self.retire_answer(answer);
            return false;
        }
        if !retain {
            self.cache = Cache::Invalid;
            self.retire_answer(answer);
            return false;
        }
        self.cache = Cache::Ready(answer.cache);
        if key != self.key(key.count) {
            let items = answer.selected.positions.len();
            self.retired.push(answer.selected, items);
            return false;
        }
        self.selected = Some((key, answer.selected));
        true
    }

    fn retire_answer(&mut self, answer: Told) {
        let items = answer.cache.runs.len() + answer.selected.positions.len();
        self.retired.push(answer, items);
    }

    pub(crate) fn take_retired(&mut self) -> Retirement {
        std::mem::take(&mut self.retired)
    }
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
mod tests {
    use super::*;

    #[test]
    fn stale_capture_preserves_new_notifications_and_the_replacements_running_pass() {
        let mut capture = Capturing::default();
        capture.changed(RowsChange::Row {
            row: 1,
            selected: true,
        });
        let first = capture
            .take(8)
            .expect("notification capture is ready")
            .run();
        capture.changed(RowsChange::Row {
            row: 3,
            selected: true,
        });
        assert!(!capture.accept(first));
        assert!(capture.selected(8).is_none());
        let updated = capture
            .take(8)
            .expect("the newer notification was retained")
            .run();
        assert!(capture.accept(updated));
        assert_eq!(
            capture
                .selected(8)
                .expect("current selection")
                .positions
                .as_slice(),
            [1..2, 3..4]
        );

        capture.changed(RowsChange::All { selected: false });
        let old = capture.take(8).expect("the old list starts a pass").run();
        let replacement = capture.seed(8, vec![5..6].into()).run();
        assert!(!capture.accept(old));
        assert!(capture.accept(replacement));
        assert_eq!(
            capture
                .selected(8)
                .expect("replacement selection")
                .positions
                .as_slice(),
            [5..6]
        );
    }
}
