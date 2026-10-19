//! Per-volume USN journal tail: one dedicated OS thread that builds its
//! volume's index, then reads the change journal from the build's cursor
//! forward and applies each batch to the shared index, keeping it live.
//!
//! v1 polls with a non-blocking drain and a short sleep when caught up, which
//! gives sub-100 ms freshness and a clean, `CancelSynchronousIo`-free shutdown
//! (the blocking `BytesToWaitFor=1` push variant is a post-v1 refinement). It
//! applies structural changes (create / rename / delete) immediately and
//! collects the `ApplyOutcome::needs_stat` side effects for the enricher,
//! which refreshes the size/date a USN record does not carry.
//!
//! Hard links are reconciled out of band: a `HARD_LINK_CHANGE` record names one
//! link and may name a dead one, so the real set comes from a Win32 link walk
//! (`gozler_winfs::file_links`) fed to `VolumeIndex::reconcile_links`. A walk that
//! cannot be resolved is skipped, never applied as an empty set (which would
//! delete a live file), and counted as `IndexStats::link_reconciles_dropped` so
//! `--status` shows it rather than hiding it.
//!
//! A rename can also bring in a folder the index lacks, because a folder walk
//! is not a snapshot (see `ApplyOutcome::needs_walk`). The tail lists such a
//! folder and everything under it; a listing that fails starts a rescan.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gozler_core::index::LinkTarget;
use gozler_core::types::{Frn, VolumePhase};
use gozler_core::usn::record::{
    USN_REASON_BASIC_INFO_CHANGE, USN_REASON_CLOSE, USN_REASON_DATA_EXTEND,
    USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
    USN_REASON_RENAME_OLD_NAME,
};
use gozler_core::usn::walk_journal_buffer;
use gozler_winfs::{
    ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED, ERROR_JOURNAL_NOT_ACTIVE,
    JournalInfo, VolumeHandle, read_usn_journal, stat_file,
};

use crate::bootstrap::{self, Filesystem, JournalSetup};
use crate::pulse::TailPulse;
use crate::volume_state::VolumeState;

/// Reason bits we ask the kernel to report: everything that changes a name,
/// a hard link, or a file's size/timestamps.
const REASON_MASK: u32 = USN_REASON_FILE_CREATE
    | USN_REASON_FILE_DELETE
    | USN_REASON_RENAME_OLD_NAME
    | USN_REASON_RENAME_NEW_NAME
    | USN_REASON_HARD_LINK_CHANGE
    | USN_REASON_BASIC_INFO_CHANGE
    | USN_REASON_DATA_OVERWRITE
    | USN_REASON_DATA_EXTEND
    | USN_REASON_DATA_TRUNCATION
    | USN_REASON_CLOSE;

/// Why the tail rebuilds its volume's index. The log names it, and a volume
/// whose rebuild fails shows it in its `Failed` reason.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RescanReason {
    /// `ERROR_JOURNAL_NOT_ACTIVE` (1179): no journal on the volume.
    JournalNotActive,
    /// `ERROR_JOURNAL_DELETE_IN_PROGRESS` (1178): a deletion is running;
    /// wait for it, then rescan.
    JournalDeleteInProgress,
    /// `ERROR_JOURNAL_ENTRY_DELETED` (1181) from a read: our `StartUsn` was
    /// purged before the read.
    EntryDeletedError,
    /// A corrupt buffer or cycle was detected while parsing/applying; we
    /// cannot prove what was missed.
    ParserAnomaly,
    /// The tail started, so it has no journal cursor and nothing proves the
    /// index is in sync with the journal: the volume was never indexed, or its
    /// supervisor revived the tail after the previous one exited.
    NoCursor,
    /// Journal reads kept failing past the escalation window with codes no
    /// arm maps (observed live: `ERROR_INVALID_PARAMETER` against a cursor
    /// whose journal was deleted mid-read); whatever the cause, a journal that
    /// will not stop failing cannot be trusted.
    ReadsKeptFailing,
    /// A folder the index missed could not be listed, so its contents stay
    /// missing.
    WalkFailed,
}

/// 512 KiB journal read buffer (~5k records per fill).
const READ_BUFFER_BYTES: usize = 512 * 1024;
/// Poll interval when the journal is caught up.
const IDLE_POLL: Duration = Duration::from_millis(50);

