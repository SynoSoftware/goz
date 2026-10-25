//! Bounded selection writes: retain the intersection, using a bulk change when
//! every row changes or clearing is cheaper than any possible difference.

use super::RowSelection;
use std::ops::Range;
use std::sync::Arc;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Write {
    All(bool),
    Row(u64, bool),
}

#[derive(Clone, Copy)]
enum Phase {
    Clear,
    All,
    Parent,
    Remove,
    Add { cleared: bool },
    Done,
}

pub(crate) struct Changes {
    shown: Arc<Vec<Range<u64>>>,
    wanted: Arc<Vec<Range<u64>>>,
    count: u64,
    phase: Phase,
    source: usize,
    other: usize,
    row: Option<u64>,
}

impl Changes {
    /// Ranges are sorted and disjoint. `shown` is unknown after an interrupted
    /// update; `selected_count` is the control's count after resizing it.
    pub(crate) fn new(
        shown: Option<Arc<Vec<Range<u64>>>>,
        wanted: &RowSelection,
        count: u64,
        selected_count: u64,
    ) -> Self {
        let phase = if shown
            .as_ref()
            .is_some_and(|shown| Arc::ptr_eq(shown, &wanted.positions))
        {
            Phase::Done
        } else if matches!(wanted.positions.as_slice(), [all] if all.end == count && all.start < 2)
        {
            Phase::All
        } else if shown.is_none()
            || wanted.count == 0
            || selected_count > wanted.count.saturating_mul(2).saturating_add(1)
        {
            Phase::Clear
        } else {
            Phase::Remove
        };
        Self {
            shown: shown.unwrap_or_default(),
            wanted: Arc::clone(&wanted.positions),
            count,
            phase,
            source: 0,
            other: 0,
            row: None,
        }
    }

    pub(crate) fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// Consumes budget for both writes and range comparisons. `None` can mean
    /// that the budget ended while skipping an unchanged intersection.
    pub(crate) fn next(&mut self, budget: &mut usize) -> Option<Write> {
        while *budget > 0 && !self.is_done() {
            *budget -= 1;
            let removing = match self.phase {
                Phase::Clear => {
                    self.phase = Phase::Add { cleared: true };
                    return Some(Write::All(false));
                }
                Phase::All => {
                    self.phase = if self.wanted[0].start == 1 {
                        Phase::Parent
                    } else {
                        Phase::Done
                    };
                    return Some(Write::All(true));
                }
                Phase::Parent => {
                    self.phase = Phase::Done;
                    return Some(Write::Row(0, false));
                }
                Phase::Remove => true,
                Phase::Add { .. } => false,
                Phase::Done => unreachable!("done ends the loop"),
            };
            let (source, other) = if removing {
                (&self.shown, self.wanted.as_slice())
            } else {
                let other = if matches!(self.phase, Phase::Add { cleared: true }) {
                    &[][..]
                } else {
                    self.shown.as_slice()
                };
                (&self.wanted, other)
            };
            let Some(range) = source
                .get(self.source)
                .filter(|range| range.start < self.count)
            else {
                self.phase = if removing {
                    Phase::Add { cleared: false }
                } else {
                    Phase::Done
                };
                self.source = 0;
                self.other = 0;
                self.row = None;
                continue;
            };
            let row = self.row.unwrap_or(range.start);
            let end = range.end.min(self.count);
            if row >= end {
                self.source += 1;
                self.row = None;
                continue;
            }
            if let Some(overlap) = other.get(self.other) {
                if overlap.end <= row {
                    self.other += 1;
                    continue;
                }
                if overlap.start <= row {
                    self.row = Some(overlap.end.min(end));
                    continue;
                }
            }
            self.row = Some(row + 1);
            return Some(Write::Row(row, !removing));
        }
        None
    }
}

#[cfg(test)]
#[allow(
    clippy::single_range_in_vec_init,
    reason = "a selection is a list of row ranges"
)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn shifting_a_large_selection_writes_only_its_boundaries() {
        let shown = RowSelection::from(vec![10_000..60_000]);
        let wanted = RowSelection::from(vec![10_001..60_001]);
        let mut changes = Changes::new(Some(shown.positions), &wanted, 100_000, 50_000);
        let mut budget = 16;
        assert_eq!(changes.next(&mut budget), Some(Write::Row(10_000, false)));
        assert_eq!(changes.next(&mut budget), Some(Write::Row(60_000, true)));
        assert_eq!(changes.next(&mut budget), None);
        assert!(changes.is_done());
    }

    #[test]
    fn unchanged_fragments_also_use_the_turn_budget() {
        let ranges: Vec<_> = (0..100).map(|row| row * 2..row * 2 + 1).collect();
        let wanted = RowSelection::from(ranges.clone());
        let mut changes = Changes::new(Some(Arc::new(ranges)), &wanted, 200, 100);
        let mut budget = 3;
        assert_eq!(changes.next(&mut budget), None);
        assert_eq!(budget, 0);
        assert!(!changes.is_done());
    }

    proptest! {
        #[test]
        fn bounded_writes_preserve_exactly_the_wanted_rows(
            shown in proptest::collection::vec(any::<bool>(), 0..64),
            wanted in proptest::collection::vec(any::<bool>(), 0..64),
            count in 0..64usize,
            known in any::<bool>(),
        ) {
            let selection = |bits: &[bool]| RowSelection::from(
                bits.iter().enumerate().filter(|(_, selected)| **selected)
                    .map(|(row, _)| row as u64..row as u64 + 1).collect::<Vec<_>>()
            );
            let before = selection(&shown);
            let wanted = selection(&wanted[..wanted.len().min(count)]);
            let mut native: Vec<_> = (0..count).map(|row| shown.get(row) == Some(&true)).collect();
            let selected_count = native.iter().filter(|selected| **selected).count() as u64;
            let mut changes = Changes::new(known.then_some(before.positions), &wanted, count as u64, selected_count);
            let mut turns = 0;
            while !changes.is_done() {
                let mut budget = 3;
                while let Some(write) = changes.next(&mut budget) {
                    match write {
                        Write::All(selected) => native.fill(selected),
                        Write::Row(row, selected) => native[row as usize] = selected,
                    }
                }
                turns += 1;
                prop_assert!(turns < 256);
            }
            let actual: Vec<_> = native.iter().enumerate().filter(|(_, selected)| **selected)
                .map(|(row, _)| row as u64).collect();
            let expected: Vec<_> = wanted.positions.iter().cloned().flatten().collect();
            prop_assert_eq!(actual, expected);
        }
    }
}
