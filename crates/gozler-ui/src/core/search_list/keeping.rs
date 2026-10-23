//! Retains the fetched-row evidence of each completed operation until its
//! calculation is accepted; later cache eviction cannot erase that evidence.

use super::super::fetch::{CachedRows, Generation};
use super::super::selection::wide_path_into;
use super::super::work::INLINE_ITEMS;
use gozler_fileops::{OperationId, Request, Taken};
use std::collections::{HashSet, LinkedList};
use std::sync::Arc;

pub(crate) struct EndEvidence {
    pub(super) generation: Generation,
    pub(super) rows: Vec<CachedRows>,
}

impl EndEvidence {
    pub(crate) fn items(&self) -> usize {
        self.rows.iter().map(|chunk| chunk.rows.len()).sum()
    }
}

#[derive(Debug)]
pub(crate) struct Ended {
    pub(crate) operations: Vec<(OperationId, Arc<Request>)>,
    pub(crate) in_queue: HashSet<OperationId>,
    pub(crate) taken: Arc<Taken>,
}

impl Ended {
    pub(crate) fn items(&self) -> usize {
        let mut items = self.in_queue.len() + self.taken.len() + self.operations.len();
        // Admission and disposal need only distinguish an inline batch from a
        // larger one; counting that larger batch would itself stop the window.
        if items <= INLINE_ITEMS {
            for (_, request) in &self.operations {
                items += match &**request {
                    Request::Copy { sources, .. }
                    | Request::Move { sources, .. }
                    | Request::Delete { sources } => sources.len(),
                    Request::NewFolder { .. } | Request::Synchronize { .. } => 1,
                };
                if items > INLINE_ITEMS {
                    break;
                }
            }
        }
        items
    }
}

#[derive(Debug)]
struct Evidence {
    ended: Arc<Ended>,
    rows: Vec<CachedRows>,
}

#[derive(Debug, Default)]
pub(super) struct Pending {
    frames: LinkedList<Evidence>,
    items: usize,
}

impl Pending {
    pub(super) fn record(&mut self, ended: Arc<Ended>, rows: Vec<CachedRows>) {
        self.items = self.items.saturating_add(
            ended.items() + rows.iter().map(|chunk| chunk.rows.len()).sum::<usize>(),
        );
        self.frames.push_back(Evidence { ended, rows });
    }

    pub(super) fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub(super) fn items(&self) -> usize {
        self.items
    }
}

#[derive(Debug)]
pub(crate) struct Keep {
    pub(super) generation: Generation,
    pub(super) kept: Arc<HashSet<OperationId>>,
    pub(super) pending: Pending,
}

#[derive(Debug)]
pub(crate) struct Kept {
    pub(super) generation: Generation,
    pub(crate) kept: Arc<HashSet<OperationId>>,
}

impl Keep {
    pub(crate) fn items(&self) -> usize {
        self.pending.items().saturating_add(self.kept.len())
    }

    pub(crate) fn run(self) -> Kept {
        let mut kept = (*self.kept).clone();
        let (mut path, mut folded) = (Vec::new(), Vec::new());
        for frame in self.pending.frames {
            let mut wanted: Vec<OperationId> = frame
                .ended
                .operations
                .iter()
                .filter(|(id, request)| {
                    matches!(**request, Request::Delete { .. } | Request::Move { .. })
                        && !kept.contains(id)
                })
                .map(|(id, _)| *id)
                .collect();
            for row in frame.rows.iter().flat_map(|chunk| chunk.rows.iter()) {
                if wanted.is_empty() {
                    break;
                }
                wide_path_into(row, &mut path);
                frame.ended.taken.takers(&path, &mut folded, |takers| {
                    wanted.retain(|id| {
                        let takes = takers.contains(id);
                        if takes {
                            kept.insert(*id);
                        }
                        !takes
                    });
                });
            }
            // Queue membership is newer than the captured Taken snapshot.
            kept.retain(|id| frame.ended.in_queue.contains(id));
        }
        Kept {
            generation: self.generation,
            kept: Arc::new(kept),
        }
    }
}
