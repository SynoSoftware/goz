//! The system time, and clocks that ignore what a wall clock counts: sleep,
//! and other threads.

use std::os::windows::io::{AsRawHandle, BorrowedHandle};
use std::time::Duration;

use windows_sys::Win32::Foundation::FILETIME;
use windows_sys::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread};
use windows_sys::Win32::System::WindowsProgramming::{
    QueryProcessCycleTime, QueryThreadCycleTime, QueryUnbiasedInterruptTime,
};

use crate::error::{WinError, last_error};

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

/// The system time as a `FILETIME`, in 100 ns units, from the clock tick
/// (`GetSystemTimeAsFileTime`). It never runs ahead of the precise clock,
/// so a file created or written after this call never has an earlier
/// creation or last write time, whichever of the two clocks its file system
/// stamps it with.
pub fn system_time() -> i64 {
    let mut time = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    // SAFETY: plain FFI writing one FILETIME through a valid pointer; it
    // cannot fail.
    unsafe { GetSystemTimeAsFileTime(&mut time) };
    (i64::from(time.dwHighDateTime) << 32) | i64::from(time.dwLowDateTime)
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

/// CPU cycles that every thread of `process` has run, in user and kernel
/// mode (`QueryProcessCycleTime`). The handle needs
/// `PROCESS_QUERY_LIMITED_INFORMATION`, as a spawned child's has.
pub fn process_cycles(process: BorrowedHandle<'_>) -> Result<u64, WinError> {
    let mut cycles: u64 = 0;
    // SAFETY: `process` is a live process handle for the call, and the call
    // writes one u64 through a valid pointer.
    if unsafe { QueryProcessCycleTime(process.as_raw_handle(), &mut cycles) } == 0 {
        return Err(last_error("QueryProcessCycleTime"));
    }
    Ok(cycles)
}

/// CPU cycles that every thread of this process has run.
pub fn own_process_cycles() -> u64 {
    // SAFETY: the pseudo-handle from GetCurrentProcess is always valid and
    // is never closed.
    let process = unsafe { BorrowedHandle::borrow_raw(GetCurrentProcess()) };
    process_cycles(process).expect("this process can always be queried")
}
