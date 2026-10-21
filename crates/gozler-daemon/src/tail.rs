//! Per-volume USN journal tail: one dedicated OS thread that builds its
//! volume's index, then reads the change journal from the build's cursor
//! forward and applies each batch to the shared index, keeping it live.
//!
//! It polls with a non-blocking drain and a short sleep when caught up, which
//! gives sub-100 ms freshness and a clean, `CancelSynchronousIo`-free shutdown,
//! unlike a blocking read with `BytesToWaitFor=1`. Each batch goes through
//! [`settle_batch`], which applies it and reads from the volume what its
//! records leave out; a missed folder that cannot be listed starts a rescan.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gozler_core::include::VolumeRules;
use gozler_core::index::settle::{Batch, settle_batch};
use gozler_core::usn::record::{
    USN_REASON_BASIC_INFO_CHANGE, USN_REASON_CLOSE, USN_REASON_DATA_EXTEND,
    USN_REASON_DATA_OVERWRITE, USN_REASON_DATA_TRUNCATION, USN_REASON_FILE_CREATE,
    USN_REASON_FILE_DELETE, USN_REASON_HARD_LINK_CHANGE, USN_REASON_RENAME_NEW_NAME,
    USN_REASON_RENAME_OLD_NAME,
};
use gozler_core::usn::walk_journal_buffer;
use gozler_winfs::{
    ERROR_JOURNAL_DELETE_IN_PROGRESS, ERROR_JOURNAL_ENTRY_DELETED, ERROR_JOURNAL_NOT_ACTIVE,
    JournalInfo, VolumeHandle, read_usn_journal,
};

use crate::bootstrap::{self, Disk, Filesystem, JournalSetup};
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
    /// Journal reads kept failing past [`ESCALATE_AFTER`] with codes no arm
    /// maps; whatever the cause, a journal that will not stop failing cannot be
    /// trusted.
    ReadsKeptFailing,
    /// A folder the index missed could not be listed, so its contents stay
    /// missing.
    WalkFailed,
    /// The Include settings now keep entries the index left out, which only
    /// a read of the volume brings back.
    IncludeChanged,
}

