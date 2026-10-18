//! Volume supervision: one background thread that keeps a tail on every
//! indexed volume and keeps the indexed volumes in step with the machine.
//!
//! A volume is indexed by its tail: the supervisor opens the volume and spawns
//! a tail without a cursor, whose first act is the full build. That one path
//! serves a volume found at startup, a volume that appears later, a volume
//! whose start failed (a disk still spinning up at boot), and a tail that
//! exited. Each scan the supervisor also lists the machine's volumes, adds the
//! fixed NTFS volumes it does not index yet, and removes the indexed volumes
//! that are gone, so a query never silently leaves out an attached volume.
//!
//! For each tail it reads the [`Pulse`] and gives one of three verdicts, one
//! per pulse state:
//!
//! - a **stale beat** means the thread is wedged in a synchronous journal
//!   read (observed live: a spinning-down HDD pended one
//!   `FSCTL_READ_USN_JOURNAL` IRP forever), so its I/O is cancelled and the
//!   tail's ordinary retry path takes over;
//! - **busy** (a full MFT rebuild) is legitimate silence and is left alone,
//!   up to a far larger bound of its own, because a rebuild blocks in the
//!   same kind of syscall and can wedge the same way;
//! - **retired** means the thread exited (journal permanently lost, rescan
//!   budget spent, or a panic) and is revived on a bounded budget: the
//!   volume is reopened by GUID and a fresh tail spawned whose first act is
//!   a full rescan. A tail that keeps dying gets one `error!` and is dropped
//!   from supervision rather than alarmed about forever.
//!
//! The supervisor owns the [`TailHandle`]s outright: it is the only place
//! that can replace one (revival) and the natural place to stop them all, so
//! teardown is [`Supervisor::shutdown`] and nothing else holds a tail.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use gozler_core::types::VolumePhase;
use gozler_winfs::{VolumeInfo, WinError};
use parking_lot::{Condvar, Mutex};

use crate::bootstrap;
use crate::pulse::{Pulse, awake_secs};
use crate::tail::{self, TailHandle};
use crate::volume_state::{VolumeRegistry, VolumeState};

/// Pause between supervision scans.
const SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// How long a beat may go stale before the tail is presumed wedged and its
/// synchronous I/O cancelled. Must comfortably exceed the tail's retry
/// backoff ceiling plus a scan interval, or a healthy tail sleeping out its
/// backoff could be shot mid-nap; a test pins that coupling.
const BEAT_STALL: Duration = Duration::from_secs(300);

/// How long `Busy` (a full MFT rebuild) may run before it too is presumed
/// wedged. Rebuilds block in the same synchronous ioctls as journal reads
/// and can wedge the same way, so exempting them forever would leave the
/// founding failure unrescuable whenever it struck mid-rescan. Generous:
/// a healthy rebuild is tens of seconds, minutes on a huge slow disk.
const BUSY_STALL: Duration = Duration::from_secs(30 * 60);

/// Revival attempts a tail gets before the supervisor stops trying. The
/// budget is consecutive: staying healthy for [`HEALTHY_AFTER`] refunds it,
/// so a volume that dies once a month is revived every time, while one that
/// dies every scan is given up on within minutes.
const MAX_REVIVES: u32 = 3;

/// How long a revived tail must beat CONTINUOUSLY before its failure history
/// is forgotten. Continuous beating, not incarnation age: a revived tail can
/// spend longer than this in its initial rebuild alone, and refunding on age
/// would let a volume that dies right after every rebuild revive forever,
/// one full MFT rebuild per cycle, the exact storm the budget exists to stop.
const HEALTHY_AFTER: Duration = Duration::from_secs(10 * 60);

/// How stale an observed beat may be and still count toward the refund. A
/// wedged tail's stamp freezes, and until [`BEAT_STALL`] declares the wedge
/// the frozen stamp would otherwise keep an earlier anchor alive, letting
/// partially earned health mature into a refund while the tail hangs. Must
/// cover one scan interval (a beat is at most one scan old when observed)
/// and stay far under [`BEAT_STALL`]; a test pins both.
const BEAT_FRESH: Duration = Duration::from_secs(60);

/// What one scan of one tail decided. Pure data so [`assess`] stays a
/// function of its inputs and the whole decision table is unit-testable.
#[derive(Debug, PartialEq, Eq)]
enum Action {
    /// Healthy, or legitimately busy: nothing to do.
    Nothing,
    /// A revived tail has proven itself; forget its failure history.
    ResetBudget,
    /// Presumed wedged in a synchronous syscall: cancel its I/O.
    CancelIo { stalled_secs: u64 },
    /// No tail is running and the budget has room: start one.
    Revive,
    /// No tail is running and the budget is spent: stop supervising it.
    GiveUp,
}

/// Decides what to do about one tail. `proven_healthy` is whether the
/// current incarnation has beaten continuously for [`HEALTHY_AFTER`]; it
/// only matters alongside `revive_attempts > 0` (an original has no history
/// to refund).
fn assess(pulse: Pulse, now: u64, revive_attempts: u32, proven_healthy: bool) -> Action {
    match pulse {
        Pulse::Retired => {
            if revive_attempts >= MAX_REVIVES {
                Action::GiveUp
            } else {
                Action::Revive
            }
        }
        Pulse::Busy(since) => {
            let stalled_secs = now.saturating_sub(since);
            if stalled_secs >= BUSY_STALL.as_secs() {
                Action::CancelIo { stalled_secs }
            } else {
                Action::Nothing
            }
        }
        Pulse::Beat(at) => {
            let stalled_secs = now.saturating_sub(at);
            if stalled_secs >= BEAT_STALL.as_secs() {
                Action::CancelIo { stalled_secs }
            } else if revive_attempts > 0 && proven_healthy {
                Action::ResetBudget
            } else {
                Action::Nothing
            }
        }
    }
}

