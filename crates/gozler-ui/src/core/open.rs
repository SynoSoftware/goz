//! The open commands of Settings > Context menu (DESIGN.md: The Context
//! menu page): Open folders, Open files and Open file location, in which
//! `%1` is the entry's full path, as in Everything and in Windows' own
//! commands. Enter, double-click and menu Open share these commands in both
//! modes; an empty command uses Windows' default. Navigation into Commander
//! is a separate command.

use super::text::name_of;
use gozler_core::proto::ResultItem;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum EntryAction {
    Open,
    OpenFileLocation,
    Explore,
    Properties,
}

impl EntryAction {
    pub(crate) const ALL: [Self; 4] = [
        Self::Open,
        Self::OpenFileLocation,
        Self::Explore,
        Self::Properties,
    ];
}

/// A selected path in UTF-16 without a trailing NUL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Selected {
    pub(crate) path: Vec<u16>,
    pub(crate) is_folder: bool,
}

impl Selected {
    pub(crate) fn of_result(item: &ResultItem) -> Self {
        Self {
            path: super::selection::wide_path(item),
            is_folder: item.is_dir,
        }
    }
}

/// Captures the command's entries without materializing a large selection on UI.
pub(crate) enum Entries {
    Ready(Vec<Selected>),
    Results(Arc<[ResultItem]>),
    Panel(super::panel::PanelEntries),
}

impl Entries {
    pub(crate) fn items(&self) -> usize {
        match self {
            Self::Ready(entries) => entries.len(),
            Self::Results(files) => files.len(),
            Self::Panel(entries) => entries.items(),
        }
    }

    pub(crate) fn resolve(self) -> Vec<Selected> {
        match self {
            Self::Ready(entries) => entries,
            Self::Results(files) => files.iter().map(Selected::of_result).collect(),
            Self::Panel(entries) => entries.resolve(),
        }
    }

    pub(crate) fn one(&self) -> Option<Selected> {
        match self {
            Self::Ready(entries) => match entries.as_slice() {
                [one] => Some(one.clone()),
                _ => None,
            },
            Self::Results(files) => match files.as_ref() {
                [one] => Some(Selected::of_result(one)),
                _ => None,
            },
            Self::Panel(entries) => entries.one(),
        }
    }
}

pub(crate) struct Commands {
    pub(crate) folders: String,
    pub(crate) files: String,
    pub(crate) location: String,
}

impl Commands {
    pub(crate) fn expand(&self, action: EntryAction, entry: &Selected) -> Option<CommandLine> {
        let command = match action {
            EntryAction::Open if entry.is_folder => &self.folders,
            EntryAction::Open => &self.files,
            EntryAction::OpenFileLocation => &self.location,
            EntryAction::Explore | EntryAction::Properties => return None,
        };
        expand(command, &String::from_utf16_lossy(&entry.path))
    }
}

/// The command that Browse… writes for the chosen program: the program in
/// quotes, so a path with spaces stays one, and the entry's path in quotes.
pub(crate) fn browse_command(program: &str) -> String {
    format!("\"{program}\" \"%1\"")
}

/// The owner's Total Commander command from Everything, with Total
/// Commander at `path`: `/O` uses the running Total Commander, `/A` ignores
/// its ask-before-replace setting, `/P=L` and `/L=` open the entry on the
/// left panel.
pub(crate) fn total_commander_command(path: &str) -> String {
    format!("\"{path}\" /O /A /P=L /L=\"%1\"")
}

/// The search that finds Total Commander in the index, newest first.
pub(crate) const TOTAL_COMMANDER_SEARCH: &str = "file: totalcmd*.exe";

/// The Total Commander that the Use Total Commander link takes from
/// `rows`, which are sorted by date modified, newest first: the newest
/// `TOTALCMD64.EXE`, or the newest `TOTALCMD.EXE` when there is no 64-bit
/// one, because an older copy is usually a backup.
pub(crate) fn newest_total_commander(rows: &[ResultItem]) -> Option<&str> {
    ["TOTALCMD64.EXE", "TOTALCMD.EXE"]
        .into_iter()
        .find_map(|name| {
            rows.iter()
                .filter(|row| !row.is_dir && name_of(&row.path).eq_ignore_ascii_case(name))
                .max_by_key(|row| row.mtime_ft)
                .map(|row| row.path.as_str())
        })
}

/// A command with `%1` replaced, split into the program and its
/// parameters as Windows starts it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct CommandLine {
    pub(crate) program: String,
    pub(crate) parameters: String,
}

/// `command` for the entry at `path`, or `None` when the command is empty,
/// which does what File Explorer does. The program is the quoted start of
/// the command, or its first word.
fn expand(command: &str, path: &str) -> Option<CommandLine> {
    let command = command.trim().replace("%1", path);
    if command.is_empty() {
        return None;
    }
    let (program, rest) = match command.strip_prefix('"') {
        Some(quoted) => quoted.split_once('"').unwrap_or((quoted, "")),
        None => command
            .split_once(char::is_whitespace)
            .unwrap_or((&command, "")),
    };
    Some(CommandLine {
        program: program.to_owned(),
        parameters: rest.trim().to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::results::tests::chunk;

    /// A program path with spaces that is not quoted runs another program,
    /// and an entry's path with spaces reaches it as several parameters.
    #[test]
    fn browse_writes_a_quoted_command_that_runs_for_a_path_with_spaces() {
        let command = browse_command(r"C:\Windows\notepad.exe");
        assert_eq!(command, r#""C:\Windows\notepad.exe" "%1""#);
        let tc = total_commander_command(r"C:\Program Files\totalcmd\TOTALCMD64.EXE");
        assert_eq!(
            expand(&tc, r"D:\My files"),
            Some(CommandLine {
                program: r"C:\Program Files\totalcmd\TOTALCMD64.EXE".into(),
                parameters: r#"/O /A /P=L /L="D:\My files""#.into(),
            })
        );
        assert_eq!(
            expand("  ", r"D:\x"),
            None,
            "empty does what File Explorer does"
        );
    }

    /// An older copy is usually a backup, and a 32-bit Total Commander
    /// beside a 64-bit one is the one the person does not use.
    #[test]
    fn use_total_commander_takes_the_newest_64_bit_one() {
        let mut rows = chunk(0, 4, 4).rows;
        let found = [
            (r"C:\Backup\TOTALCMD64.EXE", 100),
            (r"C:\Program Files\totalcmd\TOTALCMD64.EXE", 300),
            (r"C:\Old\TOTALCMD.EXE", 500),
            (r"C:\Tools\totalcmd64.exe.lnk", 900),
        ];
        for (row, (path, mtime)) in std::sync::Arc::make_mut(&mut rows).iter_mut().zip(found) {
            row.path = path.into();
            row.mtime_ft = Some(mtime);
            row.is_dir = false;
        }
        assert_eq!(
            newest_total_commander(&rows),
            Some(r"C:\Program Files\totalcmd\TOTALCMD64.EXE")
        );
        assert_eq!(
            newest_total_commander(&rows[2..]),
            Some(r"C:\Old\TOTALCMD.EXE")
        );
        assert_eq!(newest_total_commander(&rows[3..]), None);
    }
}