/// How long journal reads may keep failing before the volume stops claiming to
/// be `Live`. One failure is not news (a drive spinning up answers
/// `ERROR_NOT_READY` for a moment); several seconds of them means the index has
/// stopped tracking the volume, which every consumer must be told.
const DEGRADE_AFTER: Duration = Duration::from_secs(5);
/// How long journal reads may keep failing before retrying stops and a
/// bounded rescan takes over. Long enough that a spin-up's `ERROR_NOT_READY`
/// window (~15 s worst observed) never triggers a needless rebuild, short
/// enough that an unmappable error (see the escalation comment in the read
/// loop) costs seconds of zombie time, not forever.
const ESCALATE_AFTER: Duration = Duration::from_secs(30);
/// Read-retry backoff bounds. The ceiling is what keeps a permanently sick
/// volume from writing a log line every 500 ms for the life of the service.
/// The ceiling is `pub(crate)` because the supervisor's stall threshold must
/// comfortably exceed it, and a test over there pins that coupling.
const RETRY_BACKOFF_MIN: Duration = Duration::from_millis(500);
pub(crate) const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// Consecutive rescans before giving up and marking the volume Failed: a storm
/// guard so a persistently broken journal cannot spin on rebuilds forever. Reset
/// after any healthy batch.
const MAX_RESCANS: u32 = 3;

/// A running tail thread and the flag that stops it.
pub(crate) struct TailHandle {
    stop: Arc<AtomicBool>,
    join: JoinHandle<()>,
    /// Volume this tail serves, for supervisor log lines.
    pub(crate) mount: String,
    /// The tail's self-reported liveness: beating, busy in a rebuild, or
    /// retired. The supervisor's stall decisions read only this, never
    /// `state.phase`, which is user-facing honesty state; the phase is
    /// consulted only for what it says about the volume: whether it is served
    /// (the supervisor's `accrue_health`) and whether a retired tail left a
    /// snapshot that needs no revival.
    pub(crate) pulse: Arc<TailPulse>,
    /// Real handle to the tail thread, target of `cancel_synchronous_io`.
    pub(crate) io_handle: gozler_winfs::ThreadIoHandle,
}

impl TailHandle {
    /// Signals the tail to stop and joins it. Only safe against a thread
    /// that will actually observe the flag; teardown of a possibly-wedged
    /// tail goes through the supervisor's cancel-and-poll loop instead.
    pub(crate) fn shutdown(self) {
        self.signal_stop();
        let _ = self.join.join();
    }

    /// Signals the tail to stop without joining, so teardown can flag every
    /// tail before waiting on any single one of them.
    pub(crate) fn signal_stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Whether the tail thread has exited, i.e. a join would not block.
    pub(crate) fn is_finished(&self) -> bool {
        self.join.is_finished()
    }
}

/// Spawns the tail thread for one volume. `handle` is moved into the thread
/// (it is not shared). The thread's first act is a full build, which builds
/// the index and acquires a fresh cursor (see [`RescanReason::NoCursor`]).
pub(crate) fn spawn(state: Arc<VolumeState>, handle: VolumeHandle) -> std::io::Result<TailHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let pulse = Arc::new(TailPulse::new());
    let pulse_thread = pulse.clone();
    let mount = state.mount_prefix().to_string();
    // The supervisor needs a REAL handle to this thread, and duplicating one
    // must happen ON the thread (the pseudo-handle is per-thread), so the
    // thread sends it back before entering the loop.
    let (io_tx, io_rx) = std::sync::mpsc::sync_channel(1);
    // OS thread creation can fail on a machine near its thread/handle limit.
    // Propagate it so the supervisor retries this one volume rather than
    // panic the whole daemon.
    let join = std::thread::Builder::new()
        .name(format!("gozler-tail-{mount}"))
        .spawn(move || {
            let io = gozler_winfs::current_thread_io_handle();
            let usable = io.is_ok();
            let _ = io_tx.send(io);
            if !usable {
                // `spawn` is about to report failure and drop this thread's
                // stop flag and join handle. Running on anyway would leak a
                // live index writer that nobody can stop, join, supervise,
                // or ever rescue from a wedge; and a caller that retries the
                // spawn (the supervisor's revive) would then stack a second
                // writer on the same volume. No tail beats a ghost tail.
                return;
            }
            // Publishes `Retired` on ANY exit: the stop flag, a give-up
            // return, or a panic unwind. This is what lets the supervisor
            // trust silence: the only way to stop stamping without retiring
            // is to be genuinely wedged in a syscall. The handle moves INTO
            // tail_loop so its CloseHandle runs before this guard fires;
            // `Retired` is a promise that joining is instant, and a close is
            // itself an IRP a sick device can pend.
            let _retired = pulse_thread.retire_on_drop();
            tail_loop(&state, handle, &stop_thread, &pulse_thread)
        })?;
    let io_handle = io_rx
        .recv()
        .map_err(|_| std::io::Error::other("tail thread died before sending its handle"))?
        .map_err(|e| std::io::Error::other(format!("duplicating tail thread handle: {e}")))?;
    Ok(TailHandle {
        stop,
        join,
        mount,
        pulse,
        io_handle,
    })
}