/// Advances the continuous-health anchor for one observation. Health means
/// FRESH beats observed while the volume's phase is `Live`, and nothing
/// else. A beat with the phase Offline is a turning loop that tracks
/// nothing (the read-retry path beats once per backoff turn forever), and a
/// stale beat is a loop that has stopped turning; counting either would
/// refund the budget of a volume delivering zero service and make give-up
/// unreachable, the exact churn the budget exists to stop. This is the one
/// place the supervisor reads the phase, and it reads it for what the phase
/// means (is the volume served?), never to decide stall handling.
fn accrue_health(
    prev: Option<Instant>,
    pulse: Pulse,
    live: bool,
    now_secs: u64,
    now: Instant,
) -> Option<Instant> {
    match pulse {
        Pulse::Beat(at) if live && now_secs.saturating_sub(at) < BEAT_FRESH.as_secs() => {
            Some(prev.unwrap_or(now))
        }
        _ => None,
    }
}

/// Shutdown signal: a flag under a mutex so the scan sleep is interruptible.
/// Teardown flips it and wakes the condvar, so stopping never waits out a
/// full [`SCAN_INTERVAL`].
struct StopFlag {
    stopped: Mutex<bool>,
    wake: Condvar,
}

/// The supervision thread and the means to stop it. Owns every tail: they
/// live inside the thread while it runs and are shut down by it on exit.
pub(crate) struct Supervisor {
    stop: Arc<StopFlag>,
    join: Option<JoinHandle<()>>,
    volumes: Arc<VolumeRegistry>,
}

impl Supervisor {
    /// Starts indexing every fixed NTFS volume in `enumerated`, the machine's
    /// volumes at startup, and starts the supervision thread. Returns as soon
    /// as each of those volumes is in [`Supervisor::volumes`] as
    /// `Bootstrapping`, so queries are answered, flagged incomplete, while the
    /// volumes index. Fails only when the OS refuses the thread; nothing would
    /// index the volumes then.
    pub(crate) fn spawn(enumerated: Vec<VolumeInfo>) -> std::io::Result<Supervisor> {
        let volumes = Arc::new(VolumeRegistry::default());
        let mut fleet = Fleet {
            volumes: volumes.clone(),
            supervised: Vec::new(),
            skipped: HashSet::new(),
            discovery: None,
            starting: Some(Instant::now()),
        };
        fleet.reconcile(enumerated);
        let stop = Arc::new(StopFlag {
            stopped: Mutex::new(false),
            wake: Condvar::new(),
        });
        // `Builder::spawn` consumes its closure even when thread creation
        // fails, so the fleet rides in a slot the failure path can take back;
        // dropping it would detach tails that could then never be joined.
        let slot = Arc::new(Mutex::new(Some(fleet)));
        let thread_slot = slot.clone();
        let thread_stop = stop.clone();
        match std::thread::Builder::new()
            .name("gozler-volume-supervisor".into())
            .spawn(move || {
                let fleet = thread_slot
                    .lock()
                    .take()
                    .expect("the supervisor's fleet is taken exactly once");
                supervise(fleet, &thread_stop);
            }) {
            Ok(join) => Ok(Supervisor {
                stop,
                join: Some(join),
                volumes,
            }),
            Err(e) => {
                if let Some(fleet) = slot.lock().take() {
                    fleet.stop_all();
                }
                Err(e)
            }
        }
    }

    /// The indexed volumes, which this supervisor keeps current.
    pub(crate) fn volumes(&self) -> Arc<VolumeRegistry> {
        self.volumes.clone()
    }

    /// Stops supervision and every tail, joining all of it. The supervision
    /// thread stops the tails itself (it owns them).
    ///
    /// The join is bounded by [`SUPERVISOR_JOIN_WAIT`], which is sized past
    /// what the thread's exit legitimately takes: stopping its tails
    /// ([`stop_tails`]) and resolving in-flight revivals ([`drain_pending`]).
    pub(crate) fn shutdown(mut self) {
        *self.stop.stopped.lock() = true;
        self.stop.wake.notify_all();
        if let Some(join) = self.join.take() {
            let deadline = Instant::now() + SUPERVISOR_JOIN_WAIT;
            while !join.is_finished() && Instant::now() < deadline {
                std::thread::sleep(TEARDOWN_POLL);
            }
            if join.is_finished() {
                let _ = join.join();
            } else {
                tracing::error!(
                    "volume supervisor survived its stop signal; abandoning it to process exit"
                );
            }
        }
    }
}

