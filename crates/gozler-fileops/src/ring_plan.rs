//! The ring engine's decisions, which build and are tested on every OS: its
//! two profiles, how a file splits into blocks, and which parts of the
//! buffer are free. The run is `copy::ring`.
//!
//! The ring is Gozler's own copy loop, from FastCopy's and AngelCOPY's
//! designs (spec, "The ring engine"); their code is not used. AngelCOPY's
//! `RingCopyFile` copies one file at a time through 8 blocks of 8 MiB, and
//! each block is written as soon as it is read. FastCopy's hard-disk mode
//! shares one large buffer between every file, opens files ahead on 16
//! threads, and on one physical disk reads until the buffer is full and
//! then writes all of it, so the disk seeks once per buffer and not once
//! per block.

use std::collections::BTreeMap;
use std::collections::VecDeque;

use crate::settings::{RingHardDisk, RingSsd};

/// Files of this size and more use overlapped I/O; smaller ones read and
/// write synchronously.
pub(crate) const OVERLAPPED_FROM: u64 = 512 * 1024;

/// The physical disks of the two ends of a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Layout {
    OneDisk,
    TwoDisks,
}

/// How the ring runs for a pair of volumes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RingPlan {
    /// Files of this size and more go to the ring; smaller ones to the
    /// Windows engine's workers. `None` when the settings choose the Windows
    /// engine for the pair: the ring then copies only the files that
    /// Continue resumes, because only it resumes inside a file.
    pub(crate) from: Option<u64>,
    pub(crate) block: usize,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) buffer: usize,
    /// Read until the buffer is full, then write all of it.
    pub(crate) one_disk: bool,
    /// Threads that open and close files around the loop.
    pub(crate) openers: usize,
    /// The most files open at once.
    pub(crate) open_files: usize,
}

impl RingPlan {
    /// AngelCOPY's profile, for files of `from` bytes and more: its blocks in
    /// flight are its whole buffer, so each block is reading or writing.
    pub(crate) fn ssd(settings: RingSsd, from: Option<u64>) -> RingPlan {
        let block = settings.block.bytes() as usize;
        let in_flight = settings.in_flight.get();
        RingPlan {
            from,
            block,
            reads: in_flight,
            writes: in_flight,
            buffer: block * in_flight,
            one_disk: false,
            openers: 1,
            open_files: 1,
        }
    }

    /// FastCopy's hard-disk profile, for every file when `chosen`.
    pub(crate) fn hard_disk(settings: RingHardDisk, layout: Layout, chosen: bool) -> RingPlan {
        let buffer = settings.buffer.bytes() as usize;
        RingPlan {
            from: chosen.then_some(0),
            // A block never exceeds the buffer it is read into.
            block: (settings.block.bytes() as usize).min(buffer),
            reads: settings.in_flight.get(),
            writes: settings.in_flight.get(),
            buffer,
            one_disk: layout == Layout::OneDisk,
            openers: 16,
            open_files: 256,
        }
    }
}

pub(crate) fn round_up(value: u64, to: u64) -> u64 {
    value.div_ceil(to) * to
}

/// Whether a file of `size` bytes is written unbuffered: from 64 KB, or
/// FastCopy's 128 KB on FAT, because a small unbuffered write waits for
/// the disk.
pub(crate) fn destination_unbuffered(size: u64, fat: bool) -> bool {
    size >= if fat { 128 * 1024 } else { 64 * 1024 }
}

/// The blocks to read: each range of data split at `block` bytes, as
/// `(offset, length)`. Every range starts at a cluster boundary, so every
/// block starts at a sector boundary.
pub(crate) fn blocks(ranges: &[(u64, u64)], block: usize) -> VecDeque<(u64, usize)> {
    let mut blocks = VecDeque::new();
    for &(offset, length) in ranges {
        let mut at = 0u64;
        while at < length {
            let len = (length - at).min(block as u64);
            blocks.push_back((offset + at, len as usize));
            at += len;
        }
    }
    blocks
}

/// The ranges a clone covers: the whole file rounded up to a cluster, in
/// ranges under 4 GB, the largest that Windows clones at once.
pub(crate) fn clone_ranges(size: u64, cluster: u64) -> Vec<(u64, u64)> {
    let total = round_up(size, cluster);
    let most = (1u64 << 32) - cluster;
    let mut ranges = Vec::new();
    let mut at = 0;
    while at < total {
        let len = (total - at).min(most);
        ranges.push((at, len));
        at += len;
    }
    ranges
}

/// The free parts of the buffer, first fit, so blocks of one size reuse
/// each other's place as AngelCOPY's slots do, and small files pack
/// together as in FastCopy's buffer.
#[derive(Debug)]
pub(crate) struct Space {
    /// Offset to length.
    free: BTreeMap<usize, usize>,
}

impl Space {
    pub(crate) fn new(capacity: usize) -> Space {
        Space {
            free: BTreeMap::from([(0, capacity)]),
        }
    }

    /// The offset of `len` free bytes, now taken.
    pub(crate) fn take(&mut self, len: usize) -> Option<usize> {
        let (&at, &room) = self.free.iter().find(|&(_, &room)| room >= len)?;
        self.free.remove(&at);
        if room > len {
            self.free.insert(at + len, room - len);
        }
        Some(at)
    }

    pub(crate) fn give_back(&mut self, at: usize, len: usize) {
        let (mut at, mut len) = (at, len);
        if let Some((&before, &room)) = self.free.range(..at).next_back()
            && before + room == at
        {
            self.free.remove(&before);
            at = before;
            len += room;
        }
        if let Some(room) = self.free.remove(&(at + len)) {
            len += room;
        }
        self.free.insert(at, len);
    }
}
