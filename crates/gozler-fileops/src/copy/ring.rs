//! The ring's run. One loop on one completion port reads blocks into the
//! buffer and writes them out; threads around it open files ahead, and
//! close each file the loop has written: its streams, attributes and times,
//! then [`Walk::finish`]. The hard-disk profile is FastCopy's, with 16 such
//! threads and up to 256 open files; the SSD profile is AngelCOPY's, with
//! one of each, so one file moves at a time. `ring_plan` holds the
//! decisions.
//!
//! A file the ring cannot copy goes to the Windows engine and is counted,
//! as AngelCOPY's `CopyOneBig` does, except after a cancel, a full disk, or
//! a sharing violation at the open, which is how the ring refuses a file
//! that another program writes.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex};

use gozler_winfs::{
    BasicInfo, Completion, Entry, IoRing, RingOpen, RingWaker, WinError, create_ring_destination,
    open_ring_source,
};

use super::streams::finish_streams;
use super::{Handed, Snapshot, Walk};
use crate::retry::retry;
use crate::ring_plan::{
    OVERLAPPED_FROM, RingPlan, Space, blocks, clone_ranges, destination_unbuffered, round_up,
};
use crate::win32::{
    ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ERROR_HANDLE_EOF, ERROR_LOCK_VIOLATION,
    ERROR_SHARING_VIOLATION, ERROR_WRITE_FAULT, FILE_ATTRIBUTE_SPARSE_FILE, is_gone,
};

/// Every block's place in the buffer and every unbuffered read and write
/// is aligned to at least a page, and to the larger sector size of the two
/// volumes.
const PAGE: u64 = 4096;

/// How long the loop waits before it looks for a cancel again.
const WAIT_MS: u32 = 100;

pub(super) struct Ring {
    pub(super) plan: RingPlan,
    shared: Mutex<Shared>,
    /// Wakes the threads around the loop.
    changed: Condvar,
    /// Wakes the loop.
    waker: RingWaker,
}

#[derive(Default)]
struct Shared {
    /// From the walk, not yet opened.
    handed: VecDeque<Handed>,
    /// Handed files not yet in `opened` or done: queued or being opened.
    opening: usize,
    /// Opened, for the loop.
    opened: VecDeque<Job>,
    /// Read and written, for a thread to close.
    written: VecDeque<Job>,
    /// Given up by the loop, for a thread to copy with the Windows engine.
    to_windows: VecDeque<Handed>,
    /// Files held open, from their open to their close.
    open: usize,
    input_closed: bool,
    loop_ended: bool,
}

/// An open file in the ring.
struct Job {
    handed: Handed,
    source: Arc<Entry>,
    destination: Arc<Entry>,
    size: u64,
    basic: BasicInfo,
    before: Option<Snapshot>,
    /// Blocks still to read, as `(offset, length)`.
    blocks: VecDeque<(u64, usize)>,
    overlapped: bool,
    /// The destination is written unbuffered, in whole sectors.
    unbuffered: bool,
    align: u64,
    /// Blocks read or being read, and not yet written.
    chunks: usize,
    failed: Option<u32>,
    /// Bytes written, which the progress counts.
    written: u64,
}

/// One block's place in the buffer.
struct Chunk {
    job: usize,
    offset: u64,
    /// The file's bytes in it.
    len: usize,
    at: usize,
    room: usize,
    read: usize,
    /// The bytes its write asks for.
    writing: usize,
}

enum Task {
    Close(Job),
    Windows(Handed),
    Open(Handed),
    Discard(Vec<Handed>),
}

impl Ring {
    /// The ring and its buffer, or `None` when the buffer cannot be
    /// allocated.
    pub(super) fn new(plan: RingPlan) -> Option<(Ring, IoRing)> {
        let io = IoRing::new(plan.buffer, plan.reads + plan.writes).ok()?;
        let ring = Ring {
            plan,
            shared: Mutex::default(),
            changed: Condvar::new(),
            waker: io.waker(),
        };
        Some((ring, io))
    }

    fn shared(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().expect("no worker panics")
    }