/// Everything the supervision thread owns.
struct Fleet {
    volumes: Arc<VolumeRegistry>,
    /// The indexed volumes that have a tail or may get one. A volume leaves
    /// this list, and stays indexed, when its tail is given up on or when it
    /// is a snapshot, which needs no tail.
    supervised: Vec<Supervised>,
    /// Volumes found that gozler does not index, each logged once.
    skipped: HashSet<String>,
    /// The enumeration running on its worker thread, if one is.
    discovery: Option<mpsc::Receiver<Result<Vec<VolumeInfo>, WinError>>>,
    /// When the daemon started, until no volume is `Bootstrapping`.
    starting: Option<Instant>,
}

impl Fleet {
    /// Brings the indexed volumes in step with `enumerated`, the volumes on
    /// the machine now: starts indexing each fixed NTFS volume that is not
    /// indexed, and removes each indexed volume that is no longer listed.
    ///
    /// Only a volume that is no longer listed is removed. A listed volume
    /// that now classifies differently stays: a filesystem query that fails
    /// once must not cost a full rebuild.
    fn reconcile(&mut self, enumerated: Vec<VolumeInfo>) {
        let indexed = self.volumes.current();
        for gone in indexed
            .iter()
            .filter(|v| !enumerated.iter().any(|e| e.guid_path == v.guid))
        {
            self.remove(gone);
        }
        self.skipped
            .retain(|guid| enumerated.iter().any(|e| &e.guid_path == guid));
        for vol in enumerated {
            if indexed.iter().any(|v| v.guid == vol.guid_path) {
                continue;
            }
            match bootstrap::skip_reason(&vol) {
                Some(reason) => {
                    if self.skipped.insert(vol.guid_path.clone()) {
                        tracing::info!(
                            volume = %vol.guid_path,
                            mounts = ?vol.mounts,
                            reason,
                            "volume not indexed"
                        );
                    }
                }
                None => {
                    self.skipped.remove(&vol.guid_path);
                    let state = Arc::new(VolumeState::new(vol.guid_path, vol.mounts));
                    tracing::info!(volume = %state.mount_prefix(), "volume found; indexing it");
                    self.volumes.add(state.clone());
                    self.supervised.push(Supervised::start(state));
                }
            }
        }
    }

    /// Stops indexing a volume that is gone: queries leave it out from now on,
    /// and its tail stops.
    fn remove(&mut self, gone: &VolumeState) {
        self.volumes.remove(&gone.guid);
        tracing::info!(volume = %gone.mount_prefix(), "volume is gone; no longer indexed");
        if let Some(i) = self
            .supervised
            .iter()
            .position(|s| s.state.guid == gone.guid)
        {
            let deadline = Instant::now() + PENDING_DRAIN_WAIT;
            stop_tails(self.supervised.swap_remove(i).into_tails(deadline));
        }
    }

    /// Reconciles the volumes with the last enumeration, if it has finished,
    /// and starts the next. Enumeration queries every volume's filesystem, and
    /// a sick device can block that query, so it runs on a worker thread; one
    /// still running is waited for, never stacked.
    fn discover(&mut self) {
        let finished = match &self.discovery {
            None => None,
            Some(rx) => match rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => return,
                Ok(result) => Some(result),
                // The worker died without reporting.
                Err(mpsc::TryRecvError::Disconnected) => None,
            },
        };
        self.discovery = None;
        match finished {
            Some(Ok(enumerated)) => self.reconcile(enumerated),
            Some(Err(e)) => {
                tracing::warn!(error = %e, "could not list volumes; trying again next scan")
            }
            None => {}
        }
        let (tx, rx) = mpsc::sync_channel(1);
        match std::thread::Builder::new()
            .name("gozler-volume-discovery".into())
            .spawn(move || {
                let _ = tx.send(gozler_winfs::enumerate_volumes());
            }) {
            Ok(_) => self.discovery = Some(rx),
            Err(e) => {
                tracing::warn!(error = %e, "could not start listing volumes; trying again next scan")
            }
        }
    }

    /// Once no volume is `Bootstrapping`, logs the startup totals and settles
    /// the working set, one time only.
    fn settle_once_indexed(&mut self) {
        let Some(started) = self.starting else {
            return;
        };
        let volumes = self.volumes.current();
        if volumes
            .iter()
            .any(|v| *v.phase.read() == VolumePhase::Bootstrapping)
        {
            return;
        }
        let entries: usize = volumes.iter().map(|v| v.index.read().len()).sum();
        tracing::info!(
            entries,
            volumes = volumes.len(),
            elapsed_s = started.elapsed().as_secs_f64(),
            "startup indexing finished"
        );
        crate::run::settle_working_set();
        self.starting = None;
    }

    /// Stops every tail, resolving in-flight starts first so a tail a worker
    /// hands over is stopped with the rest.
    fn stop_all(self) {
        let deadline = Instant::now() + PENDING_DRAIN_WAIT;
        let tails = self
            .supervised
            .into_iter()
            .flat_map(|s| s.into_tails(deadline))
            .collect();
        stop_tails(tails);
    }
}

/// One supervised volume: its tail plus its revival history.
struct Supervised {
    state: Arc<VolumeState>,
    /// `None` until a start succeeds.
    tail: Option<TailHandle>,
    /// Revivals since the budget was last refunded.
    revive_attempts: u32,
    /// When the current unbroken run of healthy observations (a fresh
    /// `Beat` with the phase `Live`, see [`accrue_health`]) began; cleared
    /// by anything else and by a revival. This, not the incarnation's age,
    /// is what earns a budget refund.
    beating_since: Option<Instant>,
    /// A start in flight on its worker thread, polled each scan. `Some`
    /// suppresses further actions on this volume until the worker reports.
    pending: Option<PendingRevival>,
    /// Why the last start failed, for the phase if the supervisor gives up
    /// before the volume was ever indexed.
    last_failure: Option<String>,
}

