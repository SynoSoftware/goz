//! A clock that stops while the machine sleeps.

use std::time::Duration;

use windows_sys::Win32::System::WindowsProgramming::QueryUnbiasedInterruptTime;

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