    pub(super) fn hand(&self, handed: Handed) {
        let mut shared = self.shared();
        shared.handed.push_back(handed);
        shared.opening += 1;
        self.changed.notify_one();
    }

    pub(super) fn close_input(&self) {
        self.shared().input_closed = true;
        self.changed.notify_all();
        self.waker.wake();
    }

    /// One of the threads around the loop, until the loop ends.
    pub(super) fn open_and_close(&self, walk: &Walk) {
        let _entered = walk.copying.io.enter();
        loop {
            let task = {
                let mut shared = self.shared();
                loop {
                    if let Some(job) = shared.written.pop_front() {
                        break Task::Close(job);
                    }
                    if let Some(handed) = shared.to_windows.pop_front() {
                        break Task::Windows(handed);
                    }
                    if walk.stopped() && !shared.handed.is_empty() {
                        let dropped: Vec<_> = shared.handed.drain(..).collect();
                        shared.opening -= dropped.len();
                        break Task::Discard(dropped);
                    }
                    if shared.open < self.plan.open_files
                        && let Some(handed) = shared.handed.pop_front()
                    {
                        shared.open += 1;
                        break Task::Open(handed);
                    }
                    if shared.loop_ended {
                        return;
                    }
                    shared = self.changed.wait(shared).expect("no worker panics");
                }
            };
            match task {
                Task::Close(job) => {
                    self.close(walk, job);
                    self.shared().open -= 1;
                    self.changed.notify_all();
                }
                Task::Windows(handed) => {
                    self.windows(walk, handed);
                }
                Task::Discard(dropped) => {
                    for handed in dropped {
                        release(walk, handed);
                    }
                    self.waker.wake();
                }
                Task::Open(handed) => {
                    let job = self.open(walk, handed);
                    let mut shared = self.shared();
                    shared.opening -= 1;
                    match job {
                        Some(job) => shared.opened.push_back(job),
                        None => shared.open -= 1,
                    }
                    drop(shared);
                    self.changed.notify_all();
                    self.waker.wake();
                }
            }
        }
    }

