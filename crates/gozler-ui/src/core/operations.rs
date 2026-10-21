//! The life of each file operation as the window follows it: when the File
//! operations window opens, which operation asks its conflict question or
//! reports what failed, which one the status bar shows, and when an ended
//! one leaves the queue (DESIGN.md: File operations). The engine's queue
//! owns the operations and their states; `app` reads a snapshot after each
//! of the queue's wakes, and runs the steps that [`Operations::accept`]
//! returns for it.

use super::file_operations::{self, Choices, Failure, LastEnded, StatusOperation, Taskbar};
use super::panel::PanelFollow;
use super::text::{Numbers, Strings};
use gozler_fileops::{Choice, End, Operation, OperationId, Prepared, Request, State};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long an operation runs before the File operations window opens, so
/// a short one shows only in the status bar (DESIGN.md: File operations).
const OPEN_AFTER: Duration = Duration::from_secs(2);

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

/// The person's answer to a conflict question.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Answer {
    Chose(Choice),
    Cancelled,
}

impl Answer {
    /// The answer of the question's link `index`, or of no link.
    pub(crate) fn of(index: Option<usize>) -> Answer {
        (index.and_then(file_operations::conflict_choice)).map_or(Answer::Cancelled, Answer::Chose)
    }
}

/// A call to the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum QueueStep {
    /// Submits the prepared operation with the answer `choice`, or with
    /// `None` when it asked nothing. Once the queue accepts it, the window
    /// calls [`super::model::Model::operation_submitted`].
    Submit {
        id: OperationId,
        choice: Option<Choice>,
    },
    Cancel(OperationId),
    /// Removes an ended operation, which changes the queue's taken entries.
    Remove(OperationId),
}

/// What an operation's [`Source`] hears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SourceStep {
    /// The queue accepted the pasted files' operation; `moved` when it
    /// moves cut files.
    Pasted { id: OperationId, moved: bool },
    /// The operation ended; `succeeded` when it did all it was asked to.
    Ended { id: OperationId, succeeded: bool },
}

/// A dialog of an operation, which waits for `Message::Deferred` because
/// it runs its own message loop (DESIGN.md: window rule 13).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OperationDialog {
    /// The conflict question ([`super::model::Model::operation_question`]).
    Ask(OperationId),
    /// The failure dialog ([`super::model::Model::operation_failure`]).
    Report(OperationId),
}

/// What the File operations window does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WindowStep {
    /// Every operation ended.
    Hide,
    /// It shows, with `select` selected: the first interrupted operation
    /// when the saved ones show at start and none runs.
    Show {
        select: Option<OperationId>,
    },
    /// It shows at this moment, unless it shows already.
    OpenAt(Instant),
    Stay,
}

/// What the window does after a snapshot of the queue
/// ([`super::model::Model::accept_operations`]), in the order of the
/// fields. The sources come last, because a drop's source is another
/// program, and COM can deliver messages while the window waits for it.
#[derive(Debug, PartialEq)]
pub(crate) struct OperationSteps {
    pub(crate) queue: Vec<QueueStep>,
    pub(crate) panels: Vec<PanelFollow>,
    pub(crate) dialogs: Vec<OperationDialog>,
    pub(crate) window: WindowStep,
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
    pub(super) window: WindowStep,
}

/// Where an operation is in its life, as the window follows it.
#[derive(Debug)]
enum Phase {
    /// Nothing waits for the window: the operation prepares, waits, runs or
    /// was interrupted.
    Live,
    /// Its conflict question waits to be asked, or is open.
    Asking {
        request: Arc<Request>,
        prepared: Prepared,
    },
    /// Answered; the queue accepts the submit next.
    Answered { choice: Option<Choice>, moves: bool },
    /// Ended, and the failure dialog waits to show what failed, or shows
    /// it.
    Reporting {
        request: Arc<Request>,
        outcome: Arc<gozler_fileops::Outcome>,
    },
    /// Reported. It took entries, so it stays in the queue while a list
    /// keeps them (fast-copy-delete-spec.md: Taken entries).
    Kept,
    /// Reported, and leaving the queue.
    Removed,
}