/// 512 KiB journal read buffer.
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
/// window (up to about 15 s) never triggers a needless rebuild, short enough
/// that an unmappable error (see the escalation comment in the read loop)
/// costs seconds of zombie time, not forever.
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
    /// retired. Stall verdicts read only this, never the volume's phase.
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
    // Thread creation can fail near the machine's thread or handle limit.
    // Propagate it so the supervisor retries this volume instead of the
    // daemon panicking.
    let join = std::thread::Builder::new()
        .name(format!("gozler-tail-{mount}"))
        .spawn(move || {
            let io = gozler_winfs::current_thread_io_handle();
            let usable = io.is_ok();
            let _ = io_tx.send(io);
            if !usable {
                // `spawn` is about to report failure and drop this thread's
                // stop flag and join handle. Running on would leave a live
                // index writer that nobody can stop or supervise, and the
                // supervisor's retry would stack a second writer on the same
                // volume.
                return;
            }
            // Publishes `Retired` on any exit, a panic unwind included. The
            // handle moves into tail_loop so that it closes before this guard
            // fires.
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
    /// This run moved the volume to Offline, so only its end moves it back.
    degraded: bool,
}

/// What the tail does after a failed journal read.
#[derive(Debug, PartialEq, Eq)]
enum FailedRead {
    /// Sleep `sleep`, then read again. `newly_degraded` is true for the one
    /// failure that crossed [`DEGRADE_AFTER`]: the caller must then move the
    /// volume to Offline.
    Retry {
        sleep: Duration,
        newly_degraded: bool,
    },
    /// The run passed [`ESCALATE_AFTER`]: stop retrying and rebuild.
    Escalate,
}

impl ReadFailures {
    /// A run whose first failed read happened at `now`.
    fn new(now: Instant) -> Self {
        Self {
            since: now,
            backoff: RETRY_BACKOFF_MIN,
            degraded: false,
        }
    }

    /// Records a failed read at `now` and says what to do next. The backoff
    /// doubles on each retry, up to [`RETRY_BACKOFF_MAX`].
    fn read_failed(&mut self, now: Instant) -> FailedRead {
        let failing_for = now.duration_since(self.since);
        if failing_for >= ESCALATE_AFTER {
            return FailedRead::Escalate;
        }
        let newly_degraded = !self.degraded && failing_for >= DEGRADE_AFTER;
        self.degraded |= newly_degraded;
        let sleep = self.backoff;
        self.backoff = (self.backoff * 2).min(RETRY_BACKOFF_MAX);
        FailedRead::Retry {
            sleep,
            newly_degraded,
        }
    }

    /// Ends the run at a good read. Answers whether the run had moved the
    /// volume to Offline, which the caller must then undo.
    fn read_succeeded(self) -> bool {
        self.degraded
    }
}

fn tail_loop(state: &VolumeState, handle: VolumeHandle, stop: &AtomicBool, pulse: &TailPulse) {
    // Owned so it drops in THIS frame, on return and on unwind alike, before
    // the spawning closure's retire guard publishes `Retired`. `Retired`
    // promises that joining is instant, and a close is itself an IRP a sick
    // device can pend; a thread that claimed it while wedged in CloseHandle
    // would draw an unbounded join from the supervisor's give-up or revive
    // path.
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
    let mut disk = Disk::new(handle, state.mount_prefix());

    'tail: while !stop.load(Ordering::Relaxed) {
        // Liveness stamp for the supervisor: as long as this loop is turning,
        // however unhealthily, no one cancels our I/O.
        pulse.beat();
        // Here, between batches, so a change of the rules never races one.
        // Not a journal fault, so it costs nothing of the rescan budget.
        if let Some(rules) = state.take_new_rules()
            && !apply_rules_in_place(state, rules)
        {
            let Some(fresh) = rescan(
                state,
                handle,
                pulse,
                RescanReason::IncludeChanged,
                "the Include settings keep entries the index left out",
                stop,
            ) else {
                return;
            };
            pos = fresh;
            continue 'tail;
        }
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
                    if pos
                        .failures
                        .take()
                        .is_some_and(ReadFailures::read_succeeded)
                    {
                        tracing::info!(
                            volume = %state.mount_prefix(),
                            "journal reads recovered; volume is tracking again"
                        );
                        state.reads_recovered();
                    }
                    let walk = match walk_journal_buffer(&buf[..bytes]) {
                        Ok(w) => w,
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
                    let batch = Batch {
                        records: &walk.records,
                        replaying_walk: pos.replaying_walk,
                    };
                    // The settle asks before each folder and file it reads.
                    // Teardown must not wait behind a long batch: each read is
                    // a blocking open, and dozens on a slow disk outlast the
                    // teardown deadline. And that progress is liveness: without
                    // the stamp, a long batch on a slow disk reads as a stale
                    // beat, which delays the supervisor's budget refund and
                    // inches toward a spurious stall verdict.
                    let cancelled = || {
                        pulse.beat();
                        stop.load(Ordering::Relaxed)
                    };
                    let settled = match settle_batch(state, &mut disk, batch, cancelled) {
                        Ok(settled) => settled,
                        Err(e) => break 'rescan (RescanReason::WalkFailed, e.to_string()),
                    };
                    for anomaly in &settled.anomalies {
                        tracing::debug!(volume = %state.mount_prefix(), ?anomaly, "index anomaly");
                    }
                    if settled.missed_folders > 0 {
                        tracing::debug!(
                            volume = %state.mount_prefix(),
                            folders = settled.missed_folders,
                            entries = settled.missed_entries,
                            "listed folders the index missed"
                        );
                    }
                    if settled.wants_compact {
                        state.compact_index();
                    }
                    pos.start_usn = walk.next_usn;
                    rescans = 0;
                    continue 'tail;
                }
                Err(e) if e.code == ERROR_JOURNAL_ENTRY_DELETED => {
                    (RescanReason::EntryDeletedError, e.to_string())
                }
                Err(e)
                    if e.code == ERROR_JOURNAL_NOT_ACTIVE
                        || e.code == ERROR_JOURNAL_DELETE_IN_PROGRESS =>
                {
                    // Retrying the same cursor against a gone or dying journal
                    // can never succeed.
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
                    // Never retry silently while still claiming `Live`: the index
                    // has stopped tracking the volume, yet every query would report
                    // it complete. Past `DEGRADE_AFTER` the drift is real, so say
                    // so and let `--status`, `Hello.ready`, and every result page
                    // carry it.
                    let now = Instant::now();
                    let failures = pos.failures.get_or_insert_with(|| ReadFailures::new(now));
                    // Nor may we retry forever: deleting the journal mid-read can
                    // answer the stale cursor with ERROR_INVALID_PARAMETER (87), a
                    // code no arm maps, and no retry of that can succeed. The
                    // volume would sit Offline while its tail beat away and the
                    // supervisor saw nothing to revive. Past `ESCALATE_AFTER` the
                    // code no longer matters: reads that will not stop failing mean
                    // the journal cannot be trusted, so rebuild. A rescan that fails
                    // retires the tail into the supervisor's revival budget instead
                    // of looping here.
                    let FailedRead::Retry {
                        sleep,
                        newly_degraded,
                    } = failures.read_failed(now)
                    else {
                        break 'rescan (RescanReason::ReadsKeptFailing, e.to_string());
                    };
                    if newly_degraded {
                        tracing::error!(
                            volume = %state.mount_prefix(),
                            error = %e,
                            failing_for_secs = now.duration_since(failures.since).as_secs(),
                            "journal reads keep failing; marking volume Offline (its index is no longer current)"
                        );
                        state.reads_keep_failing();
                    } else {
                        tracing::warn!(volume = %state.mount_prefix(), error = %e, "journal read failed; retrying");
                    }
                    sleep_unless_stopped(stop, sleep);
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
    // The rebuild stamps no pulse; `Busy` tells the supervisor that silence
    // is legitimate.
    let _busy = pulse.busy_scope();
    let building = state.begin_build();
    if building.is_first() {
        tracing::info!(volume = %state.mount_prefix(), "indexing volume");
    } else {
        tracing::warn!(volume = %state.mount_prefix(), ?reason, detail, "rebuilding index");
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
    let rules = state.rules();
    let built =
        match bootstrap::build_index(handle, &state.guid, state.filesystem, &rules, Some(stop)) {
            Ok(Some(built)) => built,
            // Aborted by shutdown: leave the old index in place and stop cleanly;
            // this is NOT a failure, so do not mark the volume Failed.
            Ok(None) => return None,
            Err(e) => {
                mark_failed(state, reason, &format!("rescan rebuild failed: {e:#}"));
                return None;
            }
        };

    let entries = built.index.len();
    building.finish(built, &journal);
    // The entries the rules removed left their names behind.
    state.compact_index();

    match journal {
        JournalSetup::Active(c) => {
            tracing::info!(
                volume = %state.mount_prefix(),
                entries,
                "index built; tailing the journal"
            );
            Some(ReadPosition::at(c, state.filesystem))
        }
        JournalSetup::Refused => {
            // The tail stops, and the supervisor sees the Snapshot phase and
            // does not revive it, unless the Include settings changed during
            // the build.
            tracing::info!(
                volume = %state.mount_prefix(),
                "no USN journal; volume is indexed but its index is a snapshot and cannot track changes"
            );
            None
        }
        JournalSetup::Unavailable => {
            // The tail stops and the supervisor revives it.
            tracing::warn!(volume = %state.mount_prefix(), "index built but no journal is available; live updates paused");
            None
        }
    }
}

/// Applies new Include rules to the index when they only leave more out, and
/// answers whether it did. Rules that keep anything the index left out need a
/// rebuild, because the index no longer holds it.
fn apply_rules_in_place(state: &VolumeState, rules: Arc<VolumeRules>) -> bool {
    let mut index = state.index_mut();
    if index
        .include_rules()
        .is_some_and(|applied| rules.adds_to(applied))
    {
        return false;
    }
    tracing::info!(volume = %state.mount_prefix(), "applying the changed Include settings");
    index.apply_rules(rules);
    index.optimize_storage();
    drop(index);
    state.compact_index();
    true
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
    // The tail is about to exit for good, and nothing else announces it.
    tracing::error!(
        volume = %state.mount_prefix(),
        ?reason,
        detail,
        "volume failed; its tail is stopping and live updates have ended"
    );
    state.fail(format!("{reason:?}: {detail}"));
}

#[cfg(test)]
mod tests {
    use super::{
        DEGRADE_AFTER, FailedRead, REASON_MASK, RETRY_BACKOFF_MAX, RETRY_BACKOFF_MIN, ReadFailures,
    };
    use gozler_core::types::Frn;
    use gozler_core::usn::ops_for;
    use gozler_core::usn::record::ParsedUsnRecord;
    use std::time::{Duration, Instant};

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

    fn retry_sleep(outcome: FailedRead) -> Duration {
        match outcome {
            FailedRead::Retry { sleep, .. } => sleep,
            FailedRead::Escalate => panic!("expected a retry, got an escalation"),
        }
    }

    /// A drive spinning up answers `ERROR_NOT_READY` for up to about 15 s.
    /// If a run of failures that short escalated, every sleepy drive would eat
    /// a full index rebuild on wake.
    #[test]
    fn a_spin_up_window_of_failures_never_escalates() {
        let start = Instant::now();
        let mut run = ReadFailures::new(start);
        for secs in 0..=15 {
            let outcome = run.read_failed(start + Duration::from_secs(secs));
            assert!(
                matches!(outcome, FailedRead::Retry { .. }),
                "escalated {secs} s into the run: {outcome:?}"
            );
        }
    }

    /// An unmappable read error (for example `ERROR_INVALID_PARAMETER` after
    /// the journal was deleted mid-read) can never succeed on retry. If the run
    /// did not escalate, the volume would stay Offline for good while its tail
    /// kept beating.
    #[test]
    fn failures_past_thirty_seconds_escalate() {
        let start = Instant::now();
        let mut run = ReadFailures::new(start);
        assert_eq!(
            run.read_failed(start + Duration::from_secs(30)),
            FailedRead::Escalate
        );
    }

    /// `reads_recovered` marks the volume Live again. Reporting it after a run
    /// that never degraded the volume would be wrong, and not reporting it
    /// after a degrade would leave the volume Offline for good. The degrade is
    /// also reported once: a repeat would call `reads_keep_failing` on every
    /// failed read.
    #[test]
    fn recovery_is_reported_only_after_a_degrade() {
        let start = Instant::now();

        let mut blip = ReadFailures::new(start);
        assert_eq!(
            blip.read_failed(start),
            FailedRead::Retry {
                sleep: RETRY_BACKOFF_MIN,
                newly_degraded: false
            }
        );
        assert!(!blip.read_succeeded());

        let mut outage = ReadFailures::new(start);
        let degrading = outage.read_failed(start + DEGRADE_AFTER);
        assert!(matches!(
            degrading,
            FailedRead::Retry {
                newly_degraded: true,
                ..
            }
        ));
        let repeat = outage.read_failed(start + DEGRADE_AFTER + Duration::from_secs(1));
        assert!(matches!(
            repeat,
            FailedRead::Retry {
                newly_degraded: false,
                ..
            }
        ));
        assert!(outage.read_succeeded());
    }

    /// If the backoff did not grow, a permanently sick volume would write a
    /// log line every `RETRY_BACKOFF_MIN` for the life of the service. If it
    /// passed `RETRY_BACKOFF_MAX`, the supervisor's stall threshold, which
    /// is set above that ceiling, would misread a sleeping tail as wedged.
    #[test]
    fn backoff_doubles_from_the_minimum_and_stops_at_the_maximum() {
        let start = Instant::now();
        let mut run = ReadFailures::new(start);
        let mut previous: Option<Duration> = None;
        for _ in 0..12 {
            let sleep = retry_sleep(run.read_failed(start));
            match previous {
                None => assert_eq!(sleep, RETRY_BACKOFF_MIN),
                Some(before) => assert_eq!(sleep, (before * 2).min(RETRY_BACKOFF_MAX)),
            }
            previous = Some(sleep);
        }
        assert_eq!(previous, Some(RETRY_BACKOFF_MAX));
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