/// Where the tail reads the journal next, and the run of failed reads there.
/// One value, so a rescan's fresh cursor always comes with a fresh failure
/// run: a run carried across a rescan would leave a Live volume unmarked while
/// its reads fail, or escalate the next failure straight into another rescan.
struct ReadPosition {
    start_usn: i64,
    journal_id: u64,
    /// `None` while reads succeed.
    failures: Option<ReadFailures>,
    /// The reads are still replaying the changes made while a folder walk
    /// built the index: from the build's cursor until the first read that
    /// finds nothing new.
    replaying_walk: bool,
}

impl ReadPosition {
    /// Where the tail reads after a build of `filesystem` that took `cursor`.
    fn at(cursor: JournalInfo, filesystem: Filesystem) -> Self {
        Self {
            start_usn: cursor.next_usn,
            journal_id: cursor.journal_id,
            failures: None,
            replaying_walk: filesystem == Filesystem::Refs,
        }
    }
}

/// A run of consecutive failed journal reads.
struct ReadFailures {
    since: Instant,
    backoff: Duration,
    /// This run moved the volume to Offline, so only its end moves it back and
    /// the tail never stomps a phase someone else owns (Rescanning, Failed).
    degraded: bool,
}

fn tail_loop(state: &VolumeState, handle: VolumeHandle, stop: &AtomicBool, pulse: &TailPulse) {
    // Owned so it drops in THIS frame, on return and on unwind alike, before
    // the spawning closure's retire guard publishes `Retired`. A thread that
    // claimed Retired while still wedged in its handle's CloseHandle would
    // draw an unbounded join from the supervisor's give-up or revive path.
    let handle = &handle;
    let mut rescans: u32 = 0;
    // A starting tail cannot prove the index is in sync: a new volume has no
    // index yet, and a revived tail's volume kept changing while it had no
    // tail. Build it and pick up a fresh cursor before tailing a single
    // record. Deliberately NOT routed through `bounded_rescan`: this build is
    // the price of a start, not a journal fault, and charging it would leave
    // every incarnation one incident short of the documented budget (the
    // counter only resets on a non-empty batch, so on a quiet volume the
    // charge would never clear). Revival storms are bounded by the
    // supervisor's own budget instead.
    let Some(mut pos) = rescan(
        state,
        handle,
        pulse,
        RescanReason::NoCursor,
        "the tail started without a journal cursor",
        stop,
    ) else {
        return;
    };
    let mut buf = vec![0u8; READ_BUFFER_BYTES];

    'tail: while !stop.load(Ordering::Relaxed) {
        // Liveness stamp for the supervisor: as long as this loop is turning,
        // however unhealthily, no one cancels our I/O.
        pulse.beat();
        // A path that can no longer prove the index is in sync breaks out
        // with the reason to rebuild it; every other path continues.
        let (reason, detail) = 'rescan: {
            match read_usn_journal(
                handle,
                state.filesystem.journal_records(),
                pos.start_usn,
                pos.journal_id,
                REASON_MASK,
                0,
                &mut buf,
            ) {
                Ok(bytes) => {
                    if pos.failures.take().is_some_and(|f| f.degraded) {
                        tracing::info!(
                            volume = %state.mount_prefix(),
                            "journal reads recovered; volume is tracking again"
                        );
                        *state.phase.write() = VolumePhase::Live;
                    }
                    let walk = match walk_journal_buffer(&buf[..bytes]) {
                        Ok(w) => w,
                        // A corrupt buffer means we cannot prove what we
                        // missed, so rebuild the index from the MFT rather
                        // than drift.
                        Err(e) => break 'rescan (RescanReason::ParserAnomaly, format!("{e:?}")),
                    };
                    if walk.records.is_empty() {
                        // Caught up (or only filtered-out records): advance and idle.
                        if bytes <= size_of::<i64>() {
                            // Nothing past the next USN: the reads have passed
                            // every change made during the build.
                            pos.replaying_walk = false;
                        }
                        pos.start_usn = walk.next_usn;
                        std::thread::sleep(IDLE_POLL);
                        continue 'tail;
                    }
                    // Apply structural changes under the write lock, then collect
                    // the FRNs whose size/date must be refreshed (USN records carry
                    // neither). Hard-link-dirty FRNs are stat'd too, which refreshes
                    // their size/date but does NOT reconcile their names.
                    //
                    // Hold the write lock only for the mutation; the owned
                    // `ApplyOutcome` needs no lock, so build the dirty list after
                    // releasing it (keeps the query-blocking window minimal).
                    //
                    // The lock covers only the mutation. Arena compaction, which is
                    // the one thing here that used to run long (~184 ms on a large
                    // index), is no longer inline: `apply_batch` reports it and
                    // `compact_index` does the copying off the exclusive lock.
                    let mut outcome = {
                        let mut index = state.index.write();
                        index.apply_batch(&walk.records)
                    };
                    for anomaly in &outcome.anomalies {
                        tracing::debug!(volume = %state.mount_prefix(), ?anomaly, "index anomaly");
                    }
                    if pos.replaying_walk {
                        // A walk lists one folder at a time, so a hard link
                        // renamed during it from a folder not yet listed into one
                        // already listed was never listed, and its rename record
                        // replaced the file's other name. The link reconcile
                        // restores the file's real names.
                        outcome.needs_link_reconcile.extend(
                            walk.records
                                .iter()
                                .filter(|r| {
                                    r.reason & USN_REASON_RENAME_NEW_NAME != 0 && !r.is_dir()
                                })
                                .map(|r| r.frn),
                        );
                        outcome.needs_link_reconcile.sort_unstable();
                        outcome.needs_link_reconcile.dedup();
                    }

                    // Before the link reconcile, which finds each link's folder by
                    // its path: that folder may be one this walk adds.
                    if let Err(e) = walk_missed(state, handle, &outcome.needs_walk, stop, pulse) {
                        break 'rescan (RescanReason::WalkFailed, format!("{e:#}"));
                    }
                    let mut dirty: Vec<Frn> = outcome
                        .needs_stat
                        .iter()
                        .chain(&outcome.needs_link_reconcile)
                        .copied()
                        .collect();
                    dirty.sort_unstable();
                    dirty.dedup();
                    enrich_stats(state, handle, &dirty, stop, pulse);
                    // A hard-link change also alters the file's NAMES, which no stat
                    // can refresh and which the record itself cannot be trusted for
                    // (it may name a link that is already dead).
                    reconcile_links(state, handle, &outcome.needs_link_reconcile, stop, pulse);
                    if outcome.wants_compact {
                        compact_index(state);
                    }
                    pos.start_usn = walk.next_usn;
                    rescans = 0; // a healthy batch resets the rescan storm guard
                    continue 'tail;
                }
                Err(e) if e.code == ERROR_JOURNAL_ENTRY_DELETED => {
                    // Our cursor fell off the end of the journal (wrap / deletion):
                    // the index can no longer prove it is in sync, so rebuild it in
                    // place and resume tailing from a fresh cursor.
                    (RescanReason::EntryDeletedError, e.to_string())
                }
                Err(e)
                    if e.code == ERROR_JOURNAL_NOT_ACTIVE
                        || e.code == ERROR_JOURNAL_DELETE_IN_PROGRESS =>
                {
                    // The journal we were tailing is gone or being deleted. Retrying
                    // the same cursor against it can never succeed, so rebuild from
                    // the MFT and pick up a fresh one.
                    let reason = if e.code == ERROR_JOURNAL_NOT_ACTIVE {
                        RescanReason::JournalNotActive
                    } else {
                        RescanReason::JournalDeleteInProgress
                    };
                    (reason, e.to_string())
                }
                Err(e) => {
                    // Some other read failure. It may be transient (a drive spinning
                    // up answers ERROR_NOT_READY for a moment), so keep retrying
                    // rather than abandoning the volume.
                    //
                    // What we must never do is retry silently while still claiming
                    // `Live`: that is the third state the design forbids, where the
                    // index has stopped tracking the volume and every query still
                    // reports it complete. Past `DEGRADE_AFTER` the drift is real, so
                    // say so and let `--status`, `Hello.ready`, and every result page
                    // carry it.
                    let failures = pos.failures.get_or_insert_with(|| ReadFailures {
                        since: Instant::now(),
                        backoff: RETRY_BACKOFF_MIN,
                        degraded: false,
                    });
                    let since = failures.since;
                    // Nor may we retry FOREVER: deleting the journal mid-read was
                    // observed to answer the stale cursor with
                    // ERROR_INVALID_PARAMETER (87), a code no arm maps, and no
                    // retry of that can ever succeed, so the volume sat Offline
                    // permanently while its tail beat away and the supervisor
                    // (correctly) saw nothing to revive. Past `ESCALATE_AFTER`
                    // the code no longer matters: reads that will not stop
                    // failing mean the journal cannot be trusted, so rebuild.
                    // Bounded: a rescan that fails retires the tail into the
                    // supervisor's revival budget instead of looping here.
                    if since.elapsed() >= ESCALATE_AFTER {
                        break 'rescan (RescanReason::ReadsKeptFailing, e.to_string());
                    }
                    if !failures.degraded && since.elapsed() >= DEGRADE_AFTER {
                        tracing::error!(
                            volume = %state.mount_prefix(),
                            error = %e,
                            failing_for_secs = since.elapsed().as_secs(),
                            "journal reads keep failing; marking volume Offline (its index is no longer current)"
                        );
                        *state.phase.write() = VolumePhase::Offline;
                        failures.degraded = true;
                    } else {
                        tracing::warn!(volume = %state.mount_prefix(), error = %e, "journal read failed; retrying");
                    }
                    // Growing backoff, so a permanently sick volume costs one line
                    // per RETRY_BACKOFF_MAX instead of one every 500 ms forever.
                    // Interruptible: a stop arriving mid-backoff must not wait out
                    // up to 30 s, or teardown would mistake this tail for wedged.
                    sleep_unless_stopped(stop, failures.backoff);
                    failures.backoff = (failures.backoff * 2).min(RETRY_BACKOFF_MAX);
                    continue 'tail;
                }
            }
        };
        let Some(fresh) = bounded_rescan(state, handle, pulse, reason, &detail, &mut rescans, stop)
        else {
            return;
        };
        pos = fresh;
    }
}