impl Phase {
    fn ended(&self) -> bool {
        matches!(self, Phase::Reporting { .. } | Phase::Kept | Phase::Removed)
    }
}

/// Every operation in the queue as the window follows it.
#[derive(Debug, Default)]
pub(crate) struct Operations {
    phases: HashMap<OperationId, Phase>,
    sources: HashMap<OperationId, Source>,
    /// Each operation's [`file_operations::sources_folder`], worked out
    /// once, because the sources never change and reading every one of a
    /// large operation takes long.
    folders: HashMap<OperationId, Option<String>>,
    /// When each preparing, waiting or running operation was first seen
    /// so. One whose conflict question is open has none, so the time the
    /// person takes to answer does not count.
    started: HashMap<OperationId, Instant>,
    /// The operations that had not ended when the person hid the window,
    /// which do not show it again.
    hidden: HashSet<OperationId>,
    /// Every operation of the snapshot before, so an interrupted one shows
    /// the window once.
    known: HashSet<OperationId>,
    /// The operations whose conflict question Keep both answered, whose
    /// status line cannot name an entry's numbered name.
    numbered: HashSet<OperationId>,
    /// The operation reported last, which the status bar shows after the
    /// queue let it go.
    last_ended: Option<LastEnded>,
}

impl Operations {
    /// The queue prepares the operation `id`, whose files came from
    /// `source`.
    pub(super) fn started(&mut self, id: OperationId, source: Source) {
        self.sources.insert(id, source);
    }

    /// Follows `snapshot`, the queue's operations at `now`. A prepared
    /// operation asks its question, or is submitted when it has none, and
    /// an ended one reports what failed, or is reported at once; each only
    /// once.
    pub(super) fn accept(&mut self, snapshot: &[Operation], now: Instant) -> Seen {
        let present: HashSet<OperationId> = snapshot.iter().map(|operation| operation.id).collect();
        self.phases.retain(|id, _| present.contains(id));
        self.folders.retain(|id, _| present.contains(id));
        self.hidden.retain(|id| present.contains(id));
        let last = self.last_ended.as_ref().map(|(id, ..)| *id);
        (self.numbered).retain(|id| present.contains(id) || Some(*id) == last);
        let mut seen = Seen {
            queue: Vec::new(),
            dialogs: Vec::new(),
            sources: Vec::new(),
            ended: Vec::new(),
            window: WindowStep::Stay,
        };
        for operation in snapshot {
            let id = operation.id;
            let request = &operation.request;
            (self.folders.entry(id)).or_insert_with(|| file_operations::sources_folder(request));
            let phase = self.phases.entry(id).or_insert(Phase::Live);
            match &operation.state {
                State::Prepared(prepared) if matches!(phase, Phase::Live) => {
                    if file_operations::asks(request, prepared) {
                        *phase = Phase::Asking {
                            request: Arc::clone(request),
                            prepared: *prepared,
                        };
                        seen.dialogs.push(OperationDialog::Ask(id));
                    } else {
                        *phase = Phase::Answered {
                            choice: None,
                            moves: moves(request),
                        };
                        seen.queue.push(QueueStep::Submit { id, choice: None });
                    }
                }
                State::Ended(outcome) if !phase.ended() => {
                    *phase = Phase::Reporting {
                        request: Arc::clone(request),
                        outcome: Arc::clone(outcome),
                    };
                    seen.ended.push((id, Arc::clone(request)));
                    if self.sources.remove(&id).is_some() {
                        let succeeded = outcome.end == End::Complete && outcome.listed.is_empty();
                        seen.sources.push(SourceStep::Ended { id, succeeded });
                    }
                    if file_operations::reports(outcome) {
                        seen.dialogs.push(OperationDialog::Report(id));
                    } else {
                        seen.queue.extend(self.reported(id));
                    }
                }
                _ => {}
            }
        }
        seen.window = self.window_step(snapshot, present, now);
        seen
    }