    /// Opens a handed file for the loop. `None` when it is done here: listed,
    /// cloned, or copied by the Windows engine.
    fn open(&self, walk: &Walk, handed: Handed) -> Option<Job> {
        let file = &handed.file;
        let folder = handed.folder.as_deref();
        if walk.stopped() || !walk.clear_partial(file, folder) {
            release(walk, handed);
            return None;
        }
        let cancel = walk.copying.cancel;
        let overlapped = file.size >= OVERLAPPED_FROM;
        let opened = retry(cancel, || {
            let open = RingOpen {
                unbuffered: true,
                overlapped,
            };
            open_ring_source(&file.from, open).map_err(|error| error.code)
        });
        let source = match opened {
            Ok(source) => source,
            Err(error @ (ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)) => {
                walk.fail(&file.path, error, folder);
                release(walk, handed);
                return None;
            }
            // A file the walk listed that is gone since.
            Err(error) if is_gone(error) && folder.is_some() => {
                walk.copying.progress.add_items(1);
                release(walk, handed);
                return None;
            }
            Err(_) if walk.cancelled() => {
                release(walk, handed);
                return None;
            }
            // Such as an unbuffered open that a file system filter refuses.
            Err(_) => return self.windows(walk, handed),
        };
        let read = || -> Result<_, WinError> {
            let stat = source.stat()?;
            let basic = source.basic_info()?;
            let sector = source.sector_size()?;
            Ok((stat.size, basic, sector))
        };
        let Ok((size, basic, sector)) = read() else {
            return self.windows(walk, handed);
        };
        // For a move, the source as the ring reads it.
        let before = if walk.copying.moving {
            match Snapshot::of(&source) {
                Ok(before) => Some(before),
                Err(_) => return self.windows(walk, handed),
            }
        } else {
            None
        };
        let target = walk.copying.target;
        let unbuffered = destination_unbuffered(size, target.fat);
        let partial = file.partial_path();
        let created = retry(cancel, || {
            let open = RingOpen {
                unbuffered,
                overlapped,
            };
            create_ring_destination(&partial, open).map_err(|error| error.code)
        });
        let destination = match created {
            Ok(destination) => destination,
            Err(ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL) => {
                walk.full.store(true, Ordering::Relaxed);
                if let Some(folder) = folder {
                    folder.keep();
                }
                release(walk, handed);
                return None;
            }
            Err(_) => return self.windows(walk, handed),
        };
        let sparse = basic.attributes & FILE_ATTRIBUTE_SPARSE_FILE != 0;
        let mut job = Job {
            handed,
            source: Arc::new(source),
            destination: Arc::new(destination),
            size,
            basic,
            before,
            blocks: VecDeque::new(),
            overlapped,
            unbuffered,
            align: PAGE,
            chunks: 0,
            failed: None,
            written: 0,
        };
        let mut prepared = || -> Result<bool, WinError> {
            job.align = PAGE
                .max(u64::from(sector))
                .max(u64::from(job.destination.sector_size()?));
            if let Some(cluster) = target.clone_cluster
                && Ring::cloned(walk, &job, u64::from(cluster), sparse)?
            {
                return Ok(true);
            }
            if sparse && target.sparse_files {
                job.destination.set_sparse(true)?;
            }
            // The reader reads only the ranges that hold data.
            let ranges = if size == 0 {
                Vec::new()
            } else if sparse {
                job.source.allocated_ranges(size)?
            } else {
                vec![(0, size)]
            };
            job.blocks = blocks(&ranges, self.plan.block);
            // A write that extends a file runs synchronously, so the whole
            // file is there before the first write (Microsoft KB 156932).
            let end = if unbuffered {
                round_up(size, job.align)
            } else {
                size
            };
            job.destination.set_end_of_file(end)?;
            Ok(false)
        };
        match prepared() {
            Ok(false) => Some(job),
            Ok(true) => {
                walk.copying.progress.add_bytes(size);
                job.written = size;
                self.close(walk, job);
                None
            }
            Err(error) => {
                let Job {
                    handed,
                    source,
                    destination,
                    ..
                } = job;
                let given_up = self.give_up(walk, handed, source, destination, 0, error.code);
                given_up.and_then(|handed| self.windows(walk, handed))
            }
        }
    }

    /// Clones the whole file within its volume (D20); `false` when the
    /// volume refused, which is counted, and the destination is then empty
    /// and not sparse for the ring to copy into.
    ///
    /// ReFS reserves the whole size of a file that is not sparse, so the
    /// clone goes into a destination marked sparse, as `CopyFileExW` does.
    /// The mark is cleared afterwards for a source that is not sparse. ReFS
    /// refuses to clear it when the volume cannot hold the whole size; the
    /// copy then stays sparse, as `CopyFileExW` leaves it.
    fn cloned(walk: &Walk, job: &Job, cluster: u64, sparse: bool) -> Result<bool, WinError> {
        let destination = &job.destination;
        let cloned = destination
            .set_sparse(true)
            .and_then(|()| destination.set_end_of_file(job.size))
            .and_then(|()| {
                for (offset, len) in clone_ranges(job.size, cluster) {
                    destination.clone_range(&job.source, offset, len)?;
                }
                Ok(())
            });
        if cloned.is_err() {
            walk.clones_refused.fetch_add(1, Ordering::Relaxed);
            destination.set_end_of_file(0)?;
            destination.set_sparse(false)?;
            return Ok(false);
        }
        if !sparse
            && let Err(error) = destination.set_sparse(false)
            && error.code != ERROR_DISK_FULL
        {
            return Err(error);
        }
        Ok(true)
    }

    /// Copies a handed file with the Windows engine now, and counts it.
    fn windows(&self, walk: &Walk, handed: Handed) -> Option<Job> {
        walk.to_windows.fetch_add(1, Ordering::Relaxed);
        walk.copy_file(&handed.file, handed.folder.as_ref());
        release(walk, handed);
        None
    }