/// Compacts the name arenas, blocking queries only for the install.
///
/// The expensive half copies every live name into fresh arenas (hundreds of MB
/// on a large volume, measured at ~184 ms), and it used to run inline in
/// `apply_batch` under the write lock, so a single journal batch froze every
/// query for that long.
///
/// The plan is only valid while the index does not change, which is safe here
/// for a specific reason: a volume has exactly one writer, this tail thread. An
/// upgradable read lets queries keep reading throughout while excluding any
/// other writer, and the upgrade to exclusive covers only the column writes.
fn compact_index(state: &VolumeState) {
    let started = Instant::now();
    let guard = state.index.upgradable_read();
    let Some(plan) = guard.plan_compaction() else {
        return; // raced with another compaction, or no longer worth it
    };
    let planned = started.elapsed();

    let install_started = Instant::now();
    let mut index = parking_lot::RwLockUpgradableReadGuard::upgrade(guard);
    index.apply_compaction(plan);
    drop(index);

    // A ~184 ms query stall used to be completely invisible. `blocked_ms` is the
    // only part that stops queries; `planned_ms` runs alongside them.
    tracing::debug!(
        volume = %state.mount_prefix(),
        planned_ms = planned.as_millis() as u64,
        blocked_ms = install_started.elapsed().as_millis() as u64,
        "compacted name arenas"
    );
}