    /// The File operations window opens when an operation still runs
    /// [`OPEN_AFTER`] after it started, so a short one shows only in the
    /// status bar, and at once for the interrupted operations of the last
    /// run; it hides when the last one ends (DESIGN.md: File operations).
    fn window_step(
        &mut self,
        snapshot: &[Operation],
        present: HashSet<OperationId>,
        now: Instant,
    ) -> WindowStep {
        let live: HashSet<OperationId> = (snapshot.iter())
            .filter(|operation| {
                matches!(
                    operation.state,
                    State::Preparing | State::Waiting { .. } | State::Running(_)
                )
            })
            .map(|operation| operation.id)
            .collect();
        self.started.retain(|id, _| live.contains(id));
        for id in live {
            self.started.entry(id).or_insert(now);
        }
        let interrupted = (snapshot.iter())
            .filter(|operation| matches!(operation.state, State::Interrupted))
            .map(|operation| operation.id);
        let shown_again = interrupted.clone().any(|id| !self.known.contains(&id));
        self.known = present;
        if snapshot
            .iter()
            .all(|operation| matches!(operation.state, State::Ended(_)))
        {
            return WindowStep::Hide;
        }
        if shown_again {
            let running =
                (snapshot.iter()).any(|operation| matches!(operation.state, State::Running(_)));
            return WindowStep::Show {
                select: interrupted.clone().next().filter(|_| !running),
            };
        }
        let due = (self.started.iter())
            .filter(|(id, _)| !self.hidden.contains(id))
            .map(|(_, at)| *at + OPEN_AFTER)
            .min();
        match due {
            Some(due) if due <= now => WindowStep::Show { select: None },
            Some(due) => WindowStep::OpenAt(due),
            None => WindowStep::Stay,
        }
    }

    /// The person hid the window, so the operations of `snapshot` that
    /// have not ended do not show it again, also one whose conflict
    /// question is open, which starts after the answer; a later one does.
    pub(super) fn hide(&mut self, snapshot: &[Operation]) {
        let unended = (snapshot.iter())
            .filter(|operation| !matches!(operation.state, State::Ended(_)))
            .map(|operation| operation.id);
        self.hidden.extend(unended);
    }

    /// The conflict question of `id`, while it waits to be asked.
    pub(super) fn question(
        &self,
        id: OperationId,
        strings: &Strings,
        numbers: &dyn Numbers,
    ) -> Option<Choices> {
        let Some(Phase::Asking { request, prepared }) = self.phases.get(&id) else {
            return None;
        };
        let folder = self.folders.get(&id).and_then(Option::as_deref);
        file_operations::conflict_question(strings, numbers, request, prepared, folder)
    }

    /// The person answered the question of `id`: it is submitted, or
    /// cancelled. `None` when it no longer waits for an answer, as when the
    /// person cancelled it in the File operations window meanwhile.
    pub(super) fn answer(&mut self, id: OperationId, answer: Answer) -> Option<QueueStep> {
        let phase = self.phases.get_mut(&id)?;
        let Phase::Asking { request, .. } = phase else {
            return None;
        };
        Some(match answer {
            Answer::Chose(choice) => {
                *phase = Phase::Answered {
                    choice: Some(choice),
                    moves: moves(request),
                };
                QueueStep::Submit {
                    id,
                    choice: Some(choice),
                }
            }
            Answer::Cancelled => {
                *phase = Phase::Live;
                QueueStep::Cancel(id)
            }
        })
    }

    /// The queue accepted the submit of `id`. A cut paste's files count as
    /// moved now, because a move on one drive can end before any snapshot
    /// shows it running.
    pub(super) fn submitted(&mut self, id: OperationId) -> Option<SourceStep> {
        let phase = self.phases.get_mut(&id)?;
        let Phase::Answered { choice, moves } = *phase else {
            return None;
        };
        *phase = Phase::Live;
        if choice == Some(Choice::KeepBoth) {
            self.numbered.insert(id);
        }
        let Some(Source::Paste { cut }) = self.sources.get(&id).copied() else {
            return None;
        };
        self.sources.remove(&id);
        Some(SourceStep::Pasted {
            id,
            moved: cut && moves,
        })
    }

