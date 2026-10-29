//! Folder links: the junctions, folder symbolic links and folder mounts that
//! give a folder more than one path.
//!
//! The index holds each entry once, at its real path, and never enters a
//! link, as Windows' own MFT readers do not. A person who can open a folder
//! through a link expects a search to find what is in it, so gozlerd keeps
//! the [`Links`] of every volume, and the query engine tests the paths that
//! reach an entry through them (`query::through`). [`parse_reparse_data`]
//! reads a link's target from the disk's bytes.

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use zerocopy::little_endian::{U16, U32};
use zerocopy::{FromBytes, Immutable, KnownLayout, Unaligned};

/// The most links Windows follows when it opens one path; past it the open
/// fails, so no path a person can open passes more.
pub const MAX_LINK_HOPS: usize = 63;

/// A folder that leads to another: a junction, a folder symbolic link, or a
/// folder that mounts a volume. Both paths are full paths as gozler prints
/// them, without a trailing separator, and a volume's root is its root name,
/// such as `V:`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct FolderLink {
    /// The link's real path, such as `C:\repo\artifacts`.
    pub link: String,
    /// The real path of the folder it leads to, such as `V:\artifacts\repo`.
    pub target: String,
}

/// Every folder link that gozlerd knows of, on every volume.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(from = "Vec<FolderLink>")]
pub struct Links {
    folders: Vec<FolderLink>,
    fingerprint: u64,
}

impl From<Vec<FolderLink>> for Links {
    fn from(folders: Vec<FolderLink>) -> Self {
        use std::hash::{Hash, Hasher};
        let mut hasher = rustc_hash::FxHasher::default();
        folders.hash(&mut hasher);
        Self {
            folders,
            fingerprint: hasher.finish(),
        }
    }
}

impl Default for Links {
    fn default() -> Self {
        Vec::new().into()
    }
}

impl Serialize for Links {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.folders.serialize(serializer)
    }
}

impl Links {
    /// The table of `links`, each held once. A target that is reached
    /// through other links is written as its real path, so a target never
    /// names a link.
    pub fn new(links: Vec<FolderLink>) -> Self {
        let mut table = Links {
            folders: links,
            fingerprint: 0,
        };
        table.folders.sort();
        table.folders.dedup();
        let folded = table.folded();
        let real: Vec<String> = (table.folders.iter())
            .map(|l| table.follow(&folded, &l.target))
            .collect();
        for (link, real) in table.folders.iter_mut().zip(real) {
            link.target = root_name_of(real);
        }
        table.folders.sort();
        table.folders.dedup();
        table.folders.into()
    }

    pub fn as_slice(&self) -> &[FolderLink] {
        &self.folders
    }

    pub fn into_vec(self) -> Vec<FolderLink> {
        self.folders
    }

    pub fn is_empty(&self) -> bool {
        self.folders.is_empty()
    }

    /// A number that changes when the table does, so a client that holds a
    /// copy asks for the table only when it changed.
    pub fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// `path`, a full path, with each link on it followed as Windows follows
    /// it when it opens the path: from the left, each link replaced by its
    /// target, at most [`MAX_LINK_HOPS`] times. A drive's root keeps its
    /// separator, as in `V:\`. Names compare ignoring case, as the index
    /// looks folders up.
    pub fn real_path(&self, path: &str) -> String {
        self.follow(&self.folded(), path)
    }

    /// The real folder to scan, or every volume when a link inside that
    /// folder leads out of it. Coverage uses the same routing as the query.
    pub fn search_scope(&self, scope: Option<&str>) -> Option<String> {
        let real = self.real_path(scope?);
        (!self.lead_out_of(&real)).then_some(real)
    }

    fn folded(&self) -> FxHashMap<Vec<u8>, usize> {
        (self.folders.iter())
            .enumerate()
            .map(|(number, l)| {
                (
                    crate::fold::fold(l.link.trim_end_matches('\\').as_bytes()),
                    number,
                )
            })
            .collect()
    }

    /// [`Self::real_path`] with the links [`Self::folded`], so a caller that
    /// follows many paths folds each link once.
    fn follow(&self, folded: &FxHashMap<Vec<u8>, usize>, path: &str) -> String {
        let mut path = path.to_owned();
        for _ in 0..MAX_LINK_HOPS {
            let compared = crate::fold::fold(path.as_bytes());
            let ends = path
                .match_indices('\\')
                .map(|(at, _)| at)
                .chain([path.len()]);
            let folded_ends = compared
                .iter()
                .enumerate()
                .filter(|(_, b)| **b == b'\\')
                .map(|(at, _)| at)
                .chain([compared.len()]);
            let nearest = ends.zip(folded_ends).find_map(|(end, folded_end)| {
                folded
                    .get(&compared[..folded_end])
                    .map(|&number| (number, end))
            });
            let Some((number, end)) = nearest else {
                break;
            };
            path = self.folders[number].target.clone() + &path[end..];
        }
        if is_root_name(&path) {
            path.push('\\');
        }
        path
    }

