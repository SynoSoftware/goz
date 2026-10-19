//! A tail thread's self-reported liveness, packed into one atomic word.
//!
//! A bare heartbeat timestamp cannot tell a tail wedged in a syscall from one
//! that returned for good (journal permanently lost, rescan budget exhausted).
//! The supervisor's watchdog must act on the first and leave the second alone,
//! so the tail reports which it is. The "returned for good" report is
//! published by a [`Drop`] guard, so every exit path, including a panic unwind
//! and any `return` added later, retires the pulse.
//!
//! State and timestamp share one `u64` (tag in the top two bits, seconds of
//! [`awake_secs`] below) so a reader always sees a consistent pair; two
//! separate atomics could tear between a supervisor's loads.

use std::sync::atomic::{AtomicU64, Ordering};

const TAG_SHIFT: u32 = 62;
const SECS_MASK: u64 = (1 << TAG_SHIFT) - 1;
const TAG_BEAT: u64 = 0;
const TAG_BUSY: u64 = 1;
const TAG_RETIRED: u64 = 2;

/// One decoded pulse observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Pulse {
    /// The tail loop is turning; [`awake_secs`] of its last stamp.
    Beat(u64),
    /// The tail is inside a legitimately long, non-stamping operation (a full
    /// MFT rebuild); [`awake_secs`] of when it entered.
    Busy(u64),
    /// The tail thread returned. Not stuck, and not coming back on its own.
    Retired,
}

/// The shared word a tail writes and its supervisor reads.
///
/// Ordering invariant: `Retired` must be the last value ever stored, and that
/// is guaranteed structurally rather than atomically. Busy scopes live inside
/// `tail_loop` while the retire guard wraps the whole call, so guard drop
/// order publishes any pending `Beat` restore before `Retired`. This keeps the
/// hot-path stamp a plain relaxed store instead of a compare-exchange.
pub(crate) struct TailPulse(AtomicU64);

impl TailPulse {
    /// A fresh pulse, beating as of now (the tail is about to start).
    pub(crate) fn new() -> Self {
        Self(AtomicU64::new(pack(TAG_BEAT, awake_secs())))
    }

    /// Stamps liveness. Called once per tail-loop turn.
    pub(crate) fn beat(&self) {
        self.0
            .store(pack(TAG_BEAT, awake_secs()), Ordering::Relaxed);
    }

    /// The current observation.
    pub(crate) fn load(&self) -> Pulse {
        let word = self.0.load(Ordering::Relaxed);
        let secs = word & SECS_MASK;
        match word >> TAG_SHIFT {
            TAG_BUSY => Pulse::Busy(secs),
            TAG_RETIRED => Pulse::Retired,
            _ => Pulse::Beat(secs),
        }
    }

    /// Marks the tail busy until the returned guard drops, which restores a
    /// fresh beat so a long rebuild does not read as an instant stall the
    /// moment it finishes. Scopes must not nest (the inner drop would end the
    /// outer scope early); the tail has exactly one, around `rescan`.
    pub(crate) fn busy_scope(&self) -> BusyGuard<'_> {
        self.0
            .store(pack(TAG_BUSY, awake_secs()), Ordering::Relaxed);
        BusyGuard(self)
    }

    /// Publishes `Retired` when the returned guard drops. Hold it across the
    /// whole `tail_loop` call.
    pub(crate) fn retire_on_drop(&self) -> RetireGuard<'_> {
        RetireGuard(self)
    }
}

/// Restores a fresh [`Pulse::Beat`] on drop. See [`TailPulse::busy_scope`].
pub(crate) struct BusyGuard<'a>(&'a TailPulse);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.beat();
    }
}

/// Publishes [`Pulse::Retired`] on drop. See [`TailPulse::retire_on_drop`].
pub(crate) struct RetireGuard<'a>(&'a TailPulse);

impl Drop for RetireGuard<'_> {
    fn drop(&mut self) {
        self.0.0.store(pack(TAG_RETIRED, 0), Ordering::Relaxed);
    }
}

fn pack(tag: u64, secs: u64) -> u64 {
    (tag << TAG_SHIFT) | (secs & SECS_MASK)
}

/// The clock every stamp and every stall verdict uses: seconds the machine
/// has been awake since boot. On a wall clock, a laptop that sleeps through a
/// rescan, or a clock change, looks like a stalled rescan, and the supervisor
/// cancels a healthy rebuild.
pub(crate) fn awake_secs() -> u64 {
    gozler_winfs::awake_time().as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_pulse_beats_now() {
        let p = TailPulse::new();
        let before = awake_secs();
        match p.load() {
            Pulse::Beat(at) => assert!(at <= before && before - at <= 1),
            other => panic!("fresh pulse must beat, got {other:?}"),
        }
    }

    #[test]
    fn busy_scope_restores_a_fresh_beat() {
        let p = TailPulse::new();
        {
            let _busy = p.busy_scope();
            assert!(matches!(p.load(), Pulse::Busy(_)));
        }
        assert!(matches!(p.load(), Pulse::Beat(_)));
    }

    /// A panicking tail still reports itself gone.
    #[test]
    fn a_panicking_thread_still_retires() {
        let p = std::sync::Arc::new(TailPulse::new());
        let thread_p = p.clone();
        let joined = std::thread::spawn(move || {
            let _retired = thread_p.retire_on_drop();
            panic!("tail died unexpectedly");
        })
        .join();
        assert!(joined.is_err(), "the thread must have panicked");
        assert_eq!(p.load(), Pulse::Retired);
    }

    /// A busy scope inside the retire guard ends first, so `Retired` is the
    /// final word.
    #[test]
    fn retire_wins_over_an_unwinding_busy_scope() {
        let p = TailPulse::new();
        {
            let _retired = p.retire_on_drop();
            let _busy = p.busy_scope();
        }
        assert_eq!(p.load(), Pulse::Retired);
    }

    /// Timestamps survive the tag bits for any plausible clock value,
    /// including far-future ones.
    #[test]
    fn tag_bits_never_corrupt_the_seconds() {
        for secs in [0u64, 1, 1_752_800_000, SECS_MASK] {
            assert_eq!(pack(TAG_BEAT, secs) & SECS_MASK, secs);
            assert_eq!(pack(TAG_BUSY, secs) >> TAG_SHIFT, TAG_BUSY);
        }
    }
}