    /// Ends a file the ring could not copy: takes back its progress and
    /// deletes its temporary file through its own handle, then closes both
    /// handles. Returns the file when the Windows engine is to copy it;
    /// after a cancel or a full disk it is done here.
    fn give_up(
        &self,
        walk: &Walk,
        handed: Handed,
        source: Arc<Entry>,
        destination: Arc<Entry>,
        written: u64,
        error: u32,
    ) -> Option<Handed> {
        walk.copying.progress.remove_bytes(written);
        let _ = walk.destination.delete_entry(&destination);
        // The name leaves when the handles close, before the Windows engine
        // writes it again.
        drop((source, destination));
        if matches!(error, ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL) {
            walk.full.store(true, Ordering::Relaxed);
        }
        if walk.stopped() {
            if let Some(folder) = &handed.folder {
                folder.keep();
            }
            release(walk, handed);
            return None;
        }
        Some(handed)
    }

    /// Closes a file the loop wrote whole: its true end of file, streams,
    /// attributes and times, then its final name.
    fn close(&self, walk: &Walk, job: Job) {
        let Job {
            handed,
            source,
            destination,
            size,
            basic,
            before,
            written,
            ..
        } = job;
        if let Err(error) = finish_streams(&source, &destination, size, basic) {
            let given_up = self.give_up(walk, handed, source, destination, written, error);
            if let Some(handed) = given_up {
                self.windows(walk, handed);
            }
            return;
        }
        drop((source, destination));
        walk.finish(&handed.file, handed.folder.as_ref(), written, before);
        release(walk, handed);
    }

    /// The loop, until the walk hands nothing more and every file is out.
    pub(super) fn run(&self, walk: &Walk, io: IoRing) {
        let _entered = walk.copying.io.enter();
        let slots = self.plan.reads + self.plan.writes;
        let mut run = Loop {
            ring: self,
            walk,
            io,
            jobs: Slab::default(),
            chunks: Slab::default(),
            slots: (0..slots).map(|_| None).collect(),
            reading: VecDeque::new(),
            filled: VecDeque::new(),
            space: Space::new(self.plan.buffer),
            reads: 0,
            writes: 0,
            writing: false,
            stopped: false,
        };
        run.run();
    }
}

fn release(walk: &Walk, handed: Handed) {
    if let Some(folder) = handed.folder {
        walk.release(folder);
    }
}

/// Items named by their index, which is reused once they are removed.
struct Slab<T> {
    items: Vec<Option<T>>,
    free: Vec<usize>,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Slab {
            items: Vec::new(),
            free: Vec::new(),
        }
    }
}

impl<T> Slab<T> {
    fn insert(&mut self, item: T) -> usize {
        match self.free.pop() {
            Some(id) => {
                self.items[id] = Some(item);
                id
            }
            None => {
                self.items.push(Some(item));
                self.items.len() - 1
            }
        }
    }

    fn get(&mut self, id: usize) -> &mut T {
        self.items[id].as_mut().expect("an id names a live item")
    }

    fn remove(&mut self, id: usize) -> T {
        self.free.push(id);
        self.items[id].take().expect("an id names a live item")
    }

    fn is_empty(&self) -> bool {
        self.free.len() == self.items.len()
    }

    fn ids(&self) -> Vec<usize> {
        (self.items.iter().enumerate())
            .filter_map(|(id, item)| item.as_ref().map(|_| id))
            .collect()
    }
}

struct Loop<'r, 'w> {
    ring: &'r Ring,
    walk: &'r Walk<'w>,
    io: IoRing,
    jobs: Slab<Job>,
    chunks: Slab<Chunk>,
    /// The chunk of each slot in flight, and whether it is a write.
    slots: Vec<Option<(usize, bool)>>,
    /// Jobs with blocks still to read, in the order they came.
    reading: VecDeque<usize>,
    /// Chunks read, in the order they were read, waiting to be written.
    filled: VecDeque<usize>,
    space: Space,
    reads: usize,
    writes: usize,
    /// On one disk: the buffer is being written out, and nothing is read.
    writing: bool,
    /// Cancelled, a full disk, or a failed completion port.
    stopped: bool,
}