/// Reconciles the hard-link set of each FRN in `frns` to what the filesystem
/// actually reports.
///
/// A `HARD_LINK_CHANGE` record names one link and may name a dead one, so the
/// index cannot learn the real set from the journal. Without this a rename of a
/// file with several hard links was dropped entirely: the old name kept matching
/// and the new one never appeared.
///
/// Mirrors [`enrich_stats`]'s lock discipline, and for the same reason: the
/// Win32 walk is a blocking file open and must never run under the index write
/// lock that every query contends on. Only the apply takes the lock.
///
/// An FRN whose walk fails is SKIPPED, never reconciled to an empty set:
/// `reconcile_links` treats empty as a delete, so a transient lock would
/// otherwise erase a live file from the index.
fn reconcile_links(
    state: &VolumeState,
    handle: &VolumeHandle,
    frns: &[Frn],
    stop: &AtomicBool,
    pulse: &TailPulse,
) {
    if frns.is_empty() {
        return;
    }
    let mut resolved: Vec<(Frn, Vec<LinkTarget>)> = Vec::new();
    let mut dropped = 0u64;

    for &frn in frns {
        // Teardown must not wait behind a long walk batch: each iteration is
        // a blocking open, and dozens on a slow disk outlast the teardown
        // deadline. The daemon is exiting; the results would never be served.
        if stop.load(Ordering::Relaxed) {
            return;
        }
        // Per-file progress IS liveness: without the stamp, a long batch on
        // a slow disk reads as a stale beat, which delays the supervisor's
        // budget refund and inches toward a spurious stall verdict.
        pulse.beat();
        let links = match gozler_winfs::file_links(handle, frn.0) {
            Ok(Some(l)) if !l.is_empty() => l,
            // Gone: leave the entry alone. The journal's delete record
            // removes it.
            Ok(_) => continue,
            Err(e) => {
                tracing::debug!(volume = %state.mount_prefix(), ?frn, error = %e, "link walk failed");
                dropped += 1;
                continue;
            }
        };

        // Find each link's folder by its file ID, under one short read lock.
        // Its name would do only if the index were current, and during the
        // replay of a build it is not: a path can name a folder that a later
        // record renames or swaps.
        let index = state.index.read();
        let targets: Option<Vec<LinkTarget>> = links
            .iter()
            .map(|link| {
                let parent = index.head_of(Frn(link.folder))?;
                let mut name = Vec::new();
                gozler_core::wtf8::from_utf16(&link.name, &mut name);
                Some(LinkTarget { parent, name })
            })
            .collect();
        drop(index);

        // A folder we have not indexed yet: reconciling to a partial set
        // would delete the links we could not resolve.
        let Some(targets) = targets else {
            dropped += 1;
            continue;
        };
        resolved.push((frn, targets));
    }

    if dropped > 0 {
        // Not silent: a reconcile we could not perform means names on this
        // volume may be stale until the next rescan, and `--status` says so.
        state.index.write().note_link_reconciles_dropped(dropped);
    }
    if resolved.is_empty() {
        return;
    }
    let mut index = state.index.write();
    for (frn, targets) in &resolved {
        index.reconcile_links(*frn, targets);
    }
}

