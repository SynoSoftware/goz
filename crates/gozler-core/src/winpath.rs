//! Windows path rules, applied without asking the operating system, so a
//! command line builds the same request on any machine, the Ubuntu CI
//! included. `std::path::absolute` would turn `C:\repo` into a Unix path
//! there.

/// Whether `path` names a drive but not its root, such as `D:x` or `D:`. Its
/// folder depends on the current folder of that drive, which only Windows
/// knows, so the caller resolves it before [`absolute`].
pub fn is_drive_relative(path: &str) -> bool {
    match path.as_bytes() {
        [letter, b':', rest @ ..] => {
            letter.is_ascii_alphabetic() && rest.first().is_none_or(|&c| !is_separator(c))
        }
        _ => false,
    }
}

/// Whether `path` starts at a root: a drive's (`C:\`), a share's
/// (`\\server\share`), or a verbatim or device path's (`\\?\`, `\\.\`).
pub(crate) fn is_absolute(path: &str) -> bool {
    root(path).is_some()
}

/// `path` joined to `working_dir`, an absolute path such as `C:\repo`, and
/// without `.` and `..` components, every separator a `\`.
///
/// The join follows Windows' rules: `C:\x` and `\\server\share\x` stay as
/// they are, `\x` is on the working directory's drive, and anything else is
/// below the working directory. `.` and `..` are then removed, as
/// `GetFullPathNameW` removes them, because the daemon looks each component
/// of a scope up as a folder. A `\\?\` or `\\.\` path names a path verbatim
/// or a device, so it stays as it is, as `GetFullPathNameW` leaves it. A
/// drive-relative path such as `D:x` is joined below the working directory
/// on that drive, and below the root of any other drive; see
/// [`is_drive_relative`].
pub fn absolute(path: &str, working_dir: &str) -> String {
    if path.starts_with(r"\\?\") || path.starts_with(r"\\.\") {
        return path.to_string();
    }
    let (wd_root, wd_rest) = root(working_dir).unwrap_or((working_dir, ""));
    let (root, base, rest) = if let Some((root, rest)) = root(path) {
        (root, "", rest)
    } else if path.starts_with(['\\', '/']) {
        (wd_root, "", path)
    } else if is_drive_relative(path) {
        let (drive, rest) = path.split_at(2);
        if drive.eq_ignore_ascii_case(wd_root) {
            (wd_root, wd_rest, rest)
        } else {
            (drive, "", rest)
        }
    } else {
        (wd_root, wd_rest, path)
    };

    let mut components: Vec<&str> = Vec::new();
    for component in base.split(['\\', '/']).chain(rest.split(['\\', '/'])) {
        match component {
            "" | "." => {}
            // `..` at the root stays at the root.
            ".." => {
                components.pop();
            }
            name => components.push(name),
        }
    }
    let mut out = root.replace('/', r"\");
    for name in &components {
        out.push('\\');
        out.push_str(name);
    }
    if components.is_empty() {
        out.push('\\');
    }
    out
}

/// The folder that holds `path`, an absolute path as [`absolute`] writes it.
/// A root holds itself.
pub fn parent_of(path: &str) -> String {
    let (root, rest) = root(path).unwrap_or((path, ""));
    match rest.trim_end_matches('\\').rsplit_once('\\') {
        Some((folder, _)) if !folder.is_empty() => format!("{root}{folder}"),
        _ => format!(r"{root}\"),
    }
}

/// Whether [`parent_of`] gives `path` back, ignoring trailing `\`: a
/// drive's or a share's root, or a path that is not absolute. It allocates
/// nothing, because a panel asks for each row it reads.
pub fn is_root(path: &str) -> bool {
    root(path).is_none_or(|(_, rest)| rest.bytes().all(|c| c == b'\\'))
}

fn is_separator(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

/// An absolute path split into its root and the rest: `C:` and `\x` for
/// `C:\x`, `\\server\share` and `\x` for `\\server\share\x`. `None` when the
/// path is not absolute.
fn root(path: &str) -> Option<(&str, &str)> {
    let bytes = path.as_bytes();
    match bytes {
        [a, b, ..] if is_separator(*a) && is_separator(*b) => {
            // The root ends at the separator after the share name.
            let end = bytes
                .iter()
                .enumerate()
                .skip(2)
                .filter(|&(_, &c)| is_separator(c))
                .nth(1)
                .map_or(path.len(), |(at, _)| at);
            Some(path.split_at(end))
        }
        [letter, b':', separator, ..]
            if letter.is_ascii_alphabetic() && is_separator(*separator) =>
        {
            Some(path.split_at(2))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// A panel shows `..` unless [`is_root`] says its folder is a root.
        /// Where it differs from [`parent_of`], a root shows a `..` that
        /// leads to itself, or a folder loses its `..`.
        #[test]
        fn a_root_is_its_own_parent(path in r"(\\\\|[A-Za-z]:|)[\\/a-z]{0,8}") {
            let same = parent_of(&path).trim_end_matches('\\') == path.trim_end_matches('\\');
            prop_assert_eq!(is_root(&path), same);
        }
    }
}