    /// Whether a link below `folder`, a real path, leads to a folder outside
    /// it, so the folder can be opened at paths that a search of its own
    /// entries does not reach. Links that lead out further from there lie
    /// below that link's target, not below `folder`.
    pub fn lead_out_of(&self, folder: &str) -> bool {
        (self.folders.iter())
            .any(|l| below(&l.link, folder).is_some() && below(&l.target, folder).is_none())
    }
}

/// What follows `folder` in `path` (empty, or starting with `\`) when `path`
/// is `folder` or below it, comparing names ignoring case.
pub(crate) fn below<'p>(path: &'p str, folder: &str) -> Option<&'p str> {
    let folder = folder.trim_end_matches('\\');
    let depth = folder.split('\\').count();
    let end = path
        .match_indices('\\')
        .map(|(at, _)| at)
        .chain([path.len()])
        .nth(depth - 1)?;
    let same = crate::fold::fold(&path.as_bytes()[..end]) == crate::fold::fold(folder.as_bytes());
    same.then(|| &path[end..])
}

/// Whether `path` is a drive's root name, such as `V:`.
fn is_root_name(path: &str) -> bool {
    matches!(path.as_bytes(), [letter, b':'] if letter.is_ascii_alphabetic())
}

/// `path` without the separator of a drive's root, as a target names it.
fn root_name_of(mut path: String) -> String {
    if path.len() == 3 && path.ends_with(":\\") {
        path.pop();
    }
    path
}

/// Where a folder link leads, as its reparse data names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReparseTarget {
    /// A full local path on a drive or a volume GUID path.
    Absolute(String),
    /// A relative symbolic link's path, below the folder that holds the
    /// link, such as `..\shared`.
    Relative(String),
}

/// Why [`parse_reparse_data`] refused a buffer.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ReparseError {
    #[error("the reparse data ends before its header or its names")]
    Truncated,
    #[error("the reparse data names a path that is not UTF-16")]
    NotUtf16,
}

const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xA000_0003;
const IO_REPARSE_TAG_SYMLINK: u32 = 0xA000_000C;
const SYMLINK_FLAG_RELATIVE: u32 = 1;

/// `REPARSE_DATA_BUFFER`'s header.
#[derive(FromBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawHeader {
    tag: U32,
    data_length: U16,
    #[allow(dead_code)]
    reserved: U16,
}

/// The names of a mount point or a symbolic link: offsets and lengths in
/// bytes, from the start of the path buffer.
#[derive(FromBytes, KnownLayout, Immutable, Unaligned)]
#[repr(C)]
struct RawNames {
    substitute_offset: U16,
    substitute_length: U16,
    #[allow(dead_code)]
    print_offset: U16,
    #[allow(dead_code)]
    print_length: U16,
}

