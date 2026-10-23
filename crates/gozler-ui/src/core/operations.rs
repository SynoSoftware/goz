//! The life of each file operation as the window follows it: which reports
//! remain available, which operation the status bar shows, and when an ended
//! one leaves the queue. The engine owns the operations and their states;
//! the work owner moves the follower through each received snapshot;
//! `app` applies its ordered steps and immutable presentation.

use super::file_operations::{
    self, Completion, Detail, Failure, LastEnded, Meter, Speed, StatusOperation, Taskbar, View,
};
use super::panel::PanelFollow;
use super::text::{Numbers, Strings};
use gozler_fileops::{Operation, OperationId, Outcome, Policy, Request, State};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

mod following;
pub(crate) use following::{
    FollowChange, Followed, Following, Keepers, PreparedFailure, Presentation, Release, Reporting,
};

/// Where the files that a copy or a move acts on came from, which hears
/// what became of them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Paste; `cut` when the files were cut, so a move of them tells the
    /// clipboard that they moved.
    Paste { cut: bool },
    /// A drop, whose source keeps its files until the operation ends.
    Drop,
}

/// A call to the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueStep {
    /// The window records acceptance through `Model::operation_submitted`.
    Submit { id: OperationId },
    /// Removes an ended operation, which changes the queue's taken entries.
    Remove(OperationId),
}

/// What an operation's [`Source`] hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceStep {
    /// The pasted operation ended; `moved` only when all cut files moved.
    Pasted { id: OperationId, moved: bool },
    /// The operation ended; `succeeded` when it did all it was asked to.
    Ended { id: OperationId, succeeded: bool },
}

/// A dialog of an operation, which waits for `Message::Deferred` because
/// it runs its own message loop (DESIGN.md: window rule 13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationDialog {
    /// The failure dialog of an ended operation.
    Report(OperationId),
}

/// What the window does after a snapshot of the queue
/// ([`super::model::Model::accept_followed`]), in the order of the
/// fields. The sources come last, because a drop's source is another
/// program, and COM can deliver messages while the window waits for it.
#[derive(Debug, PartialEq)]
pub(crate) struct OperationSteps {
    pub(crate) queue: Vec<QueueStep>,
    pub(crate) panels: Vec<PanelFollow>,
    pub(crate) dialogs: Vec<OperationDialog>,
    pub(crate) progress: OperationProgress,
    pub(crate) sources: Vec<SourceStep>,
}

/// What the status bar and the taskbar button show of the operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OperationProgress {
    /// The operation the status bar shows changed, or its counts did.
    pub(crate) status_changed: bool,
    pub(crate) taskbar: Taskbar,
    /// An operation runs, whose counts change without a wake, so the
    /// window reads them twice a second.
    pub(crate) polls: bool,
}

/// What a snapshot changed ([`Operations::accept`]).
#[derive(Debug, PartialEq)]
pub(super) struct Seen {
    pub(super) queue: Vec<QueueStep>,
    pub(super) dialogs: Vec<OperationDialog>,
    pub(super) sources: Vec<SourceStep>,
    /// The operations that ended since the snapshot before, each with its
    /// request, which the lists compare with what they show.
    pub(super) ended: Vec<(OperationId, Arc<Request>)>,
}

/// Where an operation is in its life, as the window follows it.
#[derive(Debug)]
enum Phase {
    /// Nothing waits for the window: the operation prepares, waits, runs or
    /// was interrupted.
    Live,
    Submitting {
        numbered: bool,
    },
    /// Ended; it stays in the queue while a list keeps its taken entries.
    Kept,
    /// Ended, and leaving the queue.
    Removed,
}

impl Phase {
    fn ended(&self) -> bool {
        matches!(self, Phase::Kept | Phase::Removed)
    }
}

/// Every operation in the queue as the window follows it.
#[derive(Debug, Default)]
pub(crate) struct Operations {
    detail: Option<Detail>,
    selected_ended: Option<Operation>,
    batch: file_operations::batch::Batch,
    transfer: Option<StatusOperation>,
    phases: HashMap<OperationId, Phase>,
    sources: HashMap<OperationId, Source>,
    reports: HashMap<OperationId, Operation>,
    reports_changed: bool,
    meters: HashMap<OperationId, Meter>,
    counts: Counts,
    /// Keep both can choose a numbered name the window does not know.
    numbered: HashSet<OperationId>,
    /// The operation reported last, which the status bar shows after the
    /// queue let it go.
    last_ended: Option<LastEnded>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Counts {
    queued: u64,
    running: u64,
    interrupted: u64,
}

impl Operations {
    pub(super) fn record(&mut self, snapshot: &[Operation], now: Instant) -> bool {
        let prior_bar = self.batch.bar();
        let prior_speed = self.transfer.as_ref().and_then(|op| self.speed(op.id()));
        self.batch.record(snapshot);
        let mut counts = Counts::default();
        let mut running = HashSet::new();
        for operation in snapshot {
            if matches!(operation.state, State::Running(_)) {
                running.insert(operation.id);
            }
            match &operation.state {
                State::Preparing
                | State::Prepared(_)
                | State::Compared(_)
                | State::Waiting { .. } => counts.queued += 1,
                State::Running(running) if running.is_active() => counts.running += 1,
                State::Cancelling => counts.running += 1,
                State::Interrupted => counts.interrupted += 1,
                _ => {}
            }
        }
        let reports_changed = std::mem::take(&mut self.reports_changed);
        let mut changed = counts != self.counts || reports_changed;
        self.counts = counts;
        self.meters.retain(|id, _| running.contains(id));
        for operation in snapshot {
            if let State::Running(running) = &operation.state {
                let meter = self.meters.entry(operation.id).or_default();
                if !running.is_active() {
                    meter.clear();
                } else {
                    meter.record(now, running.items, running.bytes);
                }
            }
        }
        let transfer = self.transfer_status(snapshot);
        let speed = transfer.as_ref().and_then(|op| self.speed(op.id()));
        changed |=
            transfer != self.transfer || speed != prior_speed || prior_bar != self.batch.bar();
        self.transfer = transfer;
        changed
    }

