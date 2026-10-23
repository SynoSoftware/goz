//! Queue-wide work counters. Finished contributions stay until the next queue
//! starts; retained reports never enter a fresh denominator.

use super::{Bar, Measure, is_transfer, measure_of, moves_bytes};
use gozler_fileops::{End, Operation, OperationId, Request, State};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unit {
    Bytes,
    Items,
}

#[derive(Clone, Copy, Debug)]
struct Contribution {
    unit: Unit,
    measure: Measure,
}

#[derive(Debug, Default)]
pub(crate) struct Batch {
    work: HashMap<OperationId, Option<Contribution>>,
    seen: HashSet<OperationId>,
    active: bool,
}

impl Batch {
    pub(crate) fn contains(&self, id: OperationId) -> bool {
        self.work.contains_key(&id)
    }
    pub(crate) fn record(&mut self, snapshot: &[Operation]) {
        let transfers: Vec<_> = snapshot
            .iter()
            .filter(|op| is_transfer(&op.request))
            .collect();
        let new: HashSet<_> = transfers
            .iter()
            .filter(|op| !self.seen.contains(&op.id) && !matches!(op.state, State::Interrupted))
            .map(|op| op.id)
            .collect();
        if !self.active && !new.is_empty() {
            self.work.clear();
        }
        self.active = transfers
            .iter()
            .any(|op| !matches!(op.state, State::Ended(_) | State::Interrupted));
        for op in &transfers {
            if !self.work.contains_key(&op.id) && !new.contains(&op.id) {
                continue;
            }
            match &op.state {
                State::Interrupted => {}
                State::Ended(outcome) => {
                    let work = self.work.entry(op.id).or_insert(None);
                    if work.is_none() {
                        let bytes = outcome.bytes > 0
                            && !matches!(*op.request, Request::Synchronize { .. });
                        let done = if bytes { outcome.bytes } else { outcome.items };
                        *work = Some(Contribution {
                            unit: if bytes { Unit::Bytes } else { Unit::Items },
                            measure: Measure { done, total: done },
                        });
                    }
                    if let Some(contribution) = work {
                        if outcome.end == End::Cancelled {
                            contribution.measure.total = contribution.measure.done;
                        } else {
                            contribution.measure.done = contribution.measure.total;
                        }
                    }
                }
                State::Running(running) => {
                    self.work.insert(
                        op.id,
                        measure_of(running).map(|measure| Contribution {
                            unit: if moves_bytes(running) {
                                Unit::Bytes
                            } else {
                                Unit::Items
                            },
                            measure,
                        }),
                    );
                }
                State::Preparing
                | State::Prepared(_)
                | State::Compared(_)
                | State::Waiting { .. }
                | State::Cancelling => {
                    let total = match &op.state {
                        State::Prepared(p) => p.total,
                        State::Waiting { total, .. } => *total,
                        _ => None,
                    };
                    let contribution = self.work.entry(op.id).or_insert(None);
                    if let Some(total) = total.filter(|total| {
                        total.bytes == 0
                            || matches!(
                                *op.request,
                                Request::Copy { .. } | Request::Synchronize { .. }
                            )
                    }) {
                        let bytes = total.bytes > 0 && matches!(*op.request, Request::Copy { .. });
                        contribution.get_or_insert(Contribution {
                            unit: if bytes { Unit::Bytes } else { Unit::Items },
                            measure: Measure {
                                done: 0,
                                total: if bytes { total.bytes } else { total.items },
                            },
                        });
                    }
                }
            }
        }
        self.seen = transfers
            .iter()
            .filter(|op| !matches!(op.state, State::Interrupted))
            .map(|op| op.id)
            .collect();
    }

    pub(crate) fn bar(&self) -> Bar {
        if !self.active && !self.work.is_empty() {
            return Bar::Measured(Measure { done: 1, total: 1 });
        }
        let mut result = Measure { done: 0, total: 0 };
        let mut unit = None;
        for contribution in self.work.values() {
            let Some(contribution) = contribution else {
                return Bar::Moving;
            };
            if contribution.measure.total == 0 {
                continue;
            }
            if unit.is_some_and(|unit| unit != contribution.unit) {
                return Bar::Moving;
            }
            unit = Some(contribution.unit);
            result.done = result.done.saturating_add(contribution.measure.done);
            result.total = result.total.saturating_add(contribution.measure.total);
        }
        if result.total == 0 {
            if self.active {
                Bar::Moving
            } else {
                Bar::Measured(Measure { done: 1, total: 1 })
            }
        } else {
            Bar::Measured(result)
        }
    }
}