/// The target of a folder link from its `REPARSE_DATA_BUFFER`, as
/// `FSCTL_GET_REPARSE_POINT` returns it. `Ok(None)` for a reparse point that
/// leads to no local folder: another tag, such as a cloud file's, a share
/// or a device. Volume GUID targets are resolved through mount metadata.
///
/// # Errors
///
/// [`ReparseError`] when the buffer is too short for what it says it holds,
/// or a name is not UTF-16.
pub fn parse_reparse_data(bytes: &[u8]) -> Result<Option<ReparseTarget>, ReparseError> {
    let (header, rest) = RawHeader::read_from_prefix(bytes).map_err(|_| ReparseError::Truncated)?;
    let rest = rest
        .get(..usize::from(header.data_length.get()))
        .ok_or(ReparseError::Truncated)?;
    let (names, rest) = match header.tag.get() {
        IO_REPARSE_TAG_MOUNT_POINT => {
            let (names, rest) =
                RawNames::read_from_prefix(rest).map_err(|_| ReparseError::Truncated)?;
            (names, (rest, false))
        }
        IO_REPARSE_TAG_SYMLINK => {
            let (names, rest) =
                RawNames::read_from_prefix(rest).map_err(|_| ReparseError::Truncated)?;
            let (flags, rest) = U32::read_from_prefix(rest).map_err(|_| ReparseError::Truncated)?;
            (names, (rest, flags.get() & SYMLINK_FLAG_RELATIVE != 0))
        }
        _ => return Ok(None),
    };
    let (buffer, relative) = rest;
    let start = usize::from(names.substitute_offset.get());
    if start % 2 != 0 || names.substitute_length.get() % 2 != 0 {
        return Err(ReparseError::NotUtf16);
    }
    let end = start + usize::from(names.substitute_length.get());
    let name = buffer.get(start..end).ok_or(ReparseError::Truncated)?;
    let units: Vec<u16> = name
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u16::from_le_bytes(*pair))
        .collect();
    let name = String::from_utf16(&units).map_err(|_| ReparseError::NotUtf16)?;
    if relative {
        return Ok(Some(ReparseTarget::Relative(name.replace('/', "\\"))));
    }
    let Some(path) = name.strip_prefix(r"\??\") else {
        return Ok(None);
    };
    if path.starts_with("Volume{") && path.contains(r"}\") {
        return Ok(Some(ReparseTarget::Absolute(format!(r"\\?\{path}"))));
    }
    let on_drive =
        matches!(path.as_bytes(), [letter, b':', b'\\', ..] if letter.is_ascii_alphabetic());
    if !on_drive {
        return Ok(None);
    }
    let mut path = path.trim_end_matches('\\').to_owned();
    if is_root_name(&path) {
        path.push('\\');
    }
    Ok(Some(ReparseTarget::Absolute(path)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    /// A `REPARSE_DATA_BUFFER` of `tag` whose substitute name is
    /// `substitute`, as Windows writes a junction's or a symbolic link's.
    fn reparse(tag: u32, substitute: &str, flags: Option<u32>) -> Vec<u8> {
        let name: Vec<u8> = substitute
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        let mut out = Vec::new();
        out.extend_from_slice(&tag.to_le_bytes());
        let length = 8 + flags.map_or(0, |_| 4) + name.len();
        out.extend_from_slice(&u16::try_from(length).unwrap().to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        for value in [0, name.len(), name.len(), 0] {
            out.extend_from_slice(&u16::try_from(value).unwrap().to_le_bytes());
        }
        if let Some(flags) = flags {
            out.extend_from_slice(&flags.to_le_bytes());
        }
        out.extend_from_slice(&name);
        out
    }

    /// A link the index cannot follow, such as one to a share, must not
    /// become a target that a scope or a path term is rewritten to.
    #[test]
    fn a_link_leads_to_a_folder_on_a_drive_or_nowhere() {
        let target = |bytes: Vec<u8>| parse_reparse_data(&bytes).expect("well formed");
        assert_eq!(
            target(reparse(
                IO_REPARSE_TAG_MOUNT_POINT,
                r"\??\V:\artifacts\repo\",
                None
            )),
            Some(ReparseTarget::Absolute(r"V:\artifacts\repo".into()))
        );
        assert_eq!(
            target(reparse(IO_REPARSE_TAG_SYMLINK, r"..\shared", Some(1))),
            Some(ReparseTarget::Relative(r"..\shared".into()))
        );
        assert_eq!(
            target(reparse(IO_REPARSE_TAG_SYMLINK, r"\??\D:\", Some(0))),
            Some(ReparseTarget::Absolute(r"D:\".into()))
        );
        assert_eq!(
            target(reparse(
                IO_REPARSE_TAG_MOUNT_POINT,
                r"\??\Volume{1b2c}\",
                None
            )),
            Some(ReparseTarget::Absolute(r"\\?\Volume{1b2c}\".into()))
        );
        for nowhere in [r"\??\UNC\nas\media", r"\??\V:relative"] {
            assert_eq!(
                target(reparse(IO_REPARSE_TAG_MOUNT_POINT, nowhere, None)),
                None
            );
        }
        assert_eq!(
            target(reparse(0x9000_001A, r"\??\C:\x", None)),
            None,
            "a cloud file"
        );
    }

    proptest! {
        /// Bytes from the disk can be anything; they give an error or a
        /// target, never a panic.
        #[test]
        fn any_bytes_parse_without_panic(bytes in proptest::collection::vec(any::<u8>(), 0..64)) {
            let _ = parse_reparse_data(&bytes);
        }
    }

    #[test]
    fn a_reparse_name_stays_inside_its_declared_aligned_payload() {
        let valid = reparse(IO_REPARSE_TAG_MOUNT_POINT, r"\??\V:\folder", None);
        let mut short = valid.clone();
        short[4..6].copy_from_slice(&8u16.to_le_bytes());
        assert_eq!(parse_reparse_data(&short), Err(ReparseError::Truncated));
        for field in [8, 10] {
            let mut odd = valid.clone();
            odd[field] |= 1;
            assert_eq!(parse_reparse_data(&odd), Err(ReparseError::NotUtf16));
        }
    }

    /// The owner's case: `C:\…\artifacts` is a junction to
    /// `V:\artifacts\TinyTorrent2`, and a folder mounts a volume.
    #[test]
    fn a_path_through_links_is_its_real_folder() {
        let links = Links::new(vec![
            FolderLink {
                link: r"C:\SynoSoftware\TinyTorrent2\artifacts".into(),
                target: r"V:\artifacts\TinyTorrent2".into(),
            },
            FolderLink {
                link: r"C:\mnt\data".into(),
                target: "E:".into(),
            },
            FolderLink {
                link: r"E:\loop".into(),
                target: "E:".into(),
            },
        ]);
        assert_eq!(
            links.real_path(r"c:\synosoftware\tinytorrent2\ARTIFACTS\evidence\"),
            r"V:\artifacts\TinyTorrent2\evidence\"
        );
        assert_eq!(links.real_path(r"C:\mnt\data"), r"E:\");
        assert_eq!(links.real_path(r"C:\mnt\data\loop\loop\x"), r"E:\x");
        assert_eq!(links.real_path(r"C:\mnt\database"), r"C:\mnt\database");
    }
}
