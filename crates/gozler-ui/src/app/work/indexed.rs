//! Indexed folders has one listing slot per volume. A blocked old dialog
//! retains its slot; reopening queues work behind it instead of another thread.

use crate::core::work::{INLINE_ITEMS, Interest, Replacement, Retirement, Waiting};
use crate::win::wake::OpenWaker;
use crate::win::work::{OffThread, Task};
use gozler_client::CancelToken;
use std::collections::{BTreeMap, HashSet, LinkedList, VecDeque};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct Session(u64);

#[derive(Clone)]
struct Reading {
    session: Session,
    node: usize,
    folder: String,
    accepted: Instant,
}

pub(in crate::app) type Subfolders = Vec<(String, bool)>;

pub(in crate::app) struct Listed {
    pub(in crate::app) node: usize,
    pub(in crate::app) folder: String,
    pub(in crate::app) folders: Subfolders,
    session: Session,
}

struct Volume {
    task: Task<Reading, Listed>,
    active: Option<Reading>,
    queued: VecDeque<Reading>,
    asked: HashSet<usize>,
}

impl Volume {
    fn start(&mut self) {
        if !self.task.is_running()
            && let Some(reading) = self.queued.pop_front()
        {
            self.active = Some(reading.clone());
            self.task.replace(reading);
        }
    }
}

pub(in crate::app) struct Work {
    waker: OpenWaker,
    volumes: BTreeMap<String, Volume>,
    current: Option<Session>,
    next: u64,
    listed: LinkedList<Listed>,
    retired: Retirement,
    closed: bool,
}

impl Work {
    pub(super) fn new(waker: OpenWaker) -> Self {
        Self {
            waker,
            volumes: BTreeMap::new(),
            current: None,
            next: 0,
            listed: LinkedList::new(),
            retired: Retirement::default(),
            closed: false,
        }
    }

    pub(in crate::app) fn begin(&mut self) -> Session {
        if let Some(session) = self.current {
            self.end(session);
        }
        self.next = self.next.wrapping_add(1);
        let session = Session(self.next);
        if !self.closed {
            self.current = Some(session);
        }
        session
    }

    pub(in crate::app) fn list(
        &mut self,
        session: Session,
        node: usize,
        volume: &str,
        folder: String,
    ) {
        if self.current != Some(session) || self.closed {
            return;
        }
        let group = self
            .volumes
            .entry(volume.to_owned())
            .or_insert_with(|| Volume {
                task: Task::new(self.waker.clone(), Replacement::Finish, read)
                    .with_size(|_| INLINE_ITEMS + 1),
                active: None,
                queued: VecDeque::new(),
                asked: HashSet::new(),
            });
        if group.asked.insert(node) {
            group.queued.push_back(Reading {
                session,
                node,
                folder,
                accepted: Instant::now(),
            });
            group.start();
        }
    }

    pub(in crate::app) fn poll(&mut self) {
        for group in self.volumes.values_mut() {
            if let Some((_, listed)) = group.task.take() {
                if self.current == Some(listed.session) {
                    self.listed.push_back(listed);
                } else {
                    self.retired.push(listed, INLINE_ITEMS + 1);
                }
            }
            if !group.task.is_running() {
                group.active = None;
                group.start();
            }
            self.retired.append(group.task.take_retired());
        }
    }

    pub(in crate::app) fn take(&mut self, session: Session) -> Vec<Listed> {
        self.poll();
        if self.current != Some(session) {
            return Vec::new();
        }
        let mut listed = Vec::new();
        for _ in 0..INLINE_ITEMS {
            let Some(next) = self.listed.pop_front() else {
                break;
            };
            listed.push(next);
        }
        listed
    }

    pub(in crate::app) fn pending(&self, session: Session) -> bool {
        self.current == Some(session)
            && (!self.listed.is_empty()
                || self.volumes.values().any(|group| {
                    group
                        .active
                        .as_ref()
                        .is_some_and(|active| active.session == session)
                        || !group.queued.is_empty()
                }))
    }

    pub(in crate::app) fn waiting(
        &self,
        session: Session,
        now: Instant,
    ) -> Option<(Waiting, String)> {
        if self.current != Some(session) {
            return None;
        }
        let reading = self
            .volumes
            .values()
            .filter_map(|group| {
                group
                    .active
                    .as_ref()
                    .filter(|reading| reading.session == session)
                    .or_else(|| {
                        group
                            .queued
                            .front()
                            .filter(|reading| reading.session == session)
                    })
            })
            .min_by_key(|reading| reading.accepted)?;
        Some((
            Interest::Awaited.after(now.saturating_duration_since(reading.accepted)),
            reading.folder.clone(),
        ))
    }

    pub(in crate::app) fn end(&mut self, session: Session) {
        if self.current != Some(session) {
            return;
        }
        self.current = None;
        self.retired
            .push(std::mem::take(&mut self.listed), INLINE_ITEMS + 1);
        for group in self.volumes.values_mut() {
            if group
                .active
                .as_ref()
                .is_some_and(|active| active.session == session)
            {
                group.task.cancel();
            }
            self.retired
                .push(std::mem::take(&mut group.queued), INLINE_ITEMS + 1);
            self.retired
                .push(std::mem::take(&mut group.asked), INLINE_ITEMS + 1);
        }
        self.waker.deliver(|| true);
    }

    pub(in crate::app) fn retire(&mut self, retired: Retirement) {
        if retired.is_empty() {
            return;
        }
        self.retired.append(retired);
        self.waker.deliver(|| true);
    }

    pub(super) fn tick(&mut self, now: Instant) -> bool {
        let mut active = false;
        for group in self.volumes.values_mut() {
            active |= group.task.tick(now).is_some();
        }
        active
    }

    pub(in crate::app) fn take_retired(&mut self) -> Retirement {
        for group in self.volumes.values_mut() {
            self.retired.append(group.task.take_retired());
        }
        std::mem::take(&mut self.retired)
    }

    pub(super) fn close(&mut self) -> Retirement {
        if let Some(session) = self.current {
            self.end(session);
        }
        self.closed = true;
        for group in self.volumes.values_mut() {
            group.task.cancel();
            self.retired.append(group.task.close_delivery());
        }
        self.take_retired()
    }
}

fn read(reading: Reading, _worker: &OffThread, cancel: &CancelToken) -> Listed {
    let folders = subfolders(&reading.folder, cancel);
    Listed {
        session: reading.session,
        node: reading.node,
        folder: reading.folder,
        folders,
    }
}

fn subfolders(folder: &str, cancel: &CancelToken) -> Subfolders {
    if cancel.is_cancelled() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut folders: Subfolders = entries
        .take_while(|_| !cancel.is_cancelled())
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let kind = entry.file_type().ok()?;
            // Links lead elsewhere: list them, but do not expand them.
            let link = kind.is_symlink();
            (kind.is_dir() || link && entry.path().is_dir())
                .then(|| (entry.file_name().to_string_lossy().into_owned(), !link))
        })
        .collect();
    if cancel.is_cancelled() {
        return Vec::new();
    }
    folders.sort_by_key(|(name, _)| name.to_lowercase());
    folders
}