/// Refreshes size/mtime for `frns` (a file created or modified after bootstrap
/// has no size/date in the USN record; without this it stays "unknown", which
/// the CSV output renders as a directory). Stats every file with the write lock
/// RELEASED, since `OpenFileById` must never run under it, then applies the whole
/// batch under one short lock.
fn enrich_stats(
    state: &VolumeState,
    handle: &VolumeHandle,
    frns: &[Frn],
    stop: &AtomicBool,
    pulse: &TailPulse,
) {
    if frns.is_empty() {
        return;
    }
    let mut stats: Vec<(Frn, u64, i64)> = Vec::with_capacity(frns.len());
    for &frn in frns {
        // Same teardown escape as `reconcile_links`: a large dirty batch of
        // blocking opens must not make a healthy tail look wedged.
        if stop.load(Ordering::Relaxed) {
            break; // apply what was already gathered
        }
        // Per-file liveness stamp, same reason as `reconcile_links`.
        pulse.beat();
        match stat_file(handle, frn.0) {
            Ok(Some(s)) => stats.push((frn, s.size, s.mtime_ft)),
            Ok(None) => {} // gone or locked: nothing to refresh
            Err(e) => {
                tracing::debug!(volume = %state.mount_prefix(), ?frn, error = %e, "live stat failed")
            }
        }
    }
    if stats.is_empty() {
        return;
    }
    let mut index = state.index.write();
    for (frn, size, mtime) in stats {
        index.set_stat(frn, size, mtime);
    }
}

/// Lists each folder in `folders`, which the index lacked until a rename
/// brought it in, and everything under it, and inserts what it lists.
///
/// Mirrors [`reconcile_links`]'s lock discipline: the folder opens run with
/// the index lock released, beating the pulse for each folder, and the whole
/// listing is applied under one short lock. A stop ends the walk and applies
/// nothing, because the daemon is exiting.
fn walk_missed(
    state: &VolumeState,
    handle: &VolumeHandle,
    folders: &[Frn],
    stop: &AtomicBool,
    pulse: &TailPulse,
) -> anyhow::Result<()> {
    if folders.is_empty() {
        return Ok(());
    }
    let mut listed = Vec::new();
    let cancelled = || {
        pulse.beat();
        stop.load(Ordering::Relaxed)
    };
    if !bootstrap::walk_folders(handle, folders, cancelled, |files| {
        listed.extend_from_slice(files);
    })? {
        return Ok(());
    }
    tracing::debug!(
        volume = %state.mount_prefix(),
        folders = folders.len(),
        entries = listed.len(),
        "listed folders the index missed"
    );
    let mut index = state.index.write();
    for file in &listed {
        index.insert_listed(file);
    }
    Ok(())
}

/// Rescan with a storm guard: rebuilds the index, capping consecutive attempts
/// so a persistently broken journal cannot loop forever. Returns where to
/// resume reading, or `None` to stop the tail (rebuild failed, no journal
/// after rebuild, or the cap was hit; the volume is left Failed in the first
/// and last cases).
fn bounded_rescan(
    state: &VolumeState,
    handle: &VolumeHandle,
    pulse: &TailPulse,
    reason: RescanReason,
    detail: &str,
    rescans: &mut u32,
    stop: &AtomicBool,
) -> Option<ReadPosition> {
    *rescans += 1;
    if *rescans > MAX_RESCANS {
        mark_failed(
            state,
            reason,
            &format!("{detail}: too many rescans, giving up"),
        );
        return None;
    }
    rescan(state, handle, pulse, reason, detail, stop)
}