/// A tail start (the first, or a revival) running on its own short-lived
/// worker thread. It opens the volume, and that open is blocking I/O against
/// possibly-sick hardware; run inline it would wedge the scan loop and cost
/// every OTHER volume its stall rescue and revival, so the supervisor never
/// blocks on it.
struct PendingRevival {
    /// Delivers the worker's outcome over a RENDEZVOUS channel: the send can
    /// only complete into a live receive, never into a buffer, so there is
    /// no window where a tail sits sent-but-unowned. Once this receiver
    /// drops, the worker's send fails and the worker shuts down any tail it
    /// built; abandoning a wedged worker can never leak a live writer.
    rx: mpsc::Receiver<std::io::Result<TailHandle>>,
    /// Real handle to the worker thread, so a wedged `open_volume` can be
    /// cancelled; `None` only if duplication failed on the worker.
    io: Option<gozler_winfs::ThreadIoHandle>,
    /// Joined when the worker reports; abandoned if it never does.
    worker: JoinHandle<()>,
    /// The blocked-revival warning fires once, not per scan.
    reported_blocked: bool,
}

/// Grace given a stopping tail to notice its flag before the first cancel
/// wave, and between waves. Healthy tails exit within one idle poll or one
/// backoff slice (both 50 ms), so one poll of grace covers them all.
const TEARDOWN_POLL: Duration = Duration::from_millis(250);
/// Total time teardown fights for a tail before abandoning it. Covers every
/// clean exit path with a wide margin (backoff sleeps are interruptible,
/// rescans poll the stop flag between ioctls, and the stat/link-walk batches
/// check it per file); only a thread wedged in an IRP that ignores repeated
/// cancels can still be running at the deadline.
const TEARDOWN_WAIT: Duration = Duration::from_secs(5);

/// Total grace teardown gives ALL in-flight revival workers together, not
/// each: a shared deadline, so a machine with many volumes wedging at once
/// cannot stretch teardown past what [`SUPERVISOR_JOIN_WAIT`] covers.
const PENDING_DRAIN_WAIT: Duration = Duration::from_secs(3);

/// Total time [`Supervisor::shutdown`] waits for the supervision thread
/// itself. It must comfortably exceed the thread's legitimate teardown worst
/// case, [`TEARDOWN_WAIT`] plus [`PENDING_DRAIN_WAIT`]. A test pins the
/// coupling.
const SUPERVISOR_JOIN_WAIT: Duration = Duration::from_secs(15);

/// The supervisor thread body: run the scan loop, then stop every tail. The
/// scan loop is unwind-guarded because the tails were moved in here: a panic
/// that dropped their handles would detach every tail thread with no stop
/// flag set, leaving live index writers nobody could stop, join, or rescue,
/// while `Supervisor::shutdown` returned as if teardown had succeeded.
fn supervise(mut fleet: Fleet, stop: &StopFlag) {
    // AssertUnwindSafe: the closure touches only `fleet` (handles, plain
    // counters, and the registry's lock-guarded list), which stays
    // structurally valid wherever a panic lands; the worst carryover is a
    // half-updated budget on tails that are about to be stopped anyway.
    let scanning = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        scan_loop(&mut fleet, stop);
    }));
    if scanning.is_err() {
        // Do NOT stop the tails: they are healthy writers, and stopping
        // them would silently end tracking while every phase still says
        // Live and the query server keeps serving. Only supervision (stall
        // rescue, revival, volume discovery) is lost; park until shutdown so
        // teardown still stops and joins everything properly.
        tracing::error!(
            "volume supervisor panicked; tails keep running unsupervised (no stuck-I/O rescue, revival or new volumes) until shutdown"
        );
        let mut stopped = stop.stopped.lock();
        while !*stopped {
            stop.wake.wait(&mut stopped);
        }
    }
    fleet.stop_all();
}

/// Resolves one in-flight revival at teardown: cancel the worker's blocked
/// I/O, give it a moment to report, and take any tail it built so teardown
/// stops it with the rest. A worker that stays wedged is abandoned; dropping
/// its receiver makes its eventual send fail, and the worker then shuts down
/// anything it built, so nothing leaks.
fn drain_pending(pending: PendingRevival, mount: &str, deadline: Instant) -> Option<TailHandle> {
    if let Some(io) = &pending.io {
        let _ = gozler_winfs::cancel_synchronous_io(io);
    }
    // The deadline is shared across every pending drain; once it passes,
    // this degrades to a try_recv so a fleet of wedged workers costs
    // teardown [`PENDING_DRAIN_WAIT`] in total, not each.
    let grace = deadline.saturating_duration_since(Instant::now());
    match pending.rx.recv_timeout(grace) {
        Ok(outcome) => {
            let _ = pending.worker.join();
            outcome.ok()
        }
        Err(_) => {
            tracing::warn!(
                volume = %mount,
                "a tail start is still blocked at shutdown; abandoning it (it cleans up anything it builds)"
            );
            None
        }
    }
}

