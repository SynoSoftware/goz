//! Serializes lifecycle decisions with received-row evidence. UI readers keep
//! the last immutable presentation while the canonical follower is on a worker.

use super::*;
use crate::core::operations::{
    FollowChange, Followed, Following, Keepers, Operations, Presentation, Release,
};
use crate::core::panel::EndReceipt;
use crate::core::search_list::EndEvidence;
use crate::core::work::INLINE_ITEMS;
use std::collections::LinkedList;

struct Receipt {
    searches: [Option<EndEvidence>; 3],
    panels: EndReceipt,
    now: Instant,
    keepers: Option<Keepers>,
    job_moment: u64,
}

pub(super) struct OperationFollow {
    state: Option<Operations>,
    snapshot: Arc<[Operation]>,
    taken: Arc<Taken>,
    changes: LinkedList<FollowChange>,
    receipt: Option<Receipt>,
    presentation: Option<Arc<Presentation>>,
    presentation_version: u64,
    dirty: bool,
    release_requested: bool,
    release: Option<Release>,
    version: u64,
    reported: std::collections::HashSet<OperationId>,
}

impl OperationFollow {
    pub(super) fn new() -> Self {
        Self {
            state: Some(Operations::default()),
            snapshot: Arc::from([]),
            taken: Arc::default(),
            changes: LinkedList::new(),
            receipt: None,
            presentation: None,
            presentation_version: 0,
            dirty: false,
            release_requested: false,
            release: None,
            version: 0,
            reported: Default::default(),
        }
    }

    fn change(&mut self, change: FollowChange) {
        self.changes.push_back(change);
        self.dirty = true;
        self.version += 1;
    }
}

impl Model {
    pub(crate) fn can_receive_operations(&self) -> bool {
        self.following.state.is_some() && self.following.receipt.is_none()
    }

    fn capture_operation_receipt(&mut self, now: Instant) -> Receipt {
        let keepers = self.retention_ready().then(|| self.keepers());
        let searches = std::array::from_fn(|at| {
            self.list_mut(List::ALL[at])
                .and_then(SearchList::observe_end)
        });
        let panels = self.panels.observe_operations();
        Receipt {
            searches,
            panels,
            now,
            keepers,
            job_moment: self.next_job_moment(),
        }
    }

    pub(crate) fn observe_operations(
        &mut self,
        snapshot: Arc<[Operation]>,
        taken: Arc<Taken>,
        now: Instant,
    ) {
        assert!(
            self.can_receive_operations(),
            "the previous follower answer is accepted before the next receipt"
        );
        if Arc::ptr_eq(&self.following.snapshot, &snapshot) {
            return;
        }
        let receipt = self.capture_operation_receipt(now);
        self.following.receipt = Some(receipt);
        let old = std::mem::replace(&mut self.following.snapshot, snapshot);
        self.retired.push(old, INLINE_ITEMS + 1);
        let old = std::mem::replace(&mut self.following.taken, taken);
        self.retired.push(old, INLINE_ITEMS + 1);
        self.following.dirty = true;
    }

    fn retention_ready(&self) -> bool {
        self.following.receipt.is_none()
            && List::ALL
                .into_iter()
                .all(|list| self.list(list).is_none_or(|shown| !shown.is_keeping()))
            && Side::BOTH
                .into_iter()
                .all(|side| !self.panels.panel(side).is_calculating())
    }

    fn keepers(&self) -> Keepers {
        Keepers {
            searches: std::array::from_fn(|at| {
                self.list(List::ALL[at])
                    .map(|shown| Arc::clone(&shown.kept))
            }),
            panels: self.panels.kept_rows(),
        }
    }

    fn keepers_current(&self, keepers: &Keepers) -> bool {
        self.retention_ready()
            && keepers.panels.matches(&self.panels)
            && List::ALL.into_iter().enumerate().all(|(at, list)| {
                match (&keepers.searches[at], self.list(list)) {
                    (Some(kept), Some(shown)) => Arc::ptr_eq(kept, &shown.kept),
                    (None, None) => true,
                    _ => false,
                }
            })
    }

