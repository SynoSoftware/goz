//! What the walks share: listing a folder (D4), which entries they enter,
//! and paths built from a folder and a name.

use gozler_core::layout::{FolderEntry, walk_full_listing_buffer};
use gozler_winfs::Entry;

use crate::names::{Found, Holder, Kind, Names};
use crate::win32::{ERROR_INVALID_DATA, FILE_ATTRIBUTE_DIRECTORY, is_name_surrogate};

/// Files of one folder per work item, so one large folder uses several
/// workers (D2).
pub(crate) const FILES_PER_ITEM: usize = 256;

/// One listing page. 64 KiB holds several hundred entries.
pub(crate) const LISTING_WORDS: usize = 64 * 1024 / 8;

pub(crate) const BACKSLASH: u16 = b'\\' as u16;

/// Lists `folder` into `page` and gives each entry to `each`, or
/// `ERROR_INVALID_DATA` for an entry whose name a relative open cannot take.
/// Stops early when `stopped` says so. Fails when a page cannot be read.
pub(crate) fn list(
    folder: &Entry,
    page: &mut [u64],
    stopped: impl Fn() -> bool,
    mut each: impl FnMut(Result<FolderEntry, u32>),
) -> Result<(), u32> {
    while !stopped() {
        let entries = match folder.read_listing(page) {
            Ok(Some(bytes)) => walk_full_listing_buffer(bytes).map_err(|_| ERROR_INVALID_DATA)?,
            Ok(None) => return Ok(()),
            Err(error) => return Err(error.code),
        };
        for listed in entries {
            // The relative open takes one component; an empty name would
            // open the folder itself.
            if listed.name.is_empty() || listed.name.contains(&BACKSLASH) {
                each(Err(ERROR_INVALID_DATA));
            } else {
                each(Ok(listed));
            }
        }
    }
    Ok(())
}

/// Whether a walk enters an entry: a folder, unless its reparse tag names
/// another entry. A link is deleted or copied as a link, never entered,
/// because entering it would act on files outside the selection.
pub(crate) fn is_folder(attributes: u32, reparse_tag: u32) -> bool {
    attributes & FILE_ATTRIBUTE_DIRECTORY != 0 && !is_name_surrogate(reparse_tag)
}

/// What an entry is for the conflict rules.
pub(crate) fn kind(attributes: u32, reparse_tag: u32) -> Kind {
    match (
        attributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_name_surrogate(reparse_tag),
    ) {
        (false, _) => Kind::File,
        (true, false) => Kind::Folder,
        (true, true) => Kind::FolderLink,
    }
}

/// The names of a destination folder, read once for its conflicts.
pub(crate) fn read_names(folder: &Entry, page: &mut [u64]) -> Result<Names, u32> {
    let mut names = Names::default();
    list(
        folder,
        page,
        || false,
        |listed| {
            // A name a relative open cannot take is not one a copy could take.
            if let Ok(listed) = listed {
                let found = Found {
                    kind: kind(listed.attributes, listed.reparse_tag.unwrap_or(0)),
                    size: listed.size,
                    created: listed.created,
                    last_write: listed.last_write,
                    reparse_point: listed.reparse_tag.is_some(),
                };
                names.insert(&listed.name, Holder::Found(found));
            }
        },
    )?;
    Ok(names)
}

pub(crate) fn join(folder: &[u16], name: &[u16]) -> Vec<u16> {
    let mut path = folder.to_vec();
    if path.last() != Some(&BACKSLASH) {
        path.push(BACKSLASH);
    }
    path.extend_from_slice(name);
    path
}
