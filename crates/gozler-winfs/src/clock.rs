//! Clocks that ignore what a wall clock counts: sleep, and other threads.

use std::time::Duration;

use windows_sys::Win32::System::Threading::GetCurrentThread;
use windows_sys::Win32::System::WindowsProgramming::{
    QueryThreadCycleTime, QueryUnbiasedInterruptTime,
};

/// How long the system has been awake since it booted: monotonic, and
/// excluding sleep and hibernation (`QueryUnbiasedInterruptTime`). A wall
/// clock or a clock that counts suspended time makes a laptop's sleep look
/// like work that stopped.
pub fn awake_time() -> Duration {
    let mut ticks: u64 = 0;
    // SAFETY: plain FFI writing one u64 through a valid pointer. The call
    // cannot fail when given a valid pointer.
    unsafe { QueryUnbiasedInterruptTime(&mut ticks) };
    // The count is in 100 ns units.
    Duration::from_nanos(ticks.saturating_mul(100))
}

/// CPU cycles the calling thread has run (`QueryThreadCycleTime`). Unlike a
/// wall clock, other work on the machine does not change it, so a benchmark
/// that counts it gives comparable numbers on a machine in use.
pub fn thread_cycles() -> u64 {
    let mut cycles: u64 = 0;
    // SAFETY: the pseudo-handle from GetCurrentThread is always valid for the
    // calling thread, and the call writes one u64 through a valid pointer.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
    cycles
}
