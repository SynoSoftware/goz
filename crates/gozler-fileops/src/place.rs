//! Giving an entry its name in a destination folder: the copy's final
//! rename (Copy, steps 6 and 7) and the move's rename. A name that another
//! program took after the folder was listed is someone else's entry, never
//! replaced without the person's choice and never deleted (D13).

use std::sync::Mutex;

use gozler_winfs::{Existing, path_attributes, rename, set_path_attributes};

use crate::conflicts::Choice;
use crate::listing::join;
use crate::names::Names;
use crate::win32::{
    ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, FILE_ATTRIBUTE_DIRECTORY,
    FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_READONLY,
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
}

/// How an entry ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Placed {
    /// It has its name, possibly numbered after the listing.
    Named,
    /// Another program took the name, and the choice, or the lack of one,
    /// skips it. The entry stays where it was.
    Skipped,
    /// With Windows' error. The entry stays where it was.
    Failed(u32),
}

/// What to place.
pub(crate) struct Placing<'a> {
    /// The entry's path now: a copy's temporary file, or a moved source.
    pub(crate) from: &'a [u16],
    /// The name it would have in the source folder, from which Keep both
    /// numbers.
    pub(crate) base: &'a [u16],
    /// The name the conflict rules gave it.
    pub(crate) name: Vec<u16>,
    /// Rename over the file found at `name`.
    pub(crate) replace: bool,
    pub(crate) is_folder: bool,
}

/// Renames `placing.from` to its name in `target`, as `MoveFileExW`
/// without `MOVEFILE_REPLACE_EXISTING` except for Replace, so a file that
/// took the name after the listing makes the rename fail instead of being
/// replaced; the run's choice then applies to it.
pub(crate) fn place(target: &Target, placing: Placing<'_>, choice: Option<Choice>) -> Placed {
    let Placing {
        from,
        base,
        mut name,
        mut replace,
        is_folder,
    } = placing;
    loop {
        let to = join(&target.verbatim, &name);
        if replace && let Err(error) = clear_read_only(&to) {
            return Placed::Failed(error);
        }
        let existing = if replace {
            Existing::Replace
        } else {
            Existing::Refuse
        };
        let Err(error) = rename(from, &to, existing) else {
            return Placed::Named;
        };
        // Access denied comes both for a hidden or read-only file at the
        // name and for a folder whose permissions refuse the rename, so
        // only the name's existence tells them apart.
        let attributes = match error.code {
            ERROR_FILE_EXISTS | ERROR_ALREADY_EXISTS | ERROR_ACCESS_DENIED => {
                match path_attributes(&to) {
                    Ok(attributes) => attributes,
                    Err(_) => return Placed::Failed(error.code),
                }
            }
            _ => None,
        };
        let Some(attributes) = attributes else {
            return Placed::Failed(error.code);
        };
        // A file and a folder with one name are refused whatever the choice.
        if (attributes & FILE_ATTRIBUTE_DIRECTORY != 0) != is_folder {
            return Placed::Failed(ERROR_ALREADY_EXISTS);
        }
        if replace {
            return Placed::Failed(error.code);
        }
        match choice {
            Some(Choice::Replace) if !is_folder => replace = true,
            Some(Choice::KeepBoth) | Some(Choice::Replace) => {
                let mut names = target.names.lock().expect("no worker panics");
                name = names.keep_both(base, is_folder);
            }
            Some(Choice::Skip) | None => return Placed::Skipped,
        }
    }
}

/// Replace clears the read-only attribute of the file it replaces, which
/// the rename would refuse.
fn clear_read_only(path: &[u16]) -> Result<(), u32> {
    match path_attributes(path) {
        Ok(Some(attributes)) if attributes & FILE_ATTRIBUTE_READONLY != 0 => {
            let cleared = match attributes & !FILE_ATTRIBUTE_READONLY {
                0 => FILE_ATTRIBUTE_NORMAL,
                other => other,
            };
            set_path_attributes(path, cleared).map_err(|error| error.code)
        }
        Ok(_) => Ok(()),
        Err(error) => Err(error.code),
    }
}
