//! The names and the destinations the person types in Two panels' dialogs
//! (DESIGN.md: Copy and Move, New folder). The engine opens paths with `\\?\`,
//! which turns off Windows' own trimming and its reserved names, so the dialogs
//! apply those rules here, as File Explorer does: each name loses its trailing
//! dots and spaces, and a device name such as `CON` is refused. Only the text
//! decides, so a dialog closes at once.

use super::panel::expand;
use gozler_core::winpath::{absolute, parent_of};
use gozler_fileops::Destination;
use std::path::PathBuf;

/// Why a typed name or path cannot be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// A name holds a character that no name can hold.
    Characters,
    /// A name is one that Windows keeps for a device, such as `CON` or
    /// `nul.txt`.
    Reserved(String),
}

/// Whether a name may hold `c`, as New folder's box checks it while the
/// person types.
pub(crate) fn allowed_in_name(c: char) -> bool {
    !c.is_control() && !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

/// The name without the trailing dots and spaces that Windows drops; empty
/// when nothing else is left, so New folder's Create stays disabled.
pub(crate) fn trimmed(name: &str) -> &str {
    name.trim_end_matches(['.', ' '])
}

/// New folder's name, trimmed.
pub(crate) fn folder_name(typed: &str) -> Result<String, Refusal> {
    if !typed.chars().all(allowed_in_name) {
        return Err(Refusal::Characters);
    }
    let name = trimmed(typed);
    refuse_reserved(name)?;
    Ok(name.to_owned())
}

/// Where Copy to folder… or Move to folder… writes, from the box's text:
/// environment variables expanded and a relative path joined to `folder`,
/// the active panel's, as in the path box. `other` is the other panel's
/// folder that the box opened with, a real path, so a text that starts
/// with it expands only what follows, and a folder named `%USERNAME%`
/// stays so. `one` is the name of the one entry it acts on, or `None` for
/// several. `None` for an empty text.
///
/// A text that ends in `\` or `/`, or any text for several entries, names a
/// folder. For one entry, a path that ends in the entry's own name, case
/// and trailing dots included, as `F6` fills the box, names the folder that
/// holds it, so a folder of the same name there is merged and not entered;
/// any other path is decided by the engine, which knows whether it is a
/// folder.
pub(crate) fn destination(
    typed: &str,
    folder: &str,
    other: &str,
    one: Option<&str>,
    var: impl Fn(&str) -> Option<String>,
) -> Result<Option<Destination>, Refusal> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    let expanded = match typed.strip_prefix(other) {
        Some(rest) if !other.is_empty() => other.to_owned() + &expand(rest, var),
        _ => expand(typed, var),
    };
    // `absolute` drops the trailing separator, which decides the result.
    let into = expanded.ends_with(['\\', '/']);
    let named = expanded.rsplit(['\\', '/']).next().unwrap_or_default();
    let path = absolute(&checked(&expanded)?, folder);
    Ok(Some(match one {
        _ if into => Destination::Folder(path.into()),
        None => Destination::Folder(path.into()),
        Some(own) if named == own || trimmed(named) == own => {
            Destination::Folder(PathBuf::from(parent_of(&path)))
        }
        Some(_) => Destination::Typed(path.into()),
    }))
}

/// `path` with each name trimmed, after it was checked: a name that only
/// dots and spaces made is dropped, as an empty one is. A drive such as
/// `D:` may start it.
fn checked(path: &str) -> Result<String, Refusal> {
    let drive = match path.as_bytes() {
        [letter, b':', ..] if letter.is_ascii_alphabetic() => 2,
        _ => 0,
    };
    let (drive, rest) = path.split_at(drive);
    let mut out = drive.to_owned();
    for (index, name) in rest.split(['\\', '/']).enumerate() {
        if index > 0 {
            out.push('\\');
        }
        if name == "." || name == ".." {
            out.push_str(name);
            continue;
        }
        if !name.chars().all(allowed_in_name) {
            return Err(Refusal::Characters);
        }
        let name = trimmed(name);
        refuse_reserved(name)?;
        out.push_str(name);
    }
    Ok(out)
}