/// Scan every tail each interval, act on its pulse, and drop the ones that
/// need no more supervision; then bring the volumes in step with the machine.
/// Returns when the stop flag flips.
fn scan_loop(fleet: &mut Fleet, stop: &StopFlag) {
    loop {
        {
            let mut stopped = stop.stopped.lock();
            if !*stopped {
                stop.wake.wait_for(&mut stopped, SCAN_INTERVAL);
            }
            if *stopped {
                return;
            }
        }
        let now = awake_secs();
        let supervised = &mut fleet.supervised;
        let mut i = 0;
        while i < supervised.len() {
            if supervised[i].scan(now) {
                i += 1;
            } else if let Some(tail) = supervised.swap_remove(i).tail {
                // Given up on, or a snapshot: its thread already retired, so
                // this join is immediate. Dropping the entry releases the
                // pulse and the duplicated thread handle, which the old
                // watchdog held (and alarmed through) for the life of the
                // process.
                tail.shutdown();
            }
        }
        // After the scan, so a volume found now gets its first poll a full
        // scan interval after its start, the grace `poll_revival` assumes.
        fleet.discover();
        fleet.settle_once_indexed();
    }
}

/// Stops every tail, rescuing wedged ones: flag them all first, then
/// repeatedly cancel blocked synchronous I/O until each thread exits. A
/// thread inside a pended IRP can never observe its stop flag (the founding
/// failure), so a bare join here would hang teardown forever. The old design
/// got this rescue by accident, from a detached watchdog that outlived
/// teardown; this makes it explicit. A thread whose IRP ignores every cancel
/// is abandoned with an error at the deadline rather than hanging shutdown;
/// process exit reclaims it.
fn stop_tails(mut tails: Vec<TailHandle>) {
    // Every flag first: a serial signal-and-join would leave the tails
    // behind a wedged one never even asked to stop.
    for t in &tails {
        t.signal_stop();
    }
    let deadline = Instant::now() + TEARDOWN_WAIT;
    let mut grace = true;
    loop {
        let mut i = 0;
        while i < tails.len() {
            if tails[i].is_finished() {
                tails.swap_remove(i).shutdown(); // exited: join is immediate
            } else {
                i += 1;
            }
        }
        if tails.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            break;
        }
        if !grace {
            // Still running after a grace poll means blocked in a syscall.
            // Aborting it lands the tail in its error path, which rechecks
            // the stop flag promptly. `Ok(false)` (not in a cancellable
            // wait right now) and errors are retried next round.
            for t in &tails {
                let _ = gozler_winfs::cancel_synchronous_io(&t.io_handle);
            }
        }
        grace = false;
        std::thread::sleep(TEARDOWN_POLL);
    }
    for t in tails {
        tracing::error!(
            volume = %t.mount,
            thread_id = t.io_handle.thread_id,
            "tail thread survived its stop flag and every I/O cancel; abandoning it to process exit"
        );
        drop(t);
    }
}

impl Supervised {
    /// A volume that has no tail yet, with its first start under way. The
    /// first start is not a revival, so it costs no budget.
    fn start(state: Arc<VolumeState>) -> Supervised {
        let mut supervised = Supervised {
            state,
            tail: None,
            revive_attempts: 0,
            beating_since: None,
            pending: None,
            last_failure: None,
        };
        supervised.launch();
        supervised
    }

    /// Every tail this volume holds or has in flight, resolving a pending
    /// start by `deadline` (see [`drain_pending`]), so the caller stops them
    /// together.
    fn into_tails(self, deadline: Instant) -> Vec<TailHandle> {
        let mut tails = Vec::new();
        if let Some(pending) = self.pending
            && let Some(fresh) = drain_pending(pending, self.state.mount_prefix(), deadline)
        {
            tails.push(fresh);
        }
        tails.extend(self.tail);
        tails
    }

    /// One scan of this volume. Returns false when supervision should end.
    fn scan(&mut self, now: u64) -> bool {
        if self.pending.is_some() {
            self.poll_revival();
            return true;
        }
        // No tail yet reads as a retired one: started while budget lasts.
        let pulse = self
            .tail
            .as_ref()
            .map_or(Pulse::Retired, |t| t.pulse.load());
        if pulse == Pulse::Retired && *self.state.phase.read() == VolumePhase::Snapshot {
            // The tail built the index and found that the volume has no
            // journal and never will: the index is complete and there is
            // nothing to tail, so reviving it would only rebuild it again.
            return false;
        }
        // Track the unbroken run of healthy observations this incarnation
        // has shown. Sampled at scan granularity, which can only
        // under-count health, never fake it. The anchor is only ever
        // consulted for a refund, so a never-revived original skips the
        // bookkeeping (and the phase read) entirely; the common path costs
        // exactly the one atomic load the old watchdog cost.
        if self.revive_attempts > 0 {
            let live = matches!(*self.state.phase.read(), VolumePhase::Live);
            self.beating_since =
                accrue_health(self.beating_since, pulse, live, now, Instant::now());
        }
        let proven_healthy = self
            .beating_since
            .is_some_and(|at| at.elapsed() >= HEALTHY_AFTER);
        match assess(pulse, now, self.revive_attempts, proven_healthy) {
            Action::Nothing => true,
            Action::ResetBudget => {
                tracing::debug!(
                    volume = %self.state.mount_prefix(),
                    "revived tail has stayed healthy; revival budget refunded"
                );
                self.revive_attempts = 0;
                self.beating_since = None;
                true
            }
            Action::CancelIo { stalled_secs } => {
                if let Some(tail) = &self.tail {
                    cancel_stuck_io(tail, stalled_secs);
                }
                true
            }
            Action::Revive => {
                self.launch_revival();
                true
            }
            Action::GiveUp => {
                self.give_up();
                false
            }
        }
    }