    pub(crate) fn take_following(&mut self) -> Option<Following> {
        let release = self.following.release_requested && self.retention_ready();
        if self.following.state.is_none() || !(self.following.dirty || release) {
            return None;
        }
        let keepers = self
            .following
            .receipt
            .as_mut()
            .and_then(|receipt| receipt.keepers.take())
            .or_else(|| release.then(|| self.keepers()));
        let calculating_release = keepers.is_some();
        if let Some(old) = self.following.release.take() {
            self.retired.push(old, INLINE_ITEMS + 1);
            self.following.release_requested = true;
        }
        if calculating_release {
            self.following.release_requested = false;
        }
        self.following.dirty = false;
        let job_moment = match &self.following.receipt {
            Some(receipt) => receipt.job_moment,
            None => self.next_job_moment(),
        };
        Some(Following {
            operations: self.following.state.take().expect("the follower is ready"),
            snapshot: Arc::clone(&self.following.snapshot),
            taken: Arc::clone(&self.following.taken),
            received: self.following.receipt.is_some(),
            changes: std::mem::take(&mut self.following.changes),
            prior: self.operation.clone(),
            prior_view: self
                .following
                .presentation
                .as_ref()
                .map(|shown| Arc::clone(&shown.view)),
            now: self
                .following
                .receipt
                .as_ref()
                .map_or_else(Instant::now, |receipt| receipt.now),
            strings: self.strings.clone(),
            keepers,
            version: self.following.version,
            job_moment,
        })
    }

    fn accept_operation_ends(
        &mut self,
        receipt: Receipt,
        ended: Option<Arc<Ended>>,
        has_ended: impl Fn(OperationId) -> bool,
    ) -> Vec<PanelFollow> {
        for (list, evidence) in List::ALL.into_iter().zip(receipt.searches) {
            if let Some(evidence) = evidence {
                if let Some(shown) = self.list_mut(list) {
                    shown.accept_end(evidence, ended.as_ref().map(Arc::clone));
                    self.refilter_selection(list);
                } else {
                    let items = evidence.items();
                    self.retired.push(evidence, items);
                }
            }
        }
        let panels = self.panels.accept_operations(
            receipt.panels,
            ended.as_ref().map(Arc::clone),
            has_ended,
        );
        if let Some(ended) = ended {
            let items = ended.items();
            self.retired.push(ended, items);
        }
        panels
    }

    pub(crate) fn accept_followed(&mut self, answer: Followed) -> OperationSteps {
        assert!(
            self.following.state.is_none(),
            "one pass owns the canonical follower"
        );
        let Followed {
            operations,
            presentation,
            ended,
            seen,
            release,
            version,
            job_moment,
        } = answer;
        self.following.state = Some(operations);
        self.set_operation_at(presentation.status.clone(), job_moment);
        let panels = match self.following.receipt.take() {
            Some(receipt) => {
                self.following.release_requested |= ended.is_some() || release.is_none();
                self.accept_operation_ends(receipt, ended, |id| presentation.has_ended(id))
            }
            None => Vec::new(),
        };
        let progress = presentation.progress;
        if version == self.following.version {
            let reported = std::mem::take(&mut self.following.reported);
            self.retired.push(reported, INLINE_ITEMS + 1);
        }
        if let Some(old) = self.following.presentation.replace(presentation) {
            self.retired.push(old, INLINE_ITEMS + 1);
        }
        self.following.presentation_version = version;
        if let Some(old) = std::mem::replace(&mut self.following.release, release) {
            self.retired.push(old, INLINE_ITEMS + 1);
        }
        let mut queue = seen.queue;
        queue.extend(self.next_operation_release());
        OperationSteps {
            queue,
            panels,
            dialogs: seen.dialogs,
            progress,
            sources: seen.sources,
        }
    }