    /// The failure dialog of `id`, while it waits to show.
    pub(super) fn failure(
        &self,
        id: OperationId,
        strings: &Strings,
        numbers: &dyn Numbers,
        windows_text: &dyn Fn(u32) -> String,
    ) -> Option<Failure> {
        let Some(Phase::Reporting { request, outcome }) = self.phases.get(&id) else {
            return None;
        };
        file_operations::failure(strings, numbers, request, outcome, windows_text)
    }

    /// The window reported the ended operation `id`. It leaves the queue,
    /// unless it took entries, which a list may still show blank.
    pub(super) fn reported(&mut self, id: OperationId) -> Option<QueueStep> {
        let phase = self.phases.get_mut(&id)?;
        let Phase::Reporting { request, outcome } = phase else {
            return None;
        };
        let took =
            outcome.items > 0 && matches!(**request, Request::Delete { .. } | Request::Move { .. });
        if !file_operations::did_nothing(outcome) {
            self.last_ended = Some((id, Arc::clone(request), Arc::clone(outcome)));
        }
        if took {
            *phase = Phase::Kept;
            None
        } else {
            *phase = Phase::Removed;
            Some(QueueStep::Remove(id))
        }
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

    /// The operation the status bar shows ([`StatusOperation::of`]).
    pub(super) fn status(&self, snapshot: &[Operation]) -> Option<StatusOperation> {
        let reported: HashSet<OperationId> = (self.phases.iter())
            .filter(|(_, phase)| matches!(phase, Phase::Kept | Phase::Removed))
            .map(|(id, _)| *id)
            .collect();
        StatusOperation::of(
            snapshot,
            self.last_ended.as_ref(),
            &reported,
            &self.numbered,
        )
    }

    /// Each operation's [`file_operations::sources_folder`], which the File
    /// operations window shows.
    pub(super) fn folders(&self) -> HashMap<OperationId, Option<String>> {
        self.folders.clone()
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
        Conflicts, Destination, Listed, Outcome, Reason, Refusal, RingCounts, Running, Wait,
    };
    use std::path::PathBuf;

    fn id(n: u64) -> OperationId {
        OperationId::for_tests(n)
    }

    fn op(n: u64, request: Request, state: State) -> Operation {
        Operation {
            id: id(n),
            request: Arc::new(request),
            state,
        }
    }

    fn copy() -> Request {
        Request::Copy {
            sources: vec![PathBuf::from(r"C:\f\a")],
            destination: Destination::Folder(PathBuf::from(r"D:\b")),
        }
    }

    fn move_to() -> Request {
        Request::Move {
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
            items: 0,
            bytes: 0,
            passed: 0,
            total: None,
            renames: false,
            paused: false,
            current: None,
        })
    }

    fn waiting() -> State {
        State::Waiting {
            on: id(99),
            reason: Wait::Paths,
            total: None,
        }
    }