impl Loop<'_, '_> {
    fn run(&mut self) {
        let one_disk = self.ring.plan.one_disk;
        loop {
            self.take_opened();
            if !self.stopped && self.walk.stopped() {
                // The walk's stop gives no file to the Windows engine, so
                // the error is never read.
                self.stop(ERROR_WRITE_FAULT);
            }
            if !self.stopped {
                self.read();
                if one_disk && !self.writing && self.reads == 0 && self.reading.is_empty() {
                    // Nothing more to read now: write what the buffer holds.
                    self.writing = !self.filled.is_empty();
                }
                if !one_disk || (self.writing && self.reads == 0) {
                    self.write();
                }
                if one_disk && self.writing && self.filled.is_empty() && self.writes == 0 {
                    self.writing = false;
                    continue;
                }
            }
            if self.jobs.is_empty() && self.io.in_flight() == 0 {
                let mut shared = self.ring.shared();
                if shared.input_closed && shared.opening == 0 && shared.opened.is_empty() {
                    shared.loop_ended = true;
                    drop(shared);
                    self.ring.changed.notify_all();
                    return;
                }
            }
            match self.io.wait(WAIT_MS) {
                Ok(Some(Completion::Done { slot, result })) => self.completed(slot, result),
                Ok(Some(Completion::Woken) | None) => {}
                // Every file goes to the Windows engine instead.
                Err(error) => self.stop(error.code),
            }
        }
    }

    fn take_opened(&mut self) {
        let opened: Vec<Job> = self.ring.shared().opened.drain(..).collect();
        for job in opened {
            let associated = !job.overlapped
                || (self.io.associate(&job.source))
                    .and_then(|()| self.io.associate(&job.destination))
                    .is_ok();
            let has_blocks = !job.blocks.is_empty();
            let id = self.jobs.insert(job);
            if !associated || self.stopped {
                self.fail(id, ERROR_WRITE_FAULT);
            } else if has_blocks {
                self.reading.push_back(id);
            }
            self.settle(id);
        }
    }

    /// Cancels every operation in flight and gives up every file with
    /// `error`; each settles when its last operation ends.
    fn stop(&mut self, error: u32) {
        self.stopped = true;
        self.io.cancel();
        for chunk in std::mem::take(&mut self.filled) {
            self.free(chunk);
        }
        for id in self.jobs.ids() {
            self.fail(id, error);
            self.settle(id);
        }
        self.ring.changed.notify_all();
    }

    fn read(&mut self) {
        let plan = self.ring.plan;
        while !(plan.one_disk && self.writing) && self.reads < plan.reads {
            let Some(&id) = self.reading.front() else {
                return;
            };
            let job = self.jobs.get(id);
            let (offset, len) = *job.blocks.front().expect("a job in `reading` has blocks");
            let room = round_up(len as u64, job.align) as usize;
            let Some(at) = self.space.take(room) else {
                // The buffer is full.
                self.writing = plan.one_disk;
                return;
            };
            job.blocks.pop_front();
            if job.blocks.is_empty() {
                self.reading.pop_front();
            }
            job.chunks += 1;
            let (overlapped, source) = (job.overlapped, Arc::clone(&job.source));
            let chunk = self.chunks.insert(Chunk {
                job: id,
                offset,
                len,
                at,
                room,
                read: 0,
                writing: 0,
            });
            if overlapped {
                match self.io.start_read(&source, offset, at, room) {
                    Ok(slot) => {
                        self.slots[slot] = Some((chunk, false));
                        self.reads += 1;
                    }
                    Err(error) => self.chunk_failed(chunk, error.code),
                }
            } else {
                match self.io.read_sync(&source, offset, at, room) {
                    Ok(read) => {
                        self.chunks.get(chunk).read = read;
                        self.filled.push_back(chunk);
                    }
                    Err(error) => self.chunk_failed(chunk, error.code),
                }
            }
        }
    }

