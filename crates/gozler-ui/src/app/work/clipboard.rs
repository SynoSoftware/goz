//! Clipboard publication retains its original selection and bounded retries.

use crate::core::clipboard::{ClipboardData, ClipboardFormat, Publication};
use crate::core::work::{Id, Replacement, Retirement, Waiting};
pub(in crate::app) use crate::win::clipboard::CopyFailure;
use crate::win::clipboard::{self, ClipboardBlock, CopyAttempt};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use gozler_core::proto::ResultItem;
use std::sync::Arc;
use std::sync::atomic::AtomicU32;
use std::time::{Duration, Instant};

struct Copy {
    data: CopyData,
    publication: Publication,
    preceding: Arc<AtomicU32>,
}

enum CopyData {
    Rows(Arc<[ResultItem]>, ClipboardFormat),
    Block(ClipboardBlock),
}

impl Copy {
    fn items(&self) -> usize {
        match &self.data {
            CopyData::Rows(files, _) => files.len(),
            CopyData::Block(block) => block.items(),
        }
    }
}

enum Copied {
    Finished,
    Busy(Copy),
    Failed(windows::core::Error),
}

struct CopyRetry {
    id: Id,
    copy: Copy,
    due: Instant,
}

pub(in crate::app) enum CopyNotice {
    Retry,
    Failed(Id, CopyFailure),
    Finished,
}

pub(in crate::app) struct Work {
    task: Task<Copy, Copied>,
    retry: Option<CopyRetry>,
    last_publication: Arc<AtomicU32>,
    retired: Retirement,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            task: Task::new(waker.clone(), Replacement::Cancel, copy).with_size(Copy::items),
            retry: None,
            last_publication: Arc::new(AtomicU32::new(0)),
            retired: Retirement::default(),
        }
    }

    pub(in crate::app) fn copy(&mut self, files: Arc<[ResultItem]>, format: ClipboardFormat) {
        self.clear_copy_retry();
        self.task.replace(Copy {
            data: CopyData::Rows(files, format),
            publication: Publication::new(clipboard::sequence()),
            preceding: Arc::clone(&self.last_publication),
        });
        self.retire_copies();
    }

    pub(in crate::app) fn cancel(&mut self) {
        self.clear_copy_retry();
        self.task.cancel();
        self.retire_copies();
    }

    pub(in crate::app) fn retry(&mut self) -> Option<Duration> {
        let retry = self.retry.as_ref()?;
        if !self.task.is_current(retry.id) {
            self.clear_copy_retry();
            return None;
        }
        let left = retry.due.saturating_duration_since(Instant::now());
        if !left.is_zero() {
            return Some(left);
        }
        let retry = self.retry.take().expect("the current retry is due");
        self.task.replace(retry.copy);
        self.retire_copies();
        None
    }

    pub(in crate::app) fn take(&mut self) -> Option<CopyNotice> {
        let answer = self.task.take();
        self.retire_copies();
        let (id, answer) = answer?;
        Some(match answer {
            Copied::Finished => CopyNotice::Finished,
            Copied::Failed(error) => CopyNotice::Failed(id, CopyFailure::Native(error)),
            Copied::Busy(mut copy) => {
                if copy.publication.retry() {
                    self.retry = Some(CopyRetry {
                        id,
                        copy,
                        due: Instant::now() + crate::core::clipboard::RETRY_AFTER,
                    });
                    CopyNotice::Retry
                } else {
                    self.retire_copy(copy);
                    CopyNotice::Failed(id, CopyFailure::Busy)
                }
            }
        })
    }

    pub(in crate::app) fn is_current(&self, id: Id) -> bool {
        self.task.is_current(id)
    }

    fn clear_copy_retry(&mut self) {
        if let Some(retry) = self.retry.take() {
            self.retire_copy(retry.copy);
        }
    }

    fn retire_copy(&mut self, copy: Copy) {
        let items = copy.items();
        let mut retired = Retirement::default();
        retired.push(copy, items);
        self.retired.append(retired);
    }

    fn retire_copies(&mut self) {
        let retired = self.task.take_retired();
        self.retired.append(retired);
    }

    pub(super) fn waiting(&self, now: Instant) -> Option<Waiting> {
        self.task.waiting(now)
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        self.task.tick(now).is_some()
    }

    pub(super) fn take_retired(&mut self) -> Retirement {
        self.retired.append(self.task.take_retired());
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(&mut self) -> Retirement {
        self.cancel();
        self.retired.append(self.task.close_delivery());
        self.take_retired()
    }
}

fn copy(copy: Copy, worker: &OffThread, cancel: &CancelToken) -> Copied {
    if cancel.is_cancelled() {
        return Copied::Finished;
    }
    let block = match copy.data {
        CopyData::Rows(files, format) => {
            let Some(block) = ClipboardBlock::new(&ClipboardData::new(format, &files)) else {
                return Copied::Finished;
            };
            block
        }
        CopyData::Block(block) => block,
    };
    match block.set(&copy.publication, &copy.preceding, cancel, worker) {
        CopyAttempt::Published | CopyAttempt::Superseded => Copied::Finished,
        CopyAttempt::Busy(block) => Copied::Busy(Copy {
            data: CopyData::Block(block),
            publication: copy.publication,
            preceding: copy.preceding,
        }),
        CopyAttempt::Failed(error) => Copied::Failed(error),
    }
}
