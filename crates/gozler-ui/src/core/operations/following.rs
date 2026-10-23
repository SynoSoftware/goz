//! The canonical follower moves through one pass; its immutable presentation
//! stays available while the next snapshot is being processed.

use super::*;
use crate::core::panel::PanelKeeps;
use crate::core::search_list::Ended;
use crate::core::work::INLINE_ITEMS;
use gozler_fileops::Taken;
use std::collections::LinkedList;

pub(crate) enum FollowChange {
    Started(OperationId, Source),
    Submitted(OperationId),
    Reported(OperationId),
    Select(Detail),
    Open(Option<OperationId>),
    CloseReport,
}

pub(crate) struct Presentation {
    pub(crate) status: Option<StatusOperation>,
    pub(crate) progress: OperationProgress,
    pub(crate) view: Arc<View>,
    pub(crate) caption: Option<crate::core::status_transfer::Caption>,
    pub(crate) backlog: String,
    reports: HashMap<OperationId, Operation>,
    transfers: HashSet<OperationId>,
    ended: HashSet<OperationId>,
}

impl Presentation {
    pub(crate) fn report(&self, id: OperationId) -> Option<&Operation> {
        self.reports.get(&id)
    }

    pub(crate) fn outcome(&self, id: OperationId) -> Option<Arc<Outcome>> {
        let State::Ended(outcome) = &self.report(id)?.state else {
            return None;
        };
        Some(Arc::clone(outcome))
    }

    pub(crate) fn is_transfer(&self, id: OperationId) -> bool {
        self.transfers.contains(&id)
    }
    pub(crate) fn has_ended(&self, id: OperationId) -> bool {
        self.ended.contains(&id)
    }
}

pub(crate) struct Following {
    pub(crate) operations: Operations,
    pub(crate) snapshot: Arc<[Operation]>,
    pub(crate) taken: Arc<Taken>,
    pub(crate) received: bool,
    pub(crate) changes: LinkedList<FollowChange>,
    pub(crate) prior: Option<StatusOperation>,
    pub(crate) prior_view: Option<Arc<View>>,
    pub(crate) now: Instant,
    pub(crate) strings: Strings,
    pub(crate) keepers: Option<Keepers>,
    pub(crate) version: u64,
    pub(crate) job_moment: u64,
}

pub(crate) struct Followed {
    pub(crate) operations: Operations,
    pub(crate) presentation: Arc<Presentation>,
    pub(crate) ended: Option<Arc<Ended>>,
    pub(in crate::core) seen: Seen,
    pub(crate) release: Option<Release>,
    pub(crate) version: u64,
    pub(crate) job_moment: u64,
}

pub(crate) struct Reporting {
    pub(crate) operation: Operation,
    pub(crate) strings: Strings,
}

pub(crate) struct PreparedFailure {
    pub(crate) id: OperationId,
    pub(crate) outcome: Arc<Outcome>,
    pub(crate) strings: Strings,
    pub(crate) failure: Option<Failure>,
}

impl Reporting {
    pub(crate) fn items(&self) -> usize {
        1 + operation_items(&self.operation)
    }

    pub(crate) fn run(
        self,
        numbers: &dyn Numbers,
        windows_text: &dyn Fn(u32) -> String,
    ) -> PreparedFailure {
        let State::Ended(outcome) = self.operation.state else {
            unreachable!("a retained report belongs to an ended operation");
        };
        let failure = file_operations::failure(
            &self.strings,
            numbers,
            &self.operation.request,
            &outcome,
            windows_text,
        );
        PreparedFailure {
            id: self.operation.id,
            outcome,
            strings: self.strings,
            failure,
        }
    }
}

pub(crate) struct Keepers {
    pub(crate) searches: [Option<Arc<HashSet<OperationId>>>; 3],
    pub(crate) panels: PanelKeeps,
}

impl Keepers {
    fn contains(&self, id: OperationId) -> bool {
        self.searches
            .iter()
            .flatten()
            .any(|kept| kept.contains(&id))
            || self.panels.contains(id)
    }

    fn items(&self) -> usize {
        self.searches
            .iter()
            .flatten()
            .map(|kept| kept.len())
            .sum::<usize>()
            + self.panels.items()
    }
}

pub(crate) struct Release {
    pub(crate) keepers: Keepers,
    pub(crate) ids: Vec<OperationId>,
    pub(crate) next: usize,
}

fn operation_items(operation: &Operation) -> usize {
    let sources = match &*operation.request {
        Request::Copy { sources, .. }
        | Request::Move { sources, .. }
        | Request::Delete { sources } => sources.len(),
        Request::NewFolder { .. } | Request::Synchronize { .. } => 1,
    };
    sources
        + match &operation.state {
            State::Ended(outcome) => outcome.listed.len(),
            _ => 0,
        }
}