    pub(super) fn backlog(
        &self,
        strings: &Strings,
        numbers: &dyn Numbers,
        operation: &StatusOperation,
    ) -> String {
        use super::file_operations::Pending;
        let queued = self.counts.queued.saturating_sub(u64::from(matches!(
            operation,
            StatusOperation::Pending {
                phase: Pending::Preparing | Pending::Waiting | Pending::Review,
                ..
            }
        )));
        let active = self.counts.running.saturating_sub(u64::from(
            matches!(operation, StatusOperation::Running { running, .. } if running.is_active())
                || matches!(
                    operation,
                    StatusOperation::Pending {
                        phase: Pending::Cancelling,
                        ..
                    }
                ),
        ));
        if matches!(
            operation,
            StatusOperation::Pending {
                phase: Pending::Interrupted,
                ..
            }
        ) {
            return strings.count(
                "file_operations.restored",
                self.counts.interrupted,
                numbers,
                &[],
            );
        }
        [
            ("file_operations.queued", queued),
            ("file_operations.active", active),
            ("file_operations.restored", self.counts.interrupted),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(key, count)| strings.count(key, count, numbers, &[]))
        .collect::<Vec<_>>()
        .join(strings.text("file_operations.separator"))
    }

    pub(super) fn speed(&self, id: OperationId) -> Option<Speed> {
        self.meters.get(&id).and_then(Meter::speed)
    }

    #[cfg(test)]
    pub(super) fn view(
        &self,
        strings: &Strings,
        numbers: &dyn Numbers,
        snapshot: &[Operation],
    ) -> View {
        file_operations::view(
            strings,
            numbers,
            &self.with_reports(snapshot),
            &self.meters,
            self.detail,
        )
    }

    pub(super) fn select(&mut self, detail: Detail) {
        if self.detail.is_none_or(|prior| prior.id() != detail.id()) {
            self.selected_ended = None;
        }
        self.detail = Some(detail);
    }

    pub(super) fn open_selection(&mut self, select: Option<OperationId>, snapshot: &[Operation]) {
        if let Some(id) = select {
            self.select(Detail::Progress(id));
        } else if self.detail.is_none() {
            let shown = self.with_reports(snapshot);
            if let Some(op) = shown
                .iter()
                .find(|op| {
                    file_operations::is_transfer(&op.request)
                        && matches!(&op.state, State::Running(running) if running.is_active())
                })
                .or_else(|| {
                    shown
                        .iter()
                        .find(|op| file_operations::is_transfer(&op.request))
                })
            {
                self.select(Detail::Progress(op.id));
            }
        }
        if let Some(op) = snapshot.iter().find(|op| {
            self.detail.is_some_and(|detail| detail.id() == op.id)
                && matches!(op.state, State::Ended(_))
        }) {
            self.selected_ended = Some(op.clone());
        }
    }

    pub(super) fn close_report(&mut self) {
        self.detail = self.detail.map(|detail| Detail::Progress(detail.id()));
    }

    /// The queue prepares the operation `id`, whose files came from
    /// `source`.
    pub(super) fn started(&mut self, id: OperationId, source: Source) {
        self.sources.insert(id, source);
    }