    pub(crate) fn next_operation_release(&mut self) -> Vec<QueueStep> {
        if self.following.state.is_none() || self.following.dirty {
            return Vec::new();
        }
        let Some(mut release) = self.following.release.take() else {
            return Vec::new();
        };
        if !self.keepers_current(&release.keepers) {
            self.retired.push(release, INLINE_ITEMS + 1);
            self.following.release_requested = true;
            return Vec::new();
        }
        let end = release
            .next
            .saturating_add(INLINE_ITEMS)
            .min(release.ids.len());
        let operations = self
            .following
            .state
            .as_mut()
            .expect("release runs only while the follower is ready");
        let steps = operations.release(&release.ids[release.next..end]);
        release.next = end;
        if end < release.ids.len() {
            self.following.release = Some(release);
        } else {
            self.retired.push(release, INLINE_ITEMS + 1);
        }
        steps
    }

    pub(crate) fn has_operation_release(&self) -> bool {
        self.following.release.is_some()
    }

    pub(crate) fn release_operations(&mut self) -> Vec<QueueStep> {
        if self.following.release.is_some() {
            self.next_operation_release()
        } else {
            self.following.release_requested = true;
            Vec::new()
        }
    }

    pub(crate) fn operation_text_changed(&mut self) {
        self.following.version += 1;
        self.following.dirty = true;
    }

    pub(crate) fn file_operation_started(&mut self, id: OperationId, source: Option<Source>) {
        if let Some(source) = source {
            self.following.change(FollowChange::Started(id, source));
        }
    }

    pub(crate) fn operation_submitted(&mut self, id: OperationId) {
        self.following.change(FollowChange::Submitted(id));
    }
    pub(crate) fn operation_reported(&mut self, id: OperationId) {
        self.following.reported.insert(id);
        self.following.change(FollowChange::Reported(id));
        self.following.release_requested = true;
    }

    pub(crate) fn operation_report(&self, id: OperationId) -> Option<Operation> {
        if self.following.reported.contains(&id) {
            return None;
        }
        self.following.presentation.as_ref()?.report(id).cloned()
    }
    pub(crate) fn operation_outcome(
        &self,
        id: OperationId,
    ) -> Option<Arc<gozler_fileops::Outcome>> {
        if self.following.reported.contains(&id) {
            return None;
        }
        self.following.presentation.as_ref()?.outcome(id)
    }
    pub(crate) fn operation_view(&self) -> Option<Arc<file_operations::View>> {
        if self.following.presentation_version != self.following.version {
            return None;
        }
        Some(Arc::clone(&self.following.presentation.as_ref()?.view))
    }
    pub(crate) fn operation_is_transfer(&self, id: OperationId) -> bool {
        self.following
            .presentation
            .as_ref()
            .is_some_and(|shown| shown.is_transfer(id))
    }

    pub(crate) fn transfer_caption(&self) -> Option<super::super::status_transfer::Caption> {
        let mut caption = self.following.presentation.as_ref()?.caption.clone()?;
        let key = if self.inspector_content == super::super::layout::InspectorContent::Transfers
            && self.settings.window.inspector
        {
            "file_operations.hide_tip"
        } else {
            "file_operations.show_tip"
        };
        caption.summary_tip = self.strings.text(key).to_owned();
        Some(caption)
    }

    pub(crate) fn operation_backlog(&self) -> &str {
        self.following
            .presentation
            .as_ref()
            .map_or("", |shown| shown.backlog.as_str())
    }
    pub(crate) fn open_transfer_selection(&mut self, select: Option<OperationId>) {
        self.following.change(FollowChange::Open(select));
    }
    pub(crate) fn select_operation(&mut self, detail: file_operations::Detail) {
        self.following.change(FollowChange::Select(detail));
    }
    pub(crate) fn close_operation_report(&mut self) {
        self.following.change(FollowChange::CloseReport);
    }

    pub(crate) fn retire_following(&mut self) {
        let old = std::mem::replace(&mut self.following, OperationFollow::new());
        self.retired.push(old, INLINE_ITEMS + 1);
        let status = self.operation.take();
        self.retired.push(status, INLINE_ITEMS + 1);
    }

    #[cfg(test)]
    pub(super) fn operations_ended(&mut self, ended: Arc<Ended>) -> Vec<PanelFollow> {
        let receipt = self.capture_operation_receipt(Instant::now());
        let ids: std::collections::HashSet<_> =
            ended.operations.iter().map(|(id, _)| *id).collect();
        self.accept_operation_ends(receipt, Some(ended), |id| ids.contains(&id))
    }
}
