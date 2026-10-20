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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};

use gozler_winfs::{
    AlignedBuffer, BasicInfo, Completion, Entry, Existing, IoRing, RingOpen, RingWaker, WinError,
    create_ring_destination, open_ring_destination, open_ring_source,
};

use super::streams::{finish_streams, has_other_streams};
use super::{Handed, Snapshot, Walk, Write};
use crate::listing::join;
use crate::operation::{CurrentFile, Reason};
use crate::place::{Placed, place};
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
    /// Its path at the destination, with the prefix.
    to: Vec<u16>,
    /// Set when the loop reads its first block, which is when the person
    /// would call it started: the hard-disk profile opens files ahead.
    current: Option<Arc<CurrentFile>>,
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
    Close(Box<Job>),
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
            // Opening creates a destination file and closing finishes one,
            // so neither happens while paused.
            walk.copying.pause.wait(walk.copying.cancel);
            let task = {
                let mut shared = self.shared();
                loop {
                    if let Some(job) = shared.written.pop_front() {
                        break Task::Close(Box::new(job));
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
                    self.close(walk, *job);
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
        if walk.stopped() {
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
                walk.list_entry(&file.path, Reason::Failed(error), file.size, folder);
                release(walk, handed);
                return None;
            }
            // A file the walk listed that is gone since.
            Err(error) if is_gone(error) && folder.is_some() => {
                walk.copying.progress.pass(file.size);
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
        let open = RingOpen {
            unbuffered,
            overlapped,
        };
        let code = |error: WinError| error.code;
        let mut placing = file.placing();
        // Resume and Replace write over the file there in place, keeping its
        // bytes as far as the compare below finds the source's.
        let mut resumed = None;
        if file.write == Write::Resume {
            let path = join(&file.target.verbatim, &file.final_name);
            match open_in_place(cancel, &path, open) {
                Ok(InPlace::File(destination)) => resumed = Some((destination, path)),
                // A link put at the name since the listing is not this
                // operation's file, so the choice applies to it.
                Ok(InPlace::Link(_)) => placing.replace = false,
                // The operation's own file is deleted, so whatever holds the
                // name when the copy creates it is another program's.
                Ok(InPlace::Streams(destination)) => {
                    if walk.destination.delete_entry(&destination).is_err() {
                        return self.windows(walk, handed);
                    }
                    placing.replace = false;
                }
                Err(_) => return self.windows(walk, handed),
            }
        }
        let created = match resumed {
            Some((destination, path)) => Placed::Named((destination, true, path)),
            None => {
                let create = |to: &[u16], existing| {
                    if existing == Existing::Replace {
                        match open_in_place(cancel, to, open) {
                            Ok(InPlace::File(destination)) => {
                                return Ok((destination, true, to.to_vec()));
                            }
                            // Deleted through the handle, which closes at the
                            // end of the arm, so the name is free for the
                            // create below.
                            Ok(InPlace::Link(other) | InPlace::Streams(other)) => {
                                walk.destination.delete_entry(&other)?;
                            }
                            Err(error) if is_gone(error) => {}
                            Err(error) => return Err(error),
                        }
                    }
                    retry(cancel, || create_ring_destination(to, open).map_err(code))
                        .map(|created| (created, false, to.to_vec()))
                };
                place(&file.target, placing, walk.copying.rule.choice, create)
            }
        };
        let (destination, compare, to) = match created {
            Placed::Named(named) => named,
            Placed::Skipped => {
                walk.list_entry(&file.path, Reason::Skipped, file.size, folder);
                release(walk, handed);
                return None;
            }
            Placed::Failed(ERROR_DISK_FULL | ERROR_HANDLE_DISK_FULL) => {
                walk.full.store(true, Ordering::Relaxed);
                if let Some(folder) = folder {
                    folder.keep();
                }
                release(walk, handed);
                return None;
            }
            Placed::Failed(_) => return self.windows(walk, handed),
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
            to,
            current: None,
        };
        // The bytes of a file written in place that the copy keeps.
        let mut kept = 0;
        let mut prepared = || -> Result<bool, WinError> {
            job.align = PAGE
                .max(u64::from(sector))
                .max(u64::from(job.destination.sector_size()?));
            if let Some(cluster) = target.clone_cluster {
                // A clone takes no time, so a file written in place is cloned
                // again.
                if compare {
                    job.destination.set_end_of_file(0)?;
                }
                if Ring::cloned(walk, &job, u64::from(cluster), sparse)? {
                    return Ok(true);
                }
            } else if compare {
                kept = Ring::kept(&job.source, &job.destination, size, self.plan.block)?;
                // Nothing the compare did not confirm stays, so a hole of the
                // source cannot keep the earlier file's bytes.
                job.destination.set_end_of_file(kept)?;
            }
            // A file written in place may carry a mark that the source lacks.
            if target.sparse_files && (sparse || compare) {
                job.destination.set_sparse(sparse)?;
            }
            // The reader reads only the ranges that hold data.
            let ranges = if size == 0 {
                Vec::new()
            } else if sparse {
                let ranges = job.source.allocated_ranges(size)?;
                (ranges.into_iter())
                    .filter_map(|(offset, len)| {
                        let start = offset.max(kept);
                        (offset + len > start).then(|| (start, offset + len - start))
                    })
                    .collect()
            } else {
                vec![(kept, size - kept)]
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
            Ok(false) => {
                walk.copying.progress.add_bytes(kept);
                job.written = kept;
                Some(job)
            }
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

    /// Resume and Replace: how many bytes from the start of the destination
    /// hold the source's, compared block by block. A block is kept only when
    /// it is whole and equal, and the source's last block when the
    /// destination holds all of it; the copy writes the rest. The source is
    /// open without write sharing from before this compare to the end of the
    /// copy, so every byte kept still equals the source when the copy ends.
    /// Each file is read on a thread of its own, a block ahead.
    fn kept(source: &Entry, destination: &Entry, size: u64, block: usize) -> Result<u64, WinError> {
        let (source_size, destination_size) = (size, destination.stat()?.size);
        let end = source_size.min(destination_size);
        let mut buffers = Vec::new();
        for _ in 0..2 {
            let (free, taken) = mpsc::channel();
            for _ in 0..2 {
                free.send(AlignedBuffer::new(block)?)
                    .expect("the receiver is here");
            }
            buffers.push((free, taken));
        }
        std::thread::scope(|scope| {
            let mut readers = Vec::new();
            for ((free, taken), file) in buffers.into_iter().zip([source, destination]) {
                let (sender, filled) = mpsc::channel();
                scope.spawn(move || read_blocks(file, end, block, &taken, &sender));
                readers.push((free, filled));
            }
            let [(source_free, source), (destination_free, destination)] = &readers[..] else {
                unreachable!("two readers");
            };
            let next = |filled: &mpsc::Receiver<Filled>| {
                filled
                    .recv()
                    .expect("a reader sends every block below the end")
            };
            let mut kept = 0u64;
            while kept < end {
                let want = (source_size - kept).min(block as u64);
                if kept + want > destination_size {
                    break;
                }
                let (from, from_read) = next(source)?;
                let (to, to_read) = next(destination)?;
                let len = want as usize;
                let same = from_read >= len && to_read >= len && from[..len] == to[..len];
                // A reader that ended no longer takes buffers back.
                let _ = (source_free.send(from), destination_free.send(to));
                if !same {
                    break;
                }
                kept += want;
            }
            // Dropping the channels here ends both readers before the
            // scope joins them.
            drop(readers);
            Ok(kept)
        })
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
    /// deletes the file it wrote through its own handle, then closes both
    /// handles. When the queue closes, the file stays for Continue to
    /// resume. Returns the file when the Windows engine is to copy it; after
    /// a cancel or a full disk it is done here.
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
        if !walk.copying.closing.load(Ordering::Relaxed) {
            let _ = walk.destination.delete_entry(&destination);
        }
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
    /// attributes and times, then [`Walk::finish`].
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
        walk.finish(&handed.file, handed.folder.as_ref(), before);
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

/// The entry at a destination name, opened itself, never what a reparse
/// point there points to.
enum InPlace {
    /// A file whose bytes the copy can keep.
    File(Entry),
    /// A reparse point, such as a link, which writing would write through.
    Link(Entry),
    /// A file with an alternate data stream or extended attributes, or
    /// whose streams cannot be read. Only a new file sheds a stream that
    /// the source may not have, so it is copied again from its start.
    Streams(Entry),
}

fn open_in_place(cancel: &AtomicBool, path: &[u16], open: RingOpen) -> Result<InPlace, u32> {
    let code = |error: WinError| error.code;
    let entry = retry(cancel, || open_ring_destination(path, open).map_err(code))?;
    Ok(if entry.attribute_tag().map_err(code)?.reparse_tag != 0 {
        InPlace::Link(entry)
    } else if has_other_streams(&entry) != Ok(false) {
        InPlace::Streams(entry)
    } else {
        InPlace::File(entry)
    })
}

/// One block of a compared file, with the bytes read into it.
type Filled = Result<(AlignedBuffer, usize), WinError>;

/// Reads `file` from its start in blocks below `end`, each into a buffer
/// from `free`, and sends it to `filled`, until the end, an error, or the
/// compare stops taking them.
fn read_blocks(
    file: &Entry,
    end: u64,
    block: usize,
    free: &mpsc::Receiver<AlignedBuffer>,
    filled: &mpsc::Sender<Filled>,
) {
    let mut offset = 0;
    while offset < end {
        let Ok(mut buffer) = free.recv() else {
            return;
        };
        let read = file.read_at(offset, &mut buffer);
        let failed = read.is_err();
        if filled.send(read.map(|read| (buffer, read))).is_err() || failed {
            return;
        }
        offset += block as u64;
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
            let paused = self.walk.copying.pause.is_paused();
            if !self.stopped && !paused {
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
            if paused && !self.stopped && self.io.in_flight() == 0 {
                // Between blocks, with nothing in flight.
                self.walk.copying.pause.wait(self.walk.copying.cancel);
                continue;
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
            if job.current.is_none() {
                let progress = self.walk.copying.progress;
                let file = &job.handed.file;
                let current = progress.start_file(&file.path, Some(job.to.clone()), job.size);
                current.set_done(job.written);
                job.current = Some(current);
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
            if let Some(current) = &job.current {
                current.set_done(job.written);
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A resume keeps exactly the start of the destination that equals the
    /// source, in whole blocks, and the source's last block only when the
    /// destination holds all of it, and writes the rest again. A byte kept
    /// that differs from the source would stay wrong in the copy, and a
    /// block not kept is a block written again. The files are large enough
    /// that both are read overlapped, as the ring opens them.
    #[test]
    fn a_resume_keeps_only_the_start_that_equals_the_source() {
        const BLOCK: usize = 256 * 1024;
        let folder =
            std::env::temp_dir().join(format!("gozler-fileops-{}-kept", std::process::id()));
        std::fs::create_dir_all(&folder).expect("create the test folder");
        let size = 3 * BLOCK + 100;
        let old: Vec<u8> = (0..size as u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let with = |edit: &dyn Fn(&mut Vec<u8>)| {
            let mut bytes = old.clone();
            edit(&mut bytes);
            bytes
        };
        let other_after_two = with(&|bytes| bytes[2 * BLOCK..].fill(0x5A));
        let zeros_after_two = with(&|bytes| {
            bytes[2 * BLOCK..].fill(0);
            bytes.resize(size.next_multiple_of(4096), 0);
        });
        let zero_block = with(&|bytes| bytes[BLOCK..2 * BLOCK].fill(0));
        let changed = with(&|bytes| bytes[BLOCK + 7] ^= 1);
        let grown = with(&|bytes| bytes.extend_from_slice(&[9; BLOCK]));
        let cut = with(&|bytes| bytes.truncate(2 * BLOCK + 10));
        let cases: [(&str, &[u8], &[u8], u64); 8] = [
            ("equal", &old, &old, size as u64),
            ("other bytes", &old, &other_after_two, 2 * BLOCK as u64),
            ("preset zeros", &old, &zeros_after_two, 2 * BLOCK as u64),
            ("a zero block", &old, &zero_block, BLOCK as u64),
            ("source changed", &changed, &old, BLOCK as u64),
            ("source grew", &grown, &old, 3 * BLOCK as u64),
            (
                "source shrank",
                &old[..2 * BLOCK + 100],
                &old,
                2 * BLOCK as u64 + 100,
            ),
            ("cut mid-block", &old, &cut, 2 * BLOCK as u64),
        ];
        let open = RingOpen {
            unbuffered: true,
            overlapped: true,
        };
        let units = |path: &std::path::Path| -> Vec<u16> {
            use std::os::windows::ffi::OsStrExt;
            path.as_os_str().encode_wide().collect()
        };
        for (case, source, destination, kept) in cases {
            let (from, to) = (folder.join("source"), folder.join("destination"));
            std::fs::write(&from, source).expect("write the source");
            std::fs::write(&to, destination).expect("write the destination");
            let from_entry = open_ring_source(&units(&from), open).expect("open the source");
            let to_entry = open_ring_destination(&units(&to), open).expect("open the copy");
            let found = Ring::kept(&from_entry, &to_entry, source.len() as u64, BLOCK);
            assert_eq!(found.expect("compared"), kept, "{case}");
        }
        let _ = std::fs::remove_dir_all(&folder);
    }
}