    /// Follows `snapshot`, the queue's operations at `now`. A prepared
    /// operation is submitted, and
    /// an ended one reports what failed, or is reported at once; each only
    /// once.
    pub(super) fn accept(&mut self, snapshot: &[Operation], _now: Instant) -> Seen {
        let present: HashSet<OperationId> = snapshot.iter().map(|operation| operation.id).collect();
        self.phases.retain(|id, _| present.contains(id));
        (self.numbered).retain(|id| present.contains(id));
        let mut seen = Seen {
            queue: Vec::new(),
            dialogs: Vec::new(),
            sources: Vec::new(),
            ended: Vec::new(),
        };
        for operation in snapshot {
            let id = operation.id;
            let request = &operation.request;
            let phase = self.phases.entry(id).or_insert(Phase::Live);
            match &operation.state {
                State::Prepared(_) if matches!(phase, Phase::Live) => {
                    *phase = Phase::Submitting {
                        numbered: request.policy() == Some(Policy::KeepBoth),
                    };
                    seen.queue.push(QueueStep::Submit { id });
                }
                State::Ended(outcome) if !phase.ended() => {
                    let took = outcome.items > 0
                        && matches!(
                            **request,
                            Request::Delete { .. }
                                | Request::Move { .. }
                                | Request::Synchronize { .. }
                        );
                    *phase = if took { Phase::Kept } else { Phase::Removed };
                    self.last_ended = Some((
                        id,
                        Completion::of(request, outcome, self.numbered.contains(&id)),
                    ));
                    if self.detail.is_some_and(|detail| detail.id() == id) {
                        self.selected_ended = Some(operation.clone());
                    }
                    seen.ended.push((id, Arc::clone(request)));
                    if let Some(source) = self.sources.remove(&id) {
                        let succeeded = outcome.succeeded;
                        seen.sources.push(match source {
                            Source::Paste { cut } => SourceStep::Pasted {
                                id,
                                moved: cut && moves(request) && succeeded,
                            },
                            Source::Drop => SourceStep::Ended { id, succeeded },
                        });
                    }
                    if file_operations::reports(outcome) {
                        self.reports.insert(id, operation.clone());
                        if !file_operations::is_transfer(request) {
                            seen.dialogs.push(OperationDialog::Report(id));
                        }
                    }
                    if !took {
                        seen.queue.push(QueueStep::Remove(id));
                    }
                }
                _ => {}
            }
        }
        seen
    }

    /// Records the accepted choice; source acknowledgement waits for completion.
    pub(super) fn submitted(&mut self, id: OperationId) -> Option<SourceStep> {
        let phase = self.phases.get_mut(&id)?;
        let Phase::Submitting { numbered } = *phase else {
            return None;
        };
        *phase = Phase::Live;
        if numbered {
            self.numbered.insert(id);
        }
        None
    }

    pub(super) fn reported(&mut self, id: OperationId) -> Option<QueueStep> {
        self.reports_changed |= self.reports.remove(&id).is_some();
        if self
            .transfer
            .as_ref()
            .is_some_and(|status| status.id() == id)
        {
            self.transfer = self.transfer_status(&[]);
        }
        if self.detail.is_some_and(|detail| detail.id() == id) {
            self.detail = Some(Detail::Progress(id));
            self.selected_ended = None;
        }
        None
    }

    pub(super) fn with_reports(&self, snapshot: &[Operation]) -> Vec<Operation> {
        let present: HashSet<_> = snapshot.iter().map(|operation| operation.id).collect();
        let mut shown: Vec<_> = snapshot
            .iter()
            .filter(|operation| {
                !matches!(operation.state, State::Ended(_))
                    || self.reports.contains_key(&operation.id)
                    || self
                        .selected_ended
                        .as_ref()
                        .is_some_and(|op| op.id == operation.id)
            })
            .cloned()
            .collect();
        let mut reports: Vec<_> = self
            .reports
            .values()
            .filter(|operation| !present.contains(&operation.id))
            .cloned()
            .collect();
        reports.sort_unstable_by_key(|operation| operation.id);
        shown.extend(reports);
        if let Some(selected) = &self.selected_ended
            && !shown.iter().any(|op| op.id == selected.id)
        {
            shown.push(selected.clone());
        }
        shown
    }

    /// The reported operations that stay in the queue while a list keeps
    /// them.
    pub(super) fn kept(&self) -> impl Iterator<Item = OperationId> + '_ {
        (self.phases.iter())
            .filter(|(_, phase)| matches!(phase, Phase::Kept))
            .map(|(id, _)| *id)
    }

    /// Removes from the queue the kept operations `released`, which no
    /// list keeps any more.
    pub(super) fn release(&mut self, released: &[OperationId]) -> Vec<QueueStep> {
        let mut steps = Vec::new();
        for id in released {
            if let Some(phase) = self.phases.get_mut(id)
                && matches!(phase, Phase::Kept)
            {
                *phase = Phase::Removed;
                steps.push(QueueStep::Remove(*id));
            }
        }
        steps
    }

    pub(super) fn status(&self, snapshot: &[Operation]) -> Option<StatusOperation> {
        let reported = self.reported_ids();
        let independent: Vec<_> = snapshot
            .iter()
            .filter(|op| {
                !file_operations::is_transfer(&op.request)
                    && !matches!(op.state, State::Ended(_) | State::Interrupted)
            })
            .cloned()
            .collect();
        let shown = if independent.is_empty() {
            snapshot
        } else {
            &independent
        };
        StatusOperation::of(shown, self.last_ended.as_ref(), &reported, &self.numbered)
    }

    fn reported_ids(&self) -> HashSet<OperationId> {
        (self.phases.iter())
            .filter(|(_, phase)| matches!(phase, Phase::Kept | Phase::Removed))
            .map(|(id, _)| *id)
            .collect()
    }
    pub(super) fn transfer_caption(
        &self,
        strings: &Strings,
        numbers: &dyn Numbers,
    ) -> Option<super::status_transfer::Caption> {
        let operation = self.transfer.as_ref()?;
        let report = self
            .reports
            .get(&operation.id())
            .or_else(|| {
                self.reports
                    .values()
                    .filter(|op| file_operations::is_transfer(&op.request))
                    .min_by_key(|op| op.id)
            })
            .map(|op| op.id);
        Some(file_operations::transfer_caption(
            strings,
            numbers,
            operation,
            self.speed(operation.id()),
            self.batch.bar(),
            self.reports
                .values()
                .filter(|op| self.batch.contains(op.id) || op.id == operation.id())
                .fold(0, |errors, op| {
                    errors
                        | match &*op.request {
                            Request::Copy { .. } => 1,
                            Request::Move { .. } => 2,
                            Request::Synchronize { .. } => 4,
                            _ => 0,
                        }
                }),
            report,
        ))
    }