/// Refuses a device name, also with an extension, such as `nul.txt`.
fn refuse_reserved(name: &str) -> Result<(), Refusal> {
    let stem = name.split('.').next().unwrap_or_default().trim_end();
    let reserved = matches!(
        stem.to_ascii_uppercase().as_bytes(),
        b"CON"
            | b"PRN"
            | b"AUX"
            | b"NUL"
            | [b'C', b'O', b'M', b'1'..=b'9']
            | [b'L', b'P', b'T', b'1'..=b'9']
    );
    match reserved {
        true => Err(Refusal::Reserved(name.to_owned())),
        false => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(typed: &str, one: Option<&str>) -> Result<Option<Destination>, Refusal> {
        destination(typed, r"C:\Work", r"D:\%BACKUP%\", one, |name| {
            (name == "BACKUP").then(|| r"D:\Backup".to_owned())
        })
    }

    /// The engine opens `\\?\` paths, which keep a trailing dot and open a
    /// device for `CON`, so a name Windows would change or refuse must be
    /// changed or refused before it reaches the engine.
    #[test]
    fn a_name_is_trimmed_and_a_device_or_a_character_is_refused() {
        assert_eq!(folder_name("Photos. . "), Ok("Photos".to_owned()));
        assert_eq!(
            folder_name("Nul.txt"),
            Err(Refusal::Reserved("Nul.txt".to_owned()))
        );
        assert_eq!(folder_name("COM10"), Ok("COM10".to_owned()));
        assert_eq!(folder_name("a?b"), Err(Refusal::Characters));
        assert_eq!(
            resolve(r"D:\Backup\lpt1\x", None),
            Err(Refusal::Reserved("lpt1".to_owned()))
        );
        assert_eq!(resolve(r"D:\Back|up\", None), Err(Refusal::Characters));
    }

    /// `F6` fills the box with the other folder and the entry's name: left
    /// as it is, the entry goes into that folder, merged with a folder of
    /// its name there, not into that folder (DESIGN.md: Copy and Move). Any
    /// other name, also one that differs only in case, is the engine's to
    /// decide, and a trailing `\` names a folder that `absolute` would
    /// otherwise turn into a new name.
    #[test]
    fn the_dialog_decides_what_its_text_alone_decides() {
        let folder = |path: &str| Some(Destination::Folder(path.into()));
        assert_eq!(
            resolve(r"D:\Backup\Photos", Some("Photos")),
            Ok(folder(r"D:\Backup"))
        );
        assert_eq!(
            resolve(r"D:\Backup\photos", Some("Photos")),
            Ok(Some(Destination::Typed(r"D:\Backup\photos".into())))
        );
        assert_eq!(
            resolve(r"%BACKUP%\2026\", Some("report.pdf")),
            Ok(folder(r"D:\Backup\2026"))
        );
        assert_eq!(
            resolve(r"D:\Backup\2026", None),
            Ok(folder(r"D:\Backup\2026"))
        );
        assert_eq!(
            resolve("report-old.pdf. ", Some("report.pdf")),
            Ok(Some(Destination::Typed(r"C:\Work\report-old.pdf".into())))
        );
        assert_eq!(resolve("  ", Some("report.pdf")), Ok(None));
    }

    /// The box opens with the other panel's folder, a real path, which a
    /// folder named `%BACKUP%` keeps, and `F6` on `notes.` with the box left
    /// as it opened moves `notes.` there, not renamed to `notes` (#111).
    #[test]
    fn the_folder_and_the_name_the_box_opens_with_are_kept() {
        let folder = |path: &str| Some(Destination::Folder(path.into()));
        assert_eq!(
            resolve(r"D:\%BACKUP%\notes.", Some("notes.")),
            Ok(folder(r"D:\%BACKUP%"))
        );
        assert_eq!(resolve(r"D:\%BACKUP%\", None), Ok(folder(r"D:\%BACKUP%")));
    }
}