/// Builds this volume's index in place: its first build, or a rebuild for
/// `reason`, which the log gives with `detail`. Reads the volume on the
/// handle the tail already owns, swaps the fresh index in under the write lock,
/// and returns where to read the journal from, or `None` to stop the tail.
/// Queries keep serving (flagged incomplete) during the build.
fn rescan(
    state: &VolumeState,
    handle: &VolumeHandle,
    pulse: &TailPulse,
    reason: RescanReason,
    detail: &str,
    stop: &AtomicBool,
) -> Option<ReadPosition> {
    // The rebuild rereads the whole volume without stamping the pulse.
    // `Busy` tells the supervisor this silence is legitimate; the guard
    // restores a fresh beat on every way out of this function.
    let _busy = pulse.busy_scope();
    // A volume's first build keeps the phase `Bootstrapping`, so `--status`
    // tells a volume that was never indexed from one being rebuilt.
    if *state.phase.read() == VolumePhase::Bootstrapping {
        tracing::info!(volume = %state.mount_prefix(), "indexing volume");
    } else {
        tracing::warn!(volume = %state.mount_prefix(), ?reason, detail, "rebuilding index");
        *state.phase.write() = VolumePhase::Rescanning;
    }

    // Capture the cursor BEFORE reading the volume. A change landing after its
    // MFT region or folder was read but before a late cursor capture would be in
    // neither the snapshot nor the replay, and nothing would ever notice;
    // captured first, such changes replay from the journal. The other race is
    // safe: a journal deleted or recreated DURING the rebuild invalidates this
    // cursor, the first read from it fails, and the ordinary error arms
    // trigger another rescan. A stale cursor is loud, a late cursor is silent.
    let journal = bootstrap::ensure_journal(handle, &state.guid);

    // Pass the stop flag so a shutdown arriving mid-rebuild aborts promptly
    // instead of blocking `TailHandle::shutdown`'s join for the full read.
    let built = match bootstrap::build_index(handle, &state.guid, state.filesystem, Some(stop)) {
        Ok(Some(built)) => built,
        // Aborted by shutdown: leave the old index in place and stop cleanly;
        // this is NOT a failure, so do not mark the volume Failed.
        Ok(None) => return None,
        Err(e) => {
            mark_failed(state, reason, &format!("rescan rebuild failed: {e:#}"));
            return None;
        }
    };

    {
        let mut index = state.index.write();
        *index = built.index;
        // Set before the phase returns to Live, so a query that reads Live
        // also reads the new index's flag rather than the old one's.
        *state.metadata_pending.write() = built.metadata_pending;
        *state.journal_max_bytes.write() = match &journal {
            JournalSetup::Active(c) => Some(c.maximum_size),
            JournalSetup::Refused | JournalSetup::Unavailable => None,
        };
    }

    match journal {
        JournalSetup::Active(c) => {
            *state.phase.write() = VolumePhase::Live;
            tracing::info!(
                volume = %state.mount_prefix(),
                entries = state.index.read().len(),
                "index built; tailing the journal"
            );
            Some(ReadPosition::at(c, state.filesystem))
        }
        JournalSetup::Refused => {
            // Snapshot, not Offline: nothing is wrong and nothing will change,
            // so the volume must not flag every result incomplete. The tail
            // stops, and the supervisor sees the phase and does not revive it.
            *state.phase.write() = VolumePhase::Snapshot;
            tracing::info!(
                volume = %state.mount_prefix(),
                "no USN journal; volume is indexed but its index is a snapshot and cannot track changes"
            );
            None
        }
        JournalSetup::Unavailable => {
            // Rebuilt and correct, but no journal to tail right now: fresh as
            // of now but will not receive live updates. Report Offline so
            // Status/query responses flag the volume incomplete instead of
            // falsely Live; the tail stops and the supervisor revives it.
            *state.phase.write() = VolumePhase::Offline;
            tracing::warn!(volume = %state.mount_prefix(), "index built but no journal is available; live updates paused");
            None
        }
    }
}