    fn transfer_status(&self, snapshot: &[Operation]) -> Option<StatusOperation> {
        let mut transfers: Vec<_> = snapshot
            .iter()
            .filter(|op| {
                file_operations::is_transfer(&op.request) && !matches!(op.state, State::Ended(_))
            })
            .cloned()
            .collect();
        let mut reports: Vec<_> = self
            .reports
            .values()
            .filter(|op| file_operations::is_transfer(&op.request))
            .cloned()
            .collect();
        reports.sort_unstable_by_key(|op| std::cmp::Reverse(op.id));
        transfers.extend(reports);
        StatusOperation::of(&transfers, None, &HashSet::new(), &self.numbered)
    }
}

/// Whether `request` moves its entries, so a move of cut files tells the
/// clipboard that they moved.
fn moves(request: &Request) -> bool {
    matches!(request, Request::Move { .. })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::text::tests::{English, english};
    use gozler_fileops::{
        Destination, End, Listed, Outcome, Prepared, Reason, Refusal, RingCounts, Running,
        SourcesFolder, Wait,
    };
    use std::path::PathBuf;

    fn id(n: u64) -> OperationId {
        OperationId::for_tests(n)
    }

    fn op(n: u64, request: Request, state: State) -> Operation {
        Operation {
            id: id(n),
            request: Arc::new(request),
            folder: SourcesFolder::Pending,
            state,
        }
    }

    fn copy() -> Request {
        Request::Copy {
            policy: gozler_fileops::Policy::default(),
            sources: vec![PathBuf::from(r"C:\f\a")],
            destination: Destination::Folder(PathBuf::from(r"D:\b")),
        }
    }

    fn move_to() -> Request {
        Request::Move {
            policy: gozler_fileops::Policy::default(),
            sources: vec![PathBuf::from(r"C:\f\a")],
            destination: Destination::Folder(PathBuf::from(r"D:\b")),
        }
    }

    fn delete() -> Request {
        Request::Delete {
            sources: vec![PathBuf::from(r"C:\f\a")],
        }
    }

    fn running() -> State {
        State::Running(Running {
            phase: gozler_fileops::RunningPhase::Executing,
            items: 0,
            bytes: 0,
            passed: 0,
            total: None,
            renames: false,
            paused: false,
            yielded: false,
            current: None,
        })
    }

    fn waiting() -> State {
        State::Waiting {
            on: id(99),
            reason: Wait::Disk,
            total: None,
        }
    }

    fn prepared(_conflicts: u64) -> State {
        State::Prepared(Prepared {
            entries: 1,
            total: None,
        })
    }

    /// Ended as `end` after `items` items, with one failed entry when
    /// `failed`.
    fn ended(end: End, items: u64, failed: bool) -> State {
        let listed = Listed {
            path: PathBuf::from(r"C:\f\a"),
            reason: Reason::Failed(5),
        };
        State::Ended(Arc::new(Outcome {
            succeeded: end == End::Complete && !failed,
            end,
            items,
            completed: items.saturating_sub(u64::from(failed)),
            bytes: 0,
            listed: if failed { vec![listed] } else { Vec::new() },
            ring: RingCounts::default(),
        }))
    }

    fn numbered(status: Option<StatusOperation>) -> Option<bool> {
        match status? {
            StatusOperation::Ended { numbered, .. } => Some(numbered),
            StatusOperation::Summary { completion, .. } => Some(completion.numbered),
            StatusOperation::Running { .. } | StatusOperation::Pending { .. } => None,
        }
    }

    /// A cancelled command replaces stale success even before its first item.
    #[test]
    fn cancellation_before_the_first_item_replaces_stale_success() {
        let mut operations = Operations::default();
        let ended_two = [
            op(1, copy(), ended(End::Complete, 1, true)),
            op(2, copy(), ended(End::Complete, 1, false)),
        ];
        operations.accept(&ended_two, Instant::now());
        let shown = |operations: &Operations, snapshot: &[Operation]| {
            operations.status(snapshot).map(|status| status.id())
        };
        assert_eq!(shown(&operations, &ended_two), Some(id(2)));
        let with_running = [
            op(1, copy(), ended(End::Complete, 1, true)),
            op(3, copy(), running()),
        ];
        assert_eq!(shown(&operations, &with_running), Some(id(3)));
        operations.reported(id(1));
        assert_eq!(shown(&operations, &[]), Some(id(2)));
        let cancelled = [op(4, copy(), ended(End::Cancelled, 0, false))];
        operations.accept(&cancelled, Instant::now());
        assert_eq!(shown(&operations, &cancelled), Some(id(4)));
        assert_eq!(shown(&operations, &[]), Some(id(4)));
    }

    /// Keep both may give an entry a numbered name that the window does not
    /// know, so only a Keep both that the queue accepted keeps the status
    /// line from naming the entry.
    #[test]
    fn keep_both_counts_only_once_the_queue_accepts_the_submit() {
        let mut operations = Operations::default();
        let mut request = copy();
        if let Request::Copy { policy, .. } = &mut request {
            *policy = Policy::KeepBoth;
        }
        let prepared = [
            op(1, request.clone(), prepared(1)),
            op(2, request, prepared(1)),
        ];
        operations.accept(&prepared, Instant::now());
        operations.submitted(id(2));
        let first = [
            op(1, copy(), ended(End::Complete, 1, false)),
            op(2, copy(), waiting()),
        ];
        operations.accept(&first, Instant::now());
        assert_eq!(numbered(operations.status(&[])), Some(false));
        let second = [op(2, copy(), ended(End::Complete, 1, false))];
        operations.accept(&second, Instant::now());
        assert_eq!(numbered(operations.status(&second)), Some(true));
    }

    /// Accepting a move does not prove it succeeded, so Cut survives until
    /// every source has moved and survives a later cancellation.
    #[test]
    fn a_cut_paste_is_consumed_only_after_successful_completion() {
        let mut operations = Operations::default();
        operations.started(id(1), Source::Paste { cut: true });
        let seen = operations.accept(&[op(1, move_to(), prepared(0))], Instant::now());
        assert_eq!(seen.queue, [QueueStep::Submit { id: id(1) }]);
        assert!(seen.sources.is_empty());
        assert_eq!(operations.submitted(id(1)), None);
        let complete = [op(1, move_to(), ended(End::Complete, 1, false))];
        assert_eq!(
            operations.accept(&complete, Instant::now()).sources,
            [SourceStep::Pasted {
                id: id(1),
                moved: true
            }]
        );
        operations.started(id(2), Source::Paste { cut: true });
        operations.accept(&[op(2, move_to(), prepared(0))], Instant::now());
        let cancelled = [op(2, move_to(), ended(End::Cancelled, 0, false))];
        assert_eq!(
            operations.accept(&cancelled, Instant::now()).sources,
            [SourceStep::Pasted {
                id: id(2),
                moved: false
            }]
        );

        operations.started(id(3), Source::Paste { cut: true });
        let recovery_failed = Outcome::new(
            End::Complete,
            1,
            0,
            vec![Listed {
                path: "operations.json".into(),
                reason: Reason::RecoveryFailed(5),
            }],
            RingCounts::default(),
        );
        let complete = [op(3, move_to(), State::Ended(Arc::new(recovery_failed)))];
        assert_eq!(
            operations.accept(&complete, Instant::now()).sources,
            [SourceStep::Pasted {
                id: id(3),
                moved: true
            }]
        );
        assert_eq!(operations.with_reports(&[]).len(), 1);
    }

    #[test]
    fn a_dormant_interrupted_transfer_does_not_hide_the_completed_error_report() {
        let strings = english();
        let now = Instant::now();
        let mut operations = Operations::default();
        let restored = [
            op(1, copy(), State::Interrupted),
            op(2, move_to(), State::Interrupted),
        ];
        operations.record(&restored, now);
        assert_eq!(
            operations.transfer_caption(&strings, &English).unwrap().id,
            id(1)
        );
        let started = [
            op(1, copy(), running()),
            op(2, move_to(), State::Interrupted),
        ];
        operations.accept(&started, now);
        operations.record(&started, now);
        let failed = [
            op(1, copy(), ended(End::Complete, 1, true)),
            op(2, move_to(), State::Interrupted),
        ];
        assert_eq!(operations.status(&failed).unwrap().id(), id(1));
        operations.accept(&failed, now);
        operations.select(Detail::Report(id(1)));
        operations.close_report();
        operations.record(&failed, now);
        for snapshot in [&failed[..], &failed[1..]] {
            operations.record(snapshot, now);
            assert_eq!(operations.status(snapshot).unwrap().id(), id(1));
            let caption = operations.transfer_caption(&strings, &English).unwrap();
            assert_eq!(caption.id, id(1));
            assert_eq!(caption.activity, "Copy: Completed with errors");
            assert_eq!(
                caption.report.unwrap().command,
                file_operations::QueueAction::Report
            );
        }
    }

    #[test]
    fn new_intake_is_visible_ahead_of_dormant_interrupted_work() {
        let operations = Operations::default();
        let snapshot = [
            op(1, copy(), State::Interrupted),
            op(2, copy(), State::Preparing),
        ];
        assert_eq!(
            operations.status(&snapshot).map(|status| status.id()),
            Some(id(2))
        );
    }

    /// A drop's source keeps its files until it hears the end, so an
    /// operation that never runs must still tell it once, and a second
    /// call would reach a source that already let them go.
    #[test]
    fn every_drop_hears_its_end_once() {
        let mut operations = Operations::default();
        for n in 1..=3 {
            operations.started(id(n), Source::Drop);
        }
        let started = [
            op(1, copy(), prepared(1)),
            op(2, copy(), State::Preparing),
            op(3, copy(), running()),
        ];
        operations.accept(&started, Instant::now());
        let refused = End::Refused(Refusal::DestinationMissing);
        let all_ended = [
            op(1, copy(), ended(End::Cancelled, 0, false)),
            op(2, copy(), ended(refused, 0, false)),
            op(3, copy(), ended(End::Complete, 2, false)),
        ];
        let ends = |n, succeeded| SourceStep::Ended {
            id: id(n),
            succeeded,
        };
        assert_eq!(
            operations.accept(&all_ended, Instant::now()).sources,
            [ends(1, false), ends(2, false), ends(3, true)]
        );
        assert!(
            operations
                .accept(&all_ended, Instant::now())
                .sources
                .is_empty()
        );
    }

    /// Refreshing the lists does not release the report before its deferred
    /// dialog consumes the immutable outcome.
    #[test]
    fn completion_releases_taken_entries_without_acknowledging_the_report() {
        let mut operations = Operations::default();
        let failed = [op(1, delete(), ended(End::Complete, 3, true))];
        let seen = operations.accept(&failed, Instant::now());
        assert_eq!(seen.dialogs, [OperationDialog::Report(id(1))]);
        assert!(seen.queue.is_empty());
        assert_eq!(operations.kept().count(), 1);
        assert_eq!(operations.release(&[id(1)]), [QueueStep::Remove(id(1))]);
        assert_eq!(operations.with_reports(&[]).len(), 1);
        assert_eq!(operations.reported(id(1)), None);
        assert!(operations.with_reports(&[]).is_empty());
        assert!(operations.with_reports(&failed).is_empty());
        assert_eq!(
            operations.status(&[]).map(|status| status.id()),
            Some(id(1))
        );
    }

    #[test]
    fn completing_the_selected_transfer_does_not_retarget_its_buttons() {
        let strings = english();
        let now = Instant::now();
        let mut operations = Operations::default();
        let initial = [op(1, copy(), running()), op(2, move_to(), running())];
        operations.open_selection(None, &initial);
        let finished = [
            op(1, copy(), ended(End::Complete, 1, false)),
            op(2, move_to(), running()),
        ];
        operations.accept(&finished, now);
        operations.record(&finished, now);
        let remaining = &finished[1..];
        let view = operations.view(&strings, &English, remaining);
        let card = view
            .card
            .expect("the selected successful outcome remains visible");
        assert_eq!(card.id, id(1));
        assert!(matches!(
            card.phase,
            file_operations::RowPhase::Completed { .. }
        ));
        assert!(card.phase.buttons().run.is_none());
        assert_eq!(
            card.phase.buttons().end,
            file_operations::QueueAction::Clear
        );
        let caption = operations.transfer_caption(&strings, &English).unwrap();
        assert_eq!(caption.id, id(2));
        assert_eq!(
            caption.cancel.unwrap().command,
            file_operations::QueueAction::Cancel
        );
        operations.reported(id(1));
        operations.open_selection(None, remaining);
        assert!(
            operations
                .view(&strings, &English, remaining)
                .card
                .is_none()
        );
        operations.open_selection(Some(id(2)), remaining);
        assert_eq!(
            operations
                .view(&strings, &English, remaining)
                .card
                .unwrap()
                .id,
            id(2)
        );
    }

    #[test]
    fn closing_a_report_retains_it_but_clearing_releases_it() {
        let strings = english();
        let mut operations = Operations::default();
        let failed = [op(1, copy(), ended(End::Complete, 3, true))];
        operations.accept(&failed, Instant::now());
        operations.select(Detail::Report(id(1)));
        assert_eq!(
            operations
                .view(&strings, &English, &[])
                .report
                .unwrap()
                .outcome
                .listed
                .len(),
            1
        );
        operations.close_report();
        let view = operations.view(&strings, &English, &[]);
        assert!(view.report.is_none());
        assert_eq!(view.card.unwrap().id, id(1));
        operations.select(Detail::Report(id(1)));
        assert!(operations.view(&strings, &English, &[]).report.is_some());
        operations.reported(id(1));
        assert!(operations.view(&strings, &English, &[]).rows.is_empty());
    }

    #[test]
    fn successful_transfers_disappear_and_clearing_errors_removes_the_status() {
        let strings = english();
        let now = Instant::now();
        let mut operations = Operations::default();
        let success = [op(1, copy(), ended(End::Complete, 3, false))];
        operations.accept(&success, now);
        operations.record(&success, now);
        assert!(operations.transfer_caption(&strings, &English).is_none());
        let failures = [
            op(2, copy(), ended(End::Complete, 2, true)),
            op(3, move_to(), ended(End::Complete, 2, true)),
        ];
        operations.accept(&failures, now);
        operations.record(&failures, now);
        operations.reported(id(2));
        operations.record(&failures, now);
        assert_eq!(
            operations
                .transfer_caption(&strings, &English)
                .unwrap()
                .activity,
            "Move: Completed with errors"
        );
        operations.reported(id(3));
        assert!(operations.transfer_caption(&strings, &English).is_none());
        operations.record(&failures, now);
        assert!(operations.transfer_caption(&strings, &English).is_none());
    }

    #[test]
    fn a_fast_failure_reports_the_failed_type_independently_of_delete() {
        let strings = english();
        let now = Instant::now();
        let mut operations = Operations::default();
        let snapshot = [
            op(1, copy(), ended(End::Complete, 3, true)),
            op(2, delete(), running()),
        ];
        operations.accept(&snapshot, now);
        operations.record(&snapshot, now);
        assert_eq!(operations.status(&snapshot).unwrap().id(), id(2));
        let caption = operations.transfer_caption(&strings, &English).unwrap();
        assert_eq!(caption.activity, "Copy: Completed with errors");
        assert!(caption.pause.is_none() && caption.cancel.is_none());
        assert!(caption.report.is_some());
        let next = [op(3, move_to(), running())];
        operations.accept(&next, now);
        operations.record(&next, now);
        let failed = [op(3, move_to(), ended(End::Complete, 2, true))];
        operations.accept(&failed, now);
        operations.record(&failed, now);
        assert_eq!(
            operations
                .transfer_caption(&strings, &English)
                .unwrap()
                .activity,
            "Move: Completed with errors"
        );
        let running_both = [op(4, copy(), running()), op(5, move_to(), running())];
        operations.record(&running_both, now);
        let failed_both = [
            op(4, copy(), ended(End::Complete, 1, true)),
            op(5, move_to(), ended(End::Complete, 1, true)),
        ];
        operations.accept(&failed_both, now);
        operations.record(&failed_both, now);
        assert_eq!(
            operations
                .transfer_caption(&strings, &English)
                .unwrap()
                .activity,
            "Copy & Move: Completed with errors"
        );
    }

    fn measured(done: u64, total: u64, paused: bool, renames: bool) -> State {
        State::Running(Running {
            phase: gozler_fileops::RunningPhase::Executing,
            items: done,
            bytes: if renames { 0 } else { done },
            passed: 0,
            total: Some(gozler_fileops::Total {
                items: total,
                bytes: total,
            }),
            renames,
            paused,
            yielded: false,
            current: None,
        })
    }

    #[test]
    fn queue_progress_keeps_finished_work_and_resets_for_a_new_queue() {
        use file_operations::{Bar, Measure};
        let mut batch = file_operations::batch::Batch::default();
        let waiting = State::Waiting {
            on: id(1),
            reason: Wait::Disk,
            total: Some(gozler_fileops::Total {
                items: 100,
                bytes: 100,
            }),
        };
        batch.record(&[
            op(1, copy(), measured(40, 100, false, false)),
            op(2, copy(), waiting),
        ]);
        assert_eq!(
            batch.bar(),
            Bar::Measured(Measure {
                done: 40,
                total: 200
            })
        );
        batch.record(&[
            op(1, copy(), ended(End::Complete, 100, false)),
            op(2, copy(), measured(20, 100, false, false)),
        ]);
        assert_eq!(
            batch.bar(),
            Bar::Measured(Measure {
                done: 120,
                total: 200
            })
        );
        batch.record(&[op(2, copy(), measured(30, 100, false, false))]);
        assert_eq!(
            batch.bar(),
            Bar::Measured(Measure {
                done: 130,
                total: 200
            })
        );
        let retained = op(2, copy(), ended(End::Complete, 100, true));
        batch.record(std::slice::from_ref(&retained));
        batch.record(&[retained, op(3, copy(), measured(10, 50, false, false))]);
        assert_eq!(
            batch.bar(),
            Bar::Measured(Measure {
                done: 10,
                total: 50
            })
        );
    }

    #[test]
    fn priority_waiting_still_offers_manual_pause_and_preserves_it_on_return() {
        use file_operations::{QueueAction, RowPhase, Taskbar};
        let strings = english();
        let now = Instant::now();
        let mut operations = Operations::default();
        let mut yielded = measured(20, 100, false, false);
        let State::Running(running) = &mut yielded else {
            panic!("measured returns running work")
        };
        running.yielded = true;
        let mut snapshot = [
            op(1, copy(), yielded),
            op(2, copy(), measured(10, 100, false, false)),
        ];
        operations.open_selection(Some(id(1)), &snapshot);
        operations.record(&snapshot, now);
        let view = operations.view(&strings, &English, &snapshot);
        let card = view.card.unwrap();
        assert_eq!(card.status, "Waiting");
        assert_eq!(card.phase.buttons().run, Some(QueueAction::Pause));
        assert!(card.rate.is_empty() && card.files_rate.is_empty());
        assert_eq!(view.rows[0].phase, RowPhase::Running { paused: false });
        assert!(view.rows[0].detail.whole(&strings).starts_with("Waiting"));
        assert_eq!(
            operations.transfer_caption(&strings, &English).unwrap().id,
            id(2)
        );
        operations.record(&snapshot[..1], now);
        let waiting_caption = operations.transfer_caption(&strings, &English).unwrap();
        assert_eq!(waiting_caption.activity, "Copy: Waiting");
        assert_eq!(waiting_caption.pause.unwrap().command, QueueAction::Pause);
        assert!(waiting_caption.byte_rate.is_empty() && waiting_caption.file_rate.is_empty());
        let State::Running(running) = &mut snapshot[0].state else {
            panic!("the selected operation remains running")
        };
        running.paused = true;
        operations.record(&snapshot[..1], now);
        let card = operations
            .view(&strings, &English, &snapshot[..1])
            .card
            .unwrap();
        assert_eq!(card.status, "Paused");
        assert_eq!(card.phase.buttons().run, Some(QueueAction::Resume));
        assert!(matches!(
            file_operations::taskbar(&snapshot[..1]),
            Taskbar::Paused(_)
        ));
        let State::Running(running) = &mut snapshot[0].state else {
            panic!("the selected operation remains running")
        };
        running.yielded = false;
        snapshot[1].state = waiting();
        operations.record(&snapshot, now);
        let caption = operations.transfer_caption(&strings, &English).unwrap();
        assert_eq!(caption.id, id(1));
        assert_eq!(caption.activity, "Copy: Paused");
        assert!(caption.paused);
        assert_eq!(caption.pause.unwrap().command, QueueAction::Resume);
    }

    #[test]
    fn mixed_or_unknown_work_is_indeterminate_and_other_jobs_invalidate_status() {
        use file_operations::{Bar, Measure};
        let mut operations = Operations::default();
        let now = Instant::now();
        let mut snapshot = [
            op(1, copy(), measured(20, 100, true, false)),
            op(2, copy(), measured(10, 100, false, false)),
        ];
        operations.record(&snapshot, now);
        snapshot[1].state = measured(15, 100, false, false);
        assert!(operations.record(&snapshot, now));
        assert_eq!(operations.transfer.as_ref().unwrap().id(), id(2));
        assert_eq!(
            operations.batch.bar(),
            Bar::Measured(Measure {
                done: 35,
                total: 200
            })
        );
        let mut batch = file_operations::batch::Batch::default();
        batch.record(&[
            op(1, copy(), measured(20, 100, true, false)),
            op(3, move_to(), measured(15, 100, false, true)),
        ]);
        assert_eq!(batch.bar(), Bar::Moving);
        batch.record(&[
            op(1, copy(), ended(End::Complete, 100, false)),
            op(3, move_to(), ended(End::Complete, 100, false)),
        ]);
        assert_eq!(batch.bar(), Bar::Measured(Measure { done: 1, total: 1 }));
        batch.record(&[
            op(4, copy(), measured(20, 100, true, false)),
            op(5, move_to(), waiting()),
        ]);
        assert_eq!(batch.bar(), Bar::Moving);
        let mut restored = file_operations::batch::Batch::default();
        restored.record(&[op(3, copy(), State::Interrupted)]);
        restored.record(&[op(3, copy(), measured(10, 100, false, false))]);
        assert_eq!(
            restored.bar(),
            Bar::Measured(Measure {
                done: 10,
                total: 100
            })
        );
    }

    #[test]
    fn a_report_without_file_rows_still_explains_why_the_transfer_stopped() {
        let strings = english();
        let mut operations = Operations::default();
        let failed = [op(
            1,
            copy(),
            ended(End::Refused(Refusal::SourceUnavailable), 0, false),
        )];
        operations.accept(&failed, Instant::now());
        operations.select(Detail::Report(id(1)));
        let report = operations.view(&strings, &English, &[]).report.unwrap();
        assert!(report.outcome.listed.is_empty());
        assert_eq!(
            report.summary,
            "The clipboard or dropped files are no longer available."
        );
        let disk_full = [op(
            2,
            copy(),
            ended(
                End::DiskFull(gozler_fileops::NoRoom {
                    path: PathBuf::from(r"D:\b\large.zip"),
                    size: 20,
                    free: Some(10),
                }),
                0,
                false,
            ),
        )];
        operations.accept(&disk_full, Instant::now());
        operations.select(Detail::Report(id(2)));
        let report = operations.view(&strings, &English, &[]).report.unwrap();
        assert!(report.outcome.listed.is_empty());
        assert!(report.summary.contains("The destination disk is full"));
        assert!(report.summary.contains("large.zip"));
    }

    #[test]
    fn independent_delete_reports_once_while_transfers_stay_inline() {
        let now = Instant::now();
        let mut operations = Operations::default();
        let snapshot = [
            op(1, delete(), ended(End::Complete, 1, true)),
            op(2, copy(), ended(End::Complete, 1, true)),
        ];
        assert_eq!(
            operations.accept(&snapshot, now).dialogs,
            [OperationDialog::Report(id(1))]
        );
        assert!(operations.accept(&snapshot, now).dialogs.is_empty());
        let running = [op(3, copy(), running()), op(4, delete(), running())];
        operations.record(&running, now);
        assert_eq!(operations.status(&running).unwrap().id(), id(4));
        assert_eq!(
            operations
                .transfer_caption(&english(), &English)
                .unwrap()
                .id,
            id(3)
        );
    }

    #[test]
    fn a_finished_cancel_has_no_live_actions_or_folder_in_its_status() {
        let strings = english();
        let mut operations = Operations::default();
        let now = Instant::now();
        let cancelled = [op(1, copy(), ended(End::Cancelled, 0, false))];
        operations.accept(&cancelled, now);
        operations.record(&cancelled, now);
        assert!(operations.transfer_caption(&strings, &English).is_none());
    }

    #[test]
    fn a_prepared_copy_submits_once_without_a_conflict_question() {
        let mut operations = Operations::default();
        let prepared = [op(1, copy(), prepared(2))];
        let first = operations.accept(&prepared, Instant::now());
        assert_eq!(first.queue, [QueueStep::Submit { id: id(1) }]);
        assert!(first.dialogs.is_empty());
        let next = operations.accept(&prepared, Instant::now());
        assert!(next.queue.is_empty() && next.dialogs.is_empty());
    }
}
