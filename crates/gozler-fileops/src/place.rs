//! Giving an entry its name in a destination folder: the copy that writes
//! its file at the final name (Copy, steps 5 to 7) and the move's rename. A
//! name that another program took after the folder was listed is someone
//! else's entry, never replaced without the person's choice and never
//! deleted (D13).

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use gozler_winfs::{Access, Existing, Reparse, open_entry, path_attributes, set_path_attributes};

use crate::conflicts::Policy;
use crate::delete::{Disposition, remove};
use crate::listing::join;
use crate::names::Names;
use crate::names::fold;
use crate::operation::{Initialization, Pause};
use crate::win32::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_SYSTEM,
};

/// A destination folder.
#[derive(Debug)]
pub(crate) struct Target {
    /// With the `\\?\` or `\\?\UNC\` prefix.
    pub(crate) verbatim: Vec<u16>,
    /// Its names when it was listed, and the names this operation gave.
    pub(crate) names: Mutex<Names>,
    /// This operation created it.
    pub(crate) made: bool,
    children: Mutex<HashMap<Vec<u16>, Arc<Child>>>,
}

type Child = Initialization<Result<Arc<Target>, u32>>;

impl Target {
    pub(crate) fn new(verbatim: Vec<u16>, names: Names, made: bool) -> Self {
        Self {
            verbatim,
            names: Mutex::new(names),
            made,
            children: Mutex::default(),
        }
    }

    /// One name authority for every source merged into this child folder.
    pub(crate) fn child(
        &self,
        name: &[u16],
        shared: bool,
        pause: &Pause,
        cancel: &AtomicBool,
        initialize: impl FnOnce() -> Result<Target, u32>,
    ) -> Result<Arc<Target>, u32> {
        if !shared {
            return initialize().map(Arc::new);
        }
        let child = self
            .children
            .lock()
            .expect("no worker panics")
            .entry(fold(name))
            .or_default()
            .clone();
        child.get_or_init(pause, cancel, || initialize().map(Arc::new))
    }
}

/// How an entry ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placed<T> {
    /// It has its name, possibly numbered after the listing, with what the
    /// write returned.
    Named(T),
    /// The accepted policy keeps the destination. Nothing was written.
    Skipped,
    /// With Windows' error.
    Failed(u32),
}

/// What to place.
pub(crate) struct Placing<'a> {
    /// The name it would have in the source folder, from which Keep both
    /// numbers.
    pub(crate) base: &'a [u16],
    /// The name the conflict rules gave it.
    pub(crate) name: Vec<u16>,
    /// Write over the file found at `name`.
    pub(crate) replace: bool,
    pub(crate) is_folder: bool,
    pub(crate) size: u64,
    pub(crate) last_write: i64,
    /// Recovery may finish its own partial file regardless of the policy.
    pub(crate) recovering: bool,
    /// [`Wanted::shared`](crate::conflicts::Wanted::shared): a file that
    /// takes the name after the listing may be the other entry's copy, so
    /// Replace gives this entry a number instead, as two selected entries
    /// with one name get.
    pub(crate) shared: bool,
}

/// Writes an entry at its name in `target` with `write`, which receives the
/// path and whether it may write over a file there. Without Replace it is a
/// copy, a create or a rename that refuses an existing name; with Replace,
/// a rename over the file, or a copy that writes over it in place or after
/// [`remove_replaced`]. A file that took the name after the listing then
/// makes the write fail instead of being replaced, and the run's choice
/// applies to it, except that a shared entry never replaces it.
pub(crate) fn place<T>(
    target: &Target,
    placing: Placing<'_>,
    choice: Option<Policy>,
    mut write: impl FnMut(&[u16], Existing) -> Result<T, u32>,
) -> Placed<T> {
    let Placing {
        base,
        mut name,
        mut replace,
        is_folder,
        size,
        last_write,
        recovering,
        shared,
    } = placing;
    let policy = choice.unwrap_or_default();
    loop {
        let to = join(&target.verbatim, &name);
        if replace && !recovering && matches!(policy, Policy::UpdateChanged | Policy::UpdateNewer) {
            let inspected = open_entry(&to, Access::Inspect, Reparse::Open)
                .and_then(|entry| Ok((entry.attribute_tag()?, entry.stat()?)));
            match inspected {
                Ok((tag, _)) if (tag.attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != is_folder => {
                    return Placed::Failed(ERROR_ALREADY_EXISTS);
                }
                Ok((_, found))
                    if !policy.replaces(size, last_write, found.size, found.last_write) =>
                {
                    return Placed::Skipped;
                }
                Ok(_) => {}
                Err(error) if crate::win32::is_gone(error.code) => {}
                Err(error) => return Placed::Failed(error.code),
            }
        }
        if replace && let Err(error) = clear_protection(&to) {
            return Placed::Failed(error);
        }
        let existing = if replace {
            Existing::Replace
        } else {
            Existing::Refuse
        };
        let error = match write(&to, existing) {
            Ok(written) => return Placed::Named(written),
            Err(error) => error,
        };
        // Access denied comes both for a hidden or read-only file at the
        // name and for a folder whose permissions refuse the write, so only
        // the name's existence tells them apart.
        let attributes = match error {
            ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS | ERROR_ACCESS_DENIED => {
                match path_attributes(&to) {
                    Ok(attributes) => attributes,
                    Err(_) => return Placed::Failed(error),
                }
            }
            _ => None,
        };
        let Some(attributes) = attributes else {
            return Placed::Failed(error);
        };
        // A file and a folder with one name are refused whatever the choice.
        if (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != is_folder {
            return Placed::Failed(ERROR_ALREADY_EXISTS);
        }
        if replace {
            return Placed::Failed(error);
        }
        match policy {
            Policy::Replace | Policy::UpdateChanged | Policy::UpdateNewer
                if !is_folder && !shared =>
            {
                replace = true
            }
            Policy::KeepBoth | Policy::Replace | Policy::UpdateChanged | Policy::UpdateNewer => {
                let mut names = target.names.lock().expect("no worker panics");
                name = names.keep_both(base, is_folder);
            }
            Policy::Skip => return Placed::Skipped,
        }
    }
}

/// A copy's Replace: deletes the entry at `path` itself, a link included,
/// never what a link points to (D13 lets Replace delete the file it
/// replaces). The copy then creates its file with a create that refuses an
/// existing name, so an entry put at the name meanwhile fails the copy
/// instead of being written through.
pub(crate) fn remove_replaced(
    cancel: &AtomicBool,
    disposition: &Disposition,
    path: &[u16],
) -> Result<(), u32> {
    remove(cancel, disposition, || {
        open_entry(path, Access::Delete, Reparse::Open)
    })
}

/// Replace clears the read-only, hidden and system attributes of the file
/// it writes over: the rename refuses a read-only file.
pub(crate) fn clear_protection(path: &[u16]) -> Result<(), u32> {
    const PROTECTION: u32 = FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM;
    match path_attributes(path) {
        Ok(Some(attributes)) if attributes & PROTECTION != 0 => {
            let cleared = match attributes & !PROTECTION {
                0 => FILE_ATTRIBUTE_NORMAL,
                other => other,
            };
            set_path_attributes(path, cleared).map_err(|error| error.code)
        }
        Ok(_) => Ok(()),
        Err(error) => Err(error.code),
    }
}