/// Sleeps up to `total`, waking early if `stop` flips. Plain `sleep` here
/// would hold a stopping tail hostage to its own backoff (up to 30 s), which
/// teardown's cancel-and-poll rescue would misread as a wedged thread.
fn sleep_unless_stopped(stop: &AtomicBool, total: Duration) {
    let deadline = Instant::now() + total;
    while !stop.load(Ordering::Relaxed) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return;
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

fn mark_failed(state: &VolumeState, reason: RescanReason, detail: &str) {
    // The tail is about to exit for good. Without this line the volume died
    // silently: nothing else announces the exit, and the supervisor's later
    // revival attempts were the first evidence anything happened.
    tracing::error!(
        volume = %state.mount_prefix(),
        ?reason,
        detail,
        "volume failed; its tail is stopping and live updates have ended"
    );
    *state.phase.write() = VolumePhase::Failed {
        reason: format!("{reason:?}: {detail}"),
    };
}

#[cfg(test)]
mod tests {
    use super::{DEGRADE_AFTER, ESCALATE_AFTER, REASON_MASK, RETRY_BACKOFF_MAX, RETRY_BACKOFF_MIN};
    use gozler_core::types::Frn;
    use gozler_core::usn::ops_for;
    use gozler_core::usn::record::ParsedUsnRecord;
    use std::time::Duration;

    fn record_with(reason: u32) -> ParsedUsnRecord {
        ParsedUsnRecord {
            frn: Frn(42),
            parent_frn: Frn(5),
            reason,
            attributes: 0,
            name: b"f.txt".to_vec(),
        }
    }

    /// `REASON_MASK` is what we ask the kernel to report; `ops_for` (over in
    /// gozler-core) decides what an arriving record does to the index. Nothing
    /// links the two, and they live in different crates.
    ///
    /// If a bit ever drives an op but is absent from the mask, the kernel never
    /// reports it, `apply_batch` never sees it, and that class of update is
    /// dropped forever. No crash, no log, no other failing test. So sweep the
    /// whole 32-bit reason space rather than a hand-listed set of constants: a
    /// bit added to `ops_for` later is caught even if nobody updates this test.
    #[test]
    fn reason_mask_covers_every_bit_that_drives_an_index_op() {
        for pos in 0..32 {
            let bit = 1u32 << pos;
            let ops = ops_for(&record_with(bit));
            if !ops.is_empty() {
                assert_ne!(
                    REASON_MASK & bit,
                    0,
                    "reason bit {bit:#010x} yields {ops:?}, but REASON_MASK omits it: \
                     the kernel would never report it and the update would vanish silently"
                );
            }
        }
    }

    /// The read-retry policy exists to stop the one state the design forbids: a
    /// volume whose journal reads keep failing while it still reports `Live`, so
    /// `--status`, `Hello.ready`, and every result page call the index current
    /// when it has stopped tracking the volume entirely.
    ///
    /// The loop itself needs a real `VolumeHandle` and cannot be unit-tested, so
    /// this pins the constants that bound it. The behaviour they drive is
    /// asserted by the gated Windows test that stops a volume mid-tail.
    #[test]
    fn retry_policy_is_bounded_and_degrades() {
        // Must tolerate a transient failure: a drive spinning up answers
        // ERROR_NOT_READY for a moment and must not flip the volume Offline.
        assert!(
            DEGRADE_AFTER >= Duration::from_secs(1),
            "degrading on the first failure would flap on every spin-up"
        );
        // Must not be so patient that a dead volume looks healthy for minutes.
        assert!(
            DEGRADE_AFTER <= Duration::from_secs(30),
            "a volume that stopped tracking must be reported promptly"
        );
        // Backoff must actually grow, or a permanently sick volume writes a log
        // line every RETRY_BACKOFF_MIN forever (this is what produced ~96k
        // identical lines in one run).
        assert!(RETRY_BACKOFF_MAX > RETRY_BACKOFF_MIN);
        // The first retry must be prompt enough that a blip costs no freshness.
        assert!(RETRY_BACKOFF_MIN <= Duration::from_secs(1));
        // Escalation must announce Offline first (honesty precedes surgery)
        // and outlast a spin-up's NOT_READY window (~15 s worst observed), or
        // every sleepy drive would eat a full MFT rebuild on wake.
        assert!(ESCALATE_AFTER > DEGRADE_AFTER);
        assert!(ESCALATE_AFTER >= Duration::from_secs(20));
        // But not so patient that an unmappable read error (the observed
        // ERROR_INVALID_PARAMETER zombie) parks a volume for minutes.
        assert!(ESCALATE_AFTER <= Duration::from_secs(120));
    }

    /// The converse direction is deliberately NOT asserted: the mask may name
    /// bits that yield no op on their own (`CLOSE` is exactly that, it only
    /// tells us a window ended), so requesting a superset is correct. This pins
    /// that the mask stays a superset and never becomes an equality by accident.
    #[test]
    fn close_is_requested_even_though_it_drives_no_op_alone() {
        use gozler_core::usn::record::USN_REASON_CLOSE;
        assert!(ops_for(&record_with(USN_REASON_CLOSE)).is_empty());
        assert_ne!(REASON_MASK & USN_REASON_CLOSE, 0);
    }
}