    fn write(&mut self) {
        while self.writes < self.ring.plan.writes {
            let Some(chunk) = self.filled.pop_front() else {
                return;
            };
            let id = self.chunks.get(chunk).job;
            let job = self.jobs.get(id);
            let (overlapped, unbuffered, align) = (job.overlapped, job.unbuffered, job.align);
            let failed = job.failed.is_some();
            let destination = Arc::clone(&job.destination);
            if failed {
                let job = self.free(chunk);
                self.settle(job);
                continue;
            }
            let entry = self.chunks.get(chunk);
            if entry.read < entry.len {
                // The file is shorter than its size said.
                self.chunk_failed(chunk, ERROR_HANDLE_EOF);
                continue;
            }
            // An unbuffered write is whole sectors; the bytes past the end
            // go when the true end of file is set.
            entry.writing = if unbuffered {
                round_up(entry.len as u64, align) as usize
            } else {
                entry.len
            };
            let (offset, at, len) = (entry.offset, entry.at, entry.writing);
            if overlapped {
                match self.io.start_write(&destination, offset, at, len) {
                    Ok(slot) => {
                        self.slots[slot] = Some((chunk, true));
                        self.writes += 1;
                    }
                    Err(error) => self.chunk_failed(chunk, error.code),
                }
            } else {
                match self.io.write_sync(&destination, offset, at, len) {
                    Ok(()) => self.written(chunk, len),
                    Err(error) => self.chunk_failed(chunk, error.code),
                }
            }
        }
    }

    fn completed(&mut self, slot: usize, result: Result<usize, u32>) {
        let (chunk, write) = self.slots[slot]
            .take()
            .expect("a completion has a started slot");
        if write {
            self.writes -= 1;
        } else {
            self.reads -= 1;
        }
        match (write, result) {
            (true, Ok(bytes)) => self.written(chunk, bytes),
            (false, Ok(bytes)) => {
                let id = self.chunks.get(chunk).job;
                if self.stopped || self.jobs.get(id).failed.is_some() {
                    let job = self.free(chunk);
                    self.settle(job);
                } else {
                    self.chunks.get(chunk).read = bytes;
                    self.filled.push_back(chunk);
                }
            }
            (_, Err(error)) => self.chunk_failed(chunk, error),
        }
    }

    /// A chunk's write ended with `bytes` written.
    fn written(&mut self, chunk: usize, bytes: usize) {
        let entry = self.chunks.get(chunk);
        let (asked, len) = (entry.writing, entry.len as u64);
        if bytes < asked {
            return self.chunk_failed(chunk, ERROR_WRITE_FAULT);
        }
        let id = self.free(chunk);
        let job = self.jobs.get(id);
        if job.failed.is_none() {
            job.written += len;
            self.walk.copying.progress.add_bytes(len);
        }
        self.settle(id);
    }

    fn chunk_failed(&mut self, chunk: usize, error: u32) {
        let job = self.free(chunk);
        self.fail(job, error);
        self.settle(job);
    }

    /// Gives a chunk's place back; returns its job.
    fn free(&mut self, chunk: usize) -> usize {
        let chunk = self.chunks.remove(chunk);
        self.space.give_back(chunk.at, chunk.room);
        self.jobs.get(chunk.job).chunks -= 1;
        chunk.job
    }

    /// Stops reading a job; its chunks in flight still end.
    fn fail(&mut self, id: usize, error: u32) {
        let job = self.jobs.get(id);
        job.failed.get_or_insert(error);
        job.blocks.clear();
        self.reading.retain(|&reading| reading != id);
    }

    /// Hands a job on once nothing of it is in the buffer or in flight:
    /// written, to be closed, or failed, to be given up.
    fn settle(&mut self, id: usize) {
        let job = self.jobs.get(id);
        if job.chunks > 0 || !job.blocks.is_empty() {
            return;
        }
        let job = self.jobs.remove(id);
        let Some(error) = job.failed else {
            self.ring.shared().written.push_back(job);
            self.ring.changed.notify_one();
            return;
        };
        let Job {
            handed,
            source,
            destination,
            written,
            ..
        } = job;
        let given_up = (self.ring).give_up(self.walk, handed, source, destination, written, error);
        let mut shared = self.ring.shared();
        shared.open -= 1;
        if let Some(handed) = given_up {
            shared.to_windows.push_back(handed);
        }
        drop(shared);
        self.ring.changed.notify_all();
    }
}
