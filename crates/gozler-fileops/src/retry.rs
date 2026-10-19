//! Short retries of a delete or an open that another program refuses for a
//! moment, as rimraf retries them: scanners and indexers hold files open
//! briefly.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::win32::{ERROR_DIR_NOT_EMPTY, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION};

/// Whether a refusal may pass within the retries. Access denied may not: the
/// delete already deletes read-only files, so what remains is a permission,
/// and retrying it would cost 125 s on 10,000 files the person may not
/// delete. rimraf and Rust's standard library do not retry it either.
fn is_brief(code: u32) -> bool {
    matches!(
        code,
        ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION | ERROR_DIR_NOT_EMPTY
    )
}

/// The waits between tries: the first 1 ms, each 1.2 times longer than the
/// one before, until they add up to 200 ms.
fn waits() -> impl Iterator<Item = Duration> {
    let mut wait = 1.0_f64;
    let mut total = 0.0;
    std::iter::from_fn(move || {
        (total < 200.0).then(|| {
            let this = wait;
            total += wait;
            wait *= 1.2;
            Duration::from_secs_f64(this / 1000.0)
        })
    })
}

/// Runs `attempt` until it succeeds, fails with a refusal that does not
/// pass, the waits run out, or `cancel` is set. Returns its last result.
pub(crate) fn retry<T>(
    cancel: &AtomicBool,
    mut attempt: impl FnMut() -> Result<T, u32>,
) -> Result<T, u32> {
    let mut waits = waits();
    loop {
        match attempt() {
            Err(code) if is_brief(code) && !cancel.load(Ordering::Relaxed) => match waits.next() {
                Some(wait) => std::thread::sleep(wait),
                None => return Err(code),
            },
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ERROR_ACCESS_DENIED: u32 = 5;

    #[test]
    fn only_brief_refusals_are_retried() {
        let mut tries = 0;
        let _ = retry::<()>(&AtomicBool::new(false), || {
            tries += 1;
            Err(ERROR_ACCESS_DENIED)
        });
        assert_eq!(tries, 1);
        assert!(is_brief(ERROR_SHARING_VIOLATION));
        assert!(is_brief(ERROR_LOCK_VIOLATION));
        assert!(is_brief(ERROR_DIR_NOT_EMPTY));
        let total: Duration = waits().sum();
        assert!((200..250).contains(&total.as_millis()), "{total:?}");
    }
}