    /// Prepared, with `conflicts` files at the destination.
    fn prepared(conflicts: u64) -> State {
        State::Prepared(Prepared {
            entries: 1,
            total: None,
            conflicts: Some(Conflicts {
                files: conflicts,
                ..Conflicts::default()
            }),
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
            end,
            items,
            bytes: 0,
            listed: if failed { vec![listed] } else { Vec::new() },
            ring: RingCounts::default(),
        }))
    }

    fn numbered(status: Option<StatusOperation>) -> Option<bool> {
        match status? {
            StatusOperation::Ended { numbered, .. } => Some(numbered),
            StatusOperation::Running { .. } => None,
        }
    }

    /// A short operation shows only in the status bar; a window that opened
    /// for it would flash in front of the person.
    #[test]
    fn an_operation_that_ends_within_2_seconds_never_opens_the_window() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut operations = Operations::default();
        let window = |operations: &mut Operations, state, ms| {
            operations.accept(&[op(1, copy(), state)], at(ms)).window
        };
        assert_eq!(
            window(&mut operations, State::Preparing, 0),
            WindowStep::OpenAt(at(2_000))
        );
        assert_eq!(
            window(&mut operations, running(), 1_000),
            WindowStep::OpenAt(at(2_000))
        );
        assert_eq!(
            window(&mut operations, ended(End::Complete, 1, false), 1_500),
            WindowStep::Hide
        );
    }

    /// The person may take minutes to answer the conflict question, and a
    /// window that opened as soon as they answered would cover the one
    /// they just used.
    #[test]
    fn the_time_a_question_is_open_does_not_count() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut operations = Operations::default();
        operations.accept(&[op(1, copy(), State::Preparing)], at(0));
        let seen = operations.accept(&[op(1, copy(), prepared(1))], at(1_000));
        assert_eq!(seen.dialogs, [OperationDialog::Ask(id(1))]);
        assert_eq!(seen.window, WindowStep::Stay);
        operations.answer(id(1), Answer::Chose(Choice::Skip));
        operations.submitted(id(1));
        assert_eq!(
            operations
                .accept(&[op(1, copy(), waiting())], at(9_000))
                .window,
            WindowStep::OpenAt(at(11_000))
        );
    }

    /// Hide is the person's choice for the operations they saw; only an
    /// operation they have not seen yet shows the window again.
    #[test]
    fn hide_keeps_the_current_operations_hidden_and_a_later_one_shows() {
        let t0 = Instant::now();
        let at = |ms| t0 + Duration::from_millis(ms);
        let mut operations = Operations::default();
        let first = [op(1, copy(), running())];
        operations.accept(&first, at(0));
        operations.hide(&first);
        assert_eq!(
            operations.accept(&first, at(3_000)).window,
            WindowStep::Stay
        );
        let both = [op(1, copy(), running()), op(2, copy(), State::Preparing)];
        assert_eq!(
            operations.accept(&both, at(3_000)).window,
            WindowStep::OpenAt(at(5_000))
        );
        assert_eq!(
            operations.accept(&both, at(5_000)).window,
            WindowStep::Show { select: None }
        );
    }

    /// The interrupted operations of the last run wait for Continue or
    /// Dismiss, which show only for the selected row, so the window opens
    /// at once with the first one selected, and only once.
    #[test]
    fn interrupted_operations_show_the_window_at_start_with_the_first_selected() {
        let mut operations = Operations::default();
        let saved = [
            op(1, copy(), State::Interrupted),
            op(2, delete(), State::Interrupted),
        ];
        assert_eq!(
            operations.accept(&saved, Instant::now()).window,
            WindowStep::Show {
                select: Some(id(1))
            }
        );
        assert_eq!(
            operations.accept(&saved, Instant::now()).window,
            WindowStep::Stay
        );
    }

    /// The status bar shows the running operation, else the newest that
    /// ended and is not reported yet, whose dialog is about to say what
    /// failed, else the one reported last, which after Hide is the only
    /// sign that it finished; one cancelled before it did anything counts
    /// as neither.
    #[test]
    fn the_status_shows_running_then_unreported_then_last_reported() {
        let mut operations = Operations::default();
        let ended_two = [
            op(1, copy(), ended(End::Complete, 1, true)),
            op(2, copy(), ended(End::Complete, 1, false)),
        ];
        operations.accept(&ended_two, Instant::now());
        let shown = |operations: &Operations, snapshot: &[Operation]| {
            operations.status(snapshot).map(|status| status.id())
        };
        assert_eq!(shown(&operations, &ended_two), Some(id(1)));
        let with_running = [
            op(1, copy(), ended(End::Complete, 1, true)),
            op(3, copy(), running()),
        ];
        assert_eq!(shown(&operations, &with_running), Some(id(3)));
        operations.reported(id(1));
        assert_eq!(shown(&operations, &[]), Some(id(1)));
        // Cancel at a conflict question did nothing, so it leaves the line
        // of the operation before it (#111).
        let cancelled = [op(4, copy(), ended(End::Cancelled, 0, false))];
        operations.accept(&cancelled, Instant::now());
        assert_eq!(shown(&operations, &cancelled), Some(id(1)));
        operations.reported(id(4));
        assert_eq!(shown(&operations, &[]), Some(id(1)));
    }

    /// Keep both may give an entry a numbered name that the window does not
    /// know, so only a Keep both that the queue accepted keeps the status
    /// line from naming the entry.
    #[test]
    fn keep_both_counts_only_once_the_queue_accepts_the_submit() {
        let mut operations = Operations::default();
        let asked = [op(1, copy(), prepared(1)), op(2, copy(), prepared(1))];
        operations.accept(&asked, Instant::now());
        for n in [1, 2] {
            operations.answer(id(n), Answer::Chose(Choice::KeepBoth));
        }
        operations.submitted(id(2));
        let first = [
            op(1, copy(), ended(End::Complete, 1, false)),
            op(2, copy(), waiting()),
        ];
        operations.accept(&first, Instant::now());
        assert_eq!(numbered(operations.status(&first)), Some(false));
        let second = [op(2, copy(), ended(End::Complete, 1, false))];
        operations.accept(&second, Instant::now());
        assert_eq!(numbered(operations.status(&second)), Some(true));
    }

    /// The clipboard forgets cut files that moved; told before the queue
    /// accepted the move, it would forget files that are still there.
    #[test]
    fn a_cut_paste_moved_only_once_its_submit_is_accepted() {
        let mut operations = Operations::default();
        operations.started(id(1), Source::Paste { cut: true });
        let seen = operations.accept(&[op(1, move_to(), prepared(0))], Instant::now());
        assert_eq!(
            seen.queue,
            [QueueStep::Submit {
                id: id(1),
                choice: None
            }]
        );
        assert!(seen.sources.is_empty());
        assert_eq!(
            operations.submitted(id(1)),
            Some(SourceStep::Pasted {
                id: id(1),
                moved: true
            })
        );
        operations.started(id(2), Source::Paste { cut: true });
        operations.accept(&[op(2, move_to(), prepared(0))], Instant::now());
        let cancelled = [op(2, move_to(), ended(End::Cancelled, 0, false))];
        assert_eq!(
            operations.accept(&cancelled, Instant::now()).sources,
            [SourceStep::Ended {
                id: id(2),
                succeeded: false
            }]
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
        assert_eq!(
            operations.answer(id(1), Answer::Cancelled),
            Some(QueueStep::Cancel(id(1)))
        );
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

    /// The failure dialog reads its operation's outcome, and a delete that
    /// left the queue while its dialog waited would be reported to nobody.
    #[test]
    fn an_operation_leaves_the_queue_only_after_its_report() {
        let mut operations = Operations::default();
        let failed = [op(1, delete(), ended(End::Complete, 3, true))];
        let seen = operations.accept(&failed, Instant::now());
        assert_eq!(seen.dialogs, [OperationDialog::Report(id(1))]);
        assert!(seen.queue.is_empty());
        assert_eq!(operations.kept().count(), 0);
        assert_eq!(operations.reported(id(1)), None);
        assert_eq!(operations.release(&[id(1)]), [QueueStep::Remove(id(1))]);
    }

    /// Each wake while the question is open reads the operation prepared
    /// again; a second question would stack over the first.
    #[test]
    fn a_question_is_asked_once() {
        let strings = english();
        let mut operations = Operations::default();
        let asked = [op(1, copy(), prepared(2))];
        assert_eq!(
            operations.accept(&asked, Instant::now()).dialogs,
            [OperationDialog::Ask(id(1))]
        );
        assert!(operations.accept(&asked, Instant::now()).dialogs.is_empty());
        assert!(operations.question(id(1), &strings, &English).is_some());
        assert_eq!(
            operations.answer(id(1), Answer::Chose(Choice::Replace)),
            Some(QueueStep::Submit {
                id: id(1),
                choice: Some(Choice::Replace)
            })
        );
        assert!(operations.question(id(1), &strings, &English).is_none());
        assert_eq!(operations.answer(id(1), Answer::Cancelled), None);
    }
}