    /// Starts replacing a retired tail, or retries a start that failed:
    /// charge the attempt, then [`launch`](Self::launch). The attempt is
    /// charged up front: any failure leaves the retired tail (or none) in
    /// place, a later scan lands back in [`Action::Revive`], and the budget
    /// converts repeated failure into [`Action::GiveUp`] with at least a scan
    /// interval of backoff between tries.
    fn launch_revival(&mut self) {
        self.revive_attempts += 1;
        self.beating_since = None;
        // Every deliberate exit sets Failed or Offline itself, so a phase
        // still claiming to track here is the panic signature. Correct it
        // before the revival window opens: a revival blocked on sick
        // hardware must not extend a false Live indefinitely.
        {
            let mut phase = self.state.phase.write();
            if matches!(*phase, VolumePhase::Live | VolumePhase::Rescanning) {
                *phase = VolumePhase::Offline;
            }
        }
        tracing::warn!(
            volume = %self.state.mount_prefix(),
            attempt = self.revive_attempts,
            budget = MAX_REVIVES,
            "{}",
            if self.tail.is_some() {
                "tail thread exited; reviving it"
            } else {
                "the volume's tail did not start; retrying"
            }
        );
        self.launch();
    }

    /// Hands the blocking work of starting a tail (volume open, spawn) to a
    /// worker thread, which [`poll_revival`](Self::poll_revival) polls on
    /// later scans. The tail's first act is the full build.
    fn launch(&mut self) {
        let state = self.state.clone();
        let (io_tx, io_rx) = mpsc::sync_channel(1);
        // Capacity 0 (rendezvous) on purpose: a buffered channel would let
        // the worker's send "succeed" with nobody listening, right as
        // teardown's drain gives up, and the buffered TailHandle would then
        // be dropped un-stopped when the receiver drops. Rendezvous makes
        // that unrepresentable; the worker just parks in send until the next
        // poll or fails the send and cleans up itself.
        let (result_tx, result_rx) = mpsc::sync_channel(0);
        let spawned = std::thread::Builder::new()
            .name(format!("gozler-start-{}", self.state.mount_prefix()))
            .spawn(move || {
                // Duplicated ON the worker (the pseudo-handle is per-thread)
                // so the supervisor can cancel an open a sick device pends.
                let _ = io_tx.send(gozler_winfs::current_thread_io_handle());
                let outcome = bootstrap::open_volume(&state.guid)
                    .map_err(|e| std::io::Error::other(format!("{e:#}")))
                    .and_then(|handle| tail::spawn(state, handle, None));
                if let Err(returned) = result_tx.send(outcome) {
                    // The supervisor stopped listening (teardown abandoned
                    // this worker). A tail built anyway must not run
                    // unowned; its thread already checks a fresh stop flag.
                    if let Ok(tail) = returned.0 {
                        tail.shutdown();
                    }
                }
            });
        match spawned {
            Ok(worker) => {
                // The worker sends its handle before any blocking work, so
                // this recv is prompt.
                let io = io_rx.recv().ok().and_then(Result::ok);
                self.pending = Some(PendingRevival {
                    rx: result_rx,
                    io,
                    worker,
                    reported_blocked: false,
                });
            }
            Err(e) => {
                tracing::error!(
                    volume = %self.state.mount_prefix(),
                    error = %e,
                    "could not start a worker to start the tail; retrying next scan while budget lasts"
                );
            }
        }
    }

