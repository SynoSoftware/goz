//! The request gozler sends for a command line.

use super::EsPlan;
use crate::proto::{Returns, Rows, SearchRequest};
use crate::query::{QueryError, parse_query_with_case};

/// The request for `plan`: its query parsed, its scope made absolute against
/// `working_dir`, an absolute path such as `C:\repo`, and either the rows to
/// show or, for a count, the totals.
///
/// The scope is joined with Windows' rules, without asking the operating
/// system, so a command line builds the same request on any machine: `C:\x`
/// and `\\server\share\x` stay as they are, `\x` is on the working
/// directory's drive, and anything else is below the working directory. `.`
/// and `..` components are then removed, as `GetFullPathNameW` removes them,
/// because the daemon looks each component up as a folder. A drive-relative
/// scope such as `D:x` depends on the current folder of each drive, which
/// only Windows knows, so the caller resolves it first (see
/// [`is_drive_relative`]); given one, this joins it below the working
/// directory on that drive, and below the root of any other drive.
pub fn build_search(plan: &EsPlan, working_dir: &str) -> Result<SearchRequest, QueryError> {
    let returns = match plan.count {
        Some(_) => Returns::Totals,
        None => Returns::Rows(Rows {
            sort: plan.sort,
            offset: 0,
            limit: plan.row_limit(),
            // Always fetch metadata; output decides what to show.
            want_size: true,
            want_mtime: true,
        }),
    };
    Ok(SearchRequest {
        query: parse_query_with_case(&plan.query, plan.match_case)?,
        scope: plan
            .scope
            .as_deref()
            .map(|scope| absolute(scope, working_dir)),
        returns,
    })
}

/// Whether `path` names a drive but not its root, such as `D:x` or `D:`.
pub fn is_drive_relative(path: &str) -> bool {
    match path.as_bytes() {
        [letter, b':', rest @ ..] => {
            letter.is_ascii_alphabetic() && rest.first().is_none_or(|&c| !is_separator(c))
        }
        _ => false,
    }
}

fn is_separator(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

/// `path` joined to `working_dir` and without `.` and `..` components, every
/// separator a `\`. A `\\?\` or `\\.\` path names a path verbatim or a device,
/// so it stays as it is, as `GetFullPathNameW` leaves it.
fn absolute(path: &str, working_dir: &str) -> String {
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
    use crate::escompat::{Command, parse_argv};

    fn plan(tokens: &[&str]) -> EsPlan {
        let args: Vec<String> = tokens.iter().map(ToString::to_string).collect();
        match parse_argv(&args) {
            Ok(Command::Run(plan)) => plan,
            other => panic!("{tokens:?}: {other:?}"),
        }
    }

    /// The daemon looks each component of a scope up as a child folder and
    /// routes the scope by its drive, so every form a user types must arrive
    /// absolute, without `.` or `..`.
    #[test]
    fn a_scope_is_made_absolute_with_windows_rules() {
        let cases = [
            (r"C:\repo\docs\", r"D:\other", r"C:\repo\docs"),
            ("C:/repo/./src/../docs", r"D:\other", r"C:\repo\docs"),
            (".", r"C:\repo\src", r"C:\repo\src"),
            (r"..\docs", r"C:\repo\src", r"C:\repo\docs"),
            (r"\tmp", r"C:\repo\src", r"C:\tmp"),
            (r"C:\..\..", r"D:\other", r"C:\"),
            (r"\\server\share\a\..\b", r"C:\repo", r"\\server\share\b"),
            (r"\x", r"\\server\share\dir", r"\\server\share\x"),
            (r"\\?\Volume{1}\a\..", r"C:\repo", r"\\?\Volume{1}\a\.."),
            ("c:x", r"C:\repo", r"C:\repo\x"),
            ("D:x", r"C:\repo", r"D:\x"),
        ];
        for (scope, working_dir, absolute) in cases {
            let request = build_search(&plan(&["-path", scope]), working_dir).expect("parses");
            assert_eq!(
                request.scope.as_deref(),
                Some(absolute),
                "{scope} in {working_dir}"
            );
        }
        assert!(is_drive_relative("D:x") && is_drive_relative("D:"));
        assert!(!is_drive_relative(r"D:\x") && !is_drive_relative("x"));
    }

    /// A count asks for the totals, which cover every match, so `-n` never
    /// reaches it; rows carry the limit.
    #[test]
    fn a_count_asks_for_the_totals_and_rows_carry_the_limit() {
        let returns =
            |tokens: &[&str]| build_search(&plan(tokens), r"C:\").expect("parses").returns;
        assert_eq!(returns(&["-get-result-count", "-n", "5"]), Returns::Totals);
        assert_eq!(returns(&["-get-folder-size", r"C:\x"]), Returns::Totals);
        assert!(matches!(
            returns(&["-n", "5"]),
            Returns::Rows(Rows { limit: Some(5), .. })
        ));
    }
}
