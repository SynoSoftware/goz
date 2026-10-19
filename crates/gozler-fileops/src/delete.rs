//! Delete: every delete is permanent (owner ruling: no Recycle Bin).
//!
//! The walk is in [`walk`]. This module holds the choice of D1's method,
//! which builds and is tested on every OS, and the delete of one entry,
//! which the copy uses too.

#[cfg(windows)]
pub(crate) mod walk;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::win32::{ERROR_INVALID_FUNCTION, ERROR_INVALID_PARAMETER, ERROR_NOT_SUPPORTED};

/// How one volume deletes (D1): with POSIX semantics, which also deletes a
/// read-only file in one call, until the volume refuses them once; then
/// with the classic delete for the rest of the operation. Support depends
/// on the Windows version and the driver, not only on the file system's
/// name, so the refusal itself is the test, and remembering it saves a
/// failed call per file on a FAT drive.
#[derive(Debug, Default)]
pub(crate) struct Disposition {
    classic: AtomicBool,
}

impl Disposition {
    /// Deletes one entry with `posix`, or with `classic` once the volume
    /// has refused `posix`. Each returns Windows' error code.
    pub(crate) fn delete(
        &self,
        posix: impl FnOnce() -> Result<(), u32>,
        classic: impl FnOnce() -> Result<(), u32>,
    ) -> Result<(), u32> {
        if !self.classic.load(Ordering::Relaxed) {
            match posix() {
                Err(ERROR_INVALID_PARAMETER | ERROR_NOT_SUPPORTED | ERROR_INVALID_FUNCTION) => {
                    self.classic.store(true, Ordering::Relaxed);
                }
                result => return result,
            }
        }
        classic()
    }

    /// Deletes the open `entry` as [`Disposition::delete`] does.
    #[cfg(windows)]
    pub(crate) fn delete_entry(&self, entry: &gozler_winfs::Entry) -> Result<(), u32> {
        self.delete(
            || entry.delete_posix().map_err(|error| error.code),
            || entry.delete_classic().map_err(|error| error.code),
        )
    }
}

/// Opens and deletes one entry with the short retries, which end early when
/// `cancel` is set. An entry already gone counts as deleted. The handle
/// closes before this returns, so a name deleted with POSIX semantics has
/// left its folder.
#[cfg(windows)]
pub(crate) fn remove(
    cancel: &AtomicBool,
    disposition: &Disposition,
    open: impl Fn() -> Result<gozler_winfs::Entry, gozler_winfs::WinError>,
) -> Result<(), u32> {
    let removed = crate::retry::retry(cancel, || {
        let entry = open().map_err(|error| error.code)?;
        disposition.delete_entry(&entry)
    });
    match removed {
        Err(error) if crate::win32::is_gone(error) => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A FAT drive refuses POSIX semantics; only its first file pays the
    /// refused call.
    #[test]
    fn a_volume_that_refuses_posix_delete_is_asked_once() {
        let disposition = Disposition::default();
        let (posix, classic) = (Cell::new(0), Cell::new(0));
        for _ in 0..100 {
            let deleted = disposition.delete(
                || {
                    posix.set(posix.get() + 1);
                    Err(ERROR_INVALID_PARAMETER)
                },
                || {
                    classic.set(classic.get() + 1);
                    Ok(())
                },
            );
            assert_eq!(deleted, Ok(()));
        }
        assert_eq!((posix.get(), classic.get()), (1, 100));
    }
}