impl Operations {
    pub(super) fn items(&self) -> usize {
        let mut items = self.phases.len()
            + self.sources.len()
            + self.reports.len()
            + self.meters.len()
            + self.numbered.len();
        if items <= INLINE_ITEMS {
            for operation in self.reports.values().chain(self.selected_ended.iter()) {
                items = items.saturating_add(operation_items(operation));
                if items > INLINE_ITEMS {
                    break;
                }
            }
        }
        items
    }
}

impl Following {
    pub(crate) fn items(&self) -> usize {
        let mut items = self.operations.items()
            + self.snapshot.len()
            + self.changes.len()
            + self.taken.len()
            + self.keepers.as_ref().map_or(0, Keepers::items)
            + self.prior_view.as_ref().map_or(0, |view| view.rows.len());
        if items <= INLINE_ITEMS {
            for operation in &*self.snapshot {
                items = items.saturating_add(operation_items(operation));
                if items > INLINE_ITEMS {
                    break;
                }
            }
        }
        items
    }

    pub(crate) fn run(mut self, numbers: &dyn Numbers) -> Followed {
        let prior_speed = self
            .prior
            .as_ref()
            .and_then(|operation| self.operations.speed(operation.id()));
        let mut sources = Vec::new();
        let mut queue = Vec::new();
        for change in self.changes {
            match change {
                FollowChange::Started(id, source) => self.operations.started(id, source),
                FollowChange::Submitted(id) => {
                    sources.extend(self.operations.submitted(id));
                }
                FollowChange::Reported(id) => {
                    queue.extend(self.operations.reported(id));
                }
                FollowChange::Select(detail) => self.operations.select(detail),
                FollowChange::Open(id) => self.operations.open_selection(id, &self.snapshot),
                FollowChange::CloseReport => self.operations.close_report(),
            }
        }
        let release = self.keepers.map(|keepers| {
            let ids = self
                .operations
                .kept()
                .filter(|id| !keepers.contains(*id))
                .collect();
            Release {
                keepers,
                ids,
                next: 0,
            }
        });
        let mut seen = if self.received {
            self.operations.accept(&self.snapshot, self.now)
        } else {
            Seen {
                queue: Vec::new(),
                dialogs: Vec::new(),
                sources: Vec::new(),
                ended: Vec::new(),
            }
        };
        queue.append(&mut seen.queue);
        seen.queue = queue;
        sources.append(&mut seen.sources);
        seen.sources = sources;
        let ended = (!seen.ended.is_empty()).then(|| {
            Arc::new(Ended {
                operations: std::mem::take(&mut seen.ended),
                in_queue: self.snapshot.iter().map(|operation| operation.id).collect(),
                taken: Arc::clone(&self.taken),
            })
        });
        let counts_changed = self.received && self.operations.record(&self.snapshot, self.now);
        let status = self.operations.status(&self.snapshot);
        let speed = status
            .as_ref()
            .and_then(|operation| self.operations.speed(operation.id()));
        let progress = OperationProgress {
            status_changed: status != self.prior || counts_changed || speed != prior_speed,
            taskbar: file_operations::taskbar(&self.snapshot),
            polls: self
                .snapshot
                .iter()
                .any(|operation| matches!(operation.state, State::Running(_))),
        };
        let backlog = status.as_ref().map_or_else(String::new, |operation| {
            self.operations.backlog(&self.strings, numbers, operation)
        });
        let shown = self.operations.with_reports(&self.snapshot);
        let transfers = shown
            .iter()
            .filter(|operation| file_operations::is_transfer(&operation.request))
            .map(|operation| operation.id)
            .collect();
        let mut view = file_operations::view(
            &self.strings,
            numbers,
            &shown,
            &self.operations.meters,
            self.operations.detail,
        );
        if let Some(prior) = &self.prior_view {
            view.rows.reuse_order(&prior.rows);
        }
        let view = Arc::new(view);
        let caption = self.operations.transfer_caption(&self.strings, numbers);
        let presentation = Arc::new(Presentation {
            status,
            progress,
            view,
            caption,
            backlog,
            reports: self.operations.reports.clone(),
            transfers,
            ended: self
                .operations
                .phases
                .iter()
                .filter(|(_, phase)| phase.ended())
                .map(|(&id, _)| id)
                .collect(),
        });
        Followed {
            operations: self.operations,
            presentation,
            ended,
            seen,
            release,
            version: self.version,
            job_moment: self.job_moment,
        }
    }
}