    /// One poll of an in-flight start. The first poll happens a full scan
    /// interval after launch, which is the grace a legitimate spin-up gets;
    /// a worker still silent after that is presumed blocked in the open and
    /// has its I/O cancelled each scan until it reports.
    fn poll_revival(&mut self) {
        let Some(pending) = &mut self.pending else {
            return;
        };
        match pending.rx.try_recv() {
            Ok(outcome) => {
                let pending = self.pending.take().expect("pending checked above");
                // Reported means the worker is past its send; this join is
                // immediate.
                let _ = pending.worker.join();
                match outcome {
                    Ok(fresh) => {
                        tracing::info!(volume = %fresh.mount, "tail started");
                        // The old thread already retired, so this join is
                        // immediate; shutting it down (not just dropping
                        // it) reaps the thread.
                        if let Some(old) = self.tail.replace(fresh) {
                            old.shutdown();
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            volume = %self.state.mount_prefix(),
                            error = %e,
                            "starting the tail failed; retrying next scan while budget lasts"
                        );
                        self.last_failure = Some(e.to_string());
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                if !pending.reported_blocked {
                    tracing::warn!(
                        volume = %self.state.mount_prefix(),
                        "starting the tail is blocked opening the volume; cancelling its I/O"
                    );
                    pending.reported_blocked = true;
                }
                if let Some(io) = &pending.io {
                    let _ = gozler_winfs::cancel_synchronous_io(io);
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                let pending = self.pending.take().expect("pending checked above");
                let _ = pending.worker.join();
                tracing::error!(
                    volume = %self.state.mount_prefix(),
                    "the worker starting the tail died without reporting; retrying next scan while budget lasts"
                );
            }
        }
    }

    /// The budget is spent: say so once, loudly, and stop supervising. A
    /// tail's own exit already set an honest phase with its own reason
    /// (`Failed` or `Offline`, both logged), and `launch_revival` corrects
    /// the panic signature before each attempt. A volume still
    /// `Bootstrapping` was never indexed, because its tail never started or
    /// never finished the first build; it becomes `Failed` here, or it would
    /// claim to be indexing forever. The `Live` correction is
    /// belt-and-braces for any future path that retires a Live tail without
    /// a revival ever launching.
    fn give_up(&self) {
        tracing::error!(
            volume = %self.state.mount_prefix(),
            attempts = self.revive_attempts,
            "the volume's tail keeps failing; giving up until the daemon restarts"
        );
        let mut phase = self.state.phase.write();
        let reason = match *phase {
            VolumePhase::Bootstrapping => format!(
                "never indexed: {}",
                self.last_failure
                    .as_deref()
                    .unwrap_or("its tail kept stopping")
            ),
            VolumePhase::Live | VolumePhase::Rescanning => {
                "tail thread kept dying; live updates stopped".into()
            }
            _ => return,
        };
        *phase = VolumePhase::Failed { reason };
    }
}

/// Cancels the blocked synchronous I/O of a presumed-wedged tail. The
/// aborted call returns `ERROR_OPERATION_ABORTED` to the tail loop, whose
/// ordinary retry path (backoff, Offline past the degrade window, recovery
/// on success) takes over. Repeats every scan while the stall persists, so
/// an IRP that ignores one cancel gets another.
fn cancel_stuck_io(tail: &TailHandle, stalled_secs: u64) {
    tracing::error!(
        volume = %tail.mount,
        stalled_secs,
        thread_id = tail.io_handle.thread_id,
        "tail thread heartbeat stalled; cancelling its blocked synchronous I/O"
    );
    match gozler_winfs::cancel_synchronous_io(&tail.io_handle) {
        Ok(true) => {
            tracing::warn!(volume = %tail.mount, "stuck journal I/O cancelled; tail will retry")
        }
        Ok(false) => {
            tracing::warn!(volume = %tail.mount, "tail stalled but not in a cancellable wait")
        }
        Err(e) => {
            tracing::error!(volume = %tail.mount, error = %e, "cancelling stuck I/O failed")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_752_800_000;
    fn beat_ago(secs: u64) -> Pulse {
        Pulse::Beat(NOW - secs)
    }
    fn busy_for(secs: u64) -> Pulse {
        Pulse::Busy(NOW - secs)
    }

    /// The decision table, exhaustively. This is the fix for the bug where
    /// the watchdog could not tell a wedged tail from an exited one: each
    /// pulse state maps to exactly one family of actions, and Retired never
    /// reaches CancelIo.
    #[test]
    fn assess_decides_by_pulse_state() {
        // A fresh beat is left alone.
        assert_eq!(assess(beat_ago(0), NOW, 0, false), Action::Nothing);
        // A beat inside the stall window is left alone even at the edge.
        let edge = BEAT_STALL.as_secs() - 1;
        assert_eq!(assess(beat_ago(edge), NOW, 0, false), Action::Nothing);
        // Past the window: presumed wedged, cancel.
        let past = BEAT_STALL.as_secs();
        assert_eq!(
            assess(beat_ago(past), NOW, 0, false),
            Action::CancelIo { stalled_secs: past }
        );
        // Busy is exempt where a beat would already have been shot...
        assert_eq!(assess(busy_for(past), NOW, 0, false), Action::Nothing);
        // ...but not forever: a wedged rebuild is still a wedge.
        let long = BUSY_STALL.as_secs();
        assert_eq!(
            assess(busy_for(long), NOW, 0, false),
            Action::CancelIo { stalled_secs: long }
        );
        // Retired is never a cancel target, however long ago it happened:
        // it is revived while budget lasts and given up on after.
        assert_eq!(assess(Pulse::Retired, NOW, 0, false), Action::Revive);
        assert_eq!(
            assess(Pulse::Retired, NOW, MAX_REVIVES - 1, false),
            Action::Revive
        );
        assert_eq!(
            assess(Pulse::Retired, NOW, MAX_REVIVES, false),
            Action::GiveUp
        );
    }

    /// The budget refund: only a beating revived tail that has proven itself
    /// gets its history erased. An original (no revivals) has nothing to
    /// refund, an unproven revival keeps its count, and a busy or stalled
    /// revival proves nothing.
    #[test]
    fn budget_refund_requires_a_proven_healthy_beat() {
        assert_eq!(assess(beat_ago(0), NOW, 1, true), Action::ResetBudget);
        assert_eq!(assess(beat_ago(0), NOW, 0, true), Action::Nothing);
        assert_eq!(assess(beat_ago(0), NOW, 1, false), Action::Nothing);
        assert_eq!(assess(busy_for(0), NOW, 1, true), Action::Nothing);
        // A stalled revived tail is a wedge first; the refund never masks it.
        let past = BEAT_STALL.as_secs();
        assert_eq!(
            assess(beat_ago(past), NOW, 1, true),
            Action::CancelIo { stalled_secs: past }
        );
    }

    /// Health accrual: only a FRESH `Beat` observed while the volume is
    /// `Live` starts or extends the run. An Offline beat is a turning retry
    /// loop that tracks nothing, and a stale beat is a loop that stopped
    /// turning (a wedge the stall threshold has not yet declared); either
    /// refunding would make give-up unreachable and revive a dead volume
    /// forever, one full MFT rebuild per cycle.
    #[test]
    fn health_accrues_only_on_fresh_live_beats() {
        let t0 = Instant::now();
        // A fresh live beat starts the run and a later one preserves the
        // anchor.
        let started = accrue_health(None, beat_ago(0), true, NOW, t0);
        assert_eq!(started, Some(t0));
        assert_eq!(
            accrue_health(started, beat_ago(0), true, NOW, Instant::now()),
            Some(t0)
        );
        // The same beat with the volume not Live breaks the run.
        assert_eq!(accrue_health(started, beat_ago(0), false, NOW, t0), None);
        // A stale beat breaks it even while Live: six healthy minutes then
        // a wedge under the stall threshold must not mature into a refund,
        // so the frozen stamp cannot carry the run.
        let stale = BEAT_FRESH.as_secs();
        assert_eq!(accrue_health(started, beat_ago(stale), true, NOW, t0), None);
        // Busy and Retired break it regardless of phase.
        assert_eq!(accrue_health(started, busy_for(0), true, NOW, t0), None);
        assert_eq!(accrue_health(started, Pulse::Retired, true, NOW, t0), None);
    }

    /// Only a volume missing from the list leaves the index. An indexed volume
    /// whose filesystem query fails once classifies as not indexable; building
    /// the set from the indexable volumes alone would drop it and rebuild its
    /// whole index on the next scan. A volume gozler skips is not added.
    #[test]
    fn only_a_volume_that_is_no_longer_listed_leaves_the_index() {
        let indexed = r"\\?\Volume{11111111-0000-0000-0000-000000000000}\";
        let usb_stick = r"\\?\Volume{22222222-0000-0000-0000-000000000000}\";
        let volumes = Arc::new(VolumeRegistry::default());
        volumes.add(Arc::new(VolumeState::new(
            indexed.into(),
            vec![r"E:\".into()],
        )));
        let mut fleet = Fleet {
            volumes: volumes.clone(),
            supervised: Vec::new(),
            skipped: HashSet::new(),
            discovery: None,
            starting: None,
        };
        let listed = |guid: &str, is_fixed, filesystem: Option<&str>| VolumeInfo {
            guid_path: guid.into(),
            mounts: Vec::new(),
            is_fixed,
            filesystem: filesystem.map(Into::into),
        };
        let guids =
            || -> Vec<String> { volumes.current().iter().map(|v| v.guid.clone()).collect() };

        fleet.reconcile(vec![
            listed(indexed, true, None),
            listed(usb_stick, false, Some("FAT32")),
        ]);
        assert_eq!(guids(), [indexed]);

        fleet.reconcile(Vec::new());
        assert!(guids().is_empty(), "a volume that is gone leaves the index");
    }

    /// A stamp newer than the scan's clock reading (a tail that beat after
    /// the scan read the clock) must read as "not stalled", never as a huge
    /// stall that shoots a healthy tail.
    #[test]
    fn a_stamp_newer_than_now_never_cancels() {
        assert_eq!(
            assess(Pulse::Beat(NOW + 600), NOW, 0, false),
            Action::Nothing
        );
        assert_eq!(
            assess(Pulse::Busy(NOW + 600), NOW, 0, false),
            Action::Nothing
        );
    }

    /// The stall threshold must comfortably exceed the tail's longest
    /// legitimate silence: a retry sleeping out its full backoff, observed a
    /// scan interval late. Shrinking either constant past the other would
    /// shoot healthy tails mid-backoff.
    #[test]
    fn beat_stall_tolerates_a_full_backoff_sleep() {
        assert!(
            BEAT_STALL >= tail::RETRY_BACKOFF_MAX + SCAN_INTERVAL + SCAN_INTERVAL,
            "a tail sleeping out RETRY_BACKOFF_MAX must never look stalled"
        );
        // And busy tolerance dwarfs beat tolerance: a rebuild is minutes of
        // legitimate silence, not seconds.
        assert!(BUSY_STALL > BEAT_STALL);
        // Freshness must cover one scan interval (an observed beat is up to
        // one scan old on a healthy tail) yet declare staleness well before
        // the stall threshold, or the refund window reopens.
        assert!(BEAT_FRESH >= SCAN_INTERVAL);
        assert!(BEAT_FRESH < BEAT_STALL);
        // Shutdown must outwait the supervisor's own legitimate worst case,
        // draining pending revivals then stopping its tails, with slack;
        // anything tighter would abandon a supervisor that was merely busy.
        assert!(SUPERVISOR_JOIN_WAIT >= TEARDOWN_WAIT + PENDING_DRAIN_WAIT + TEARDOWN_WAIT);
        // A zero budget would turn every retirement into instant give-up and
        // make revival dead code.
        const { assert!(MAX_REVIVES > 0) };
    }
}
