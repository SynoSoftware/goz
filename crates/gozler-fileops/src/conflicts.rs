//! What a copy or a move does with each entry whose name a destination
//! folder already holds ("Conflicts" in the spec). Prepare counts the
//! conflicts with [`classify`], and the run applies the person's choice with
//! [`decide`], so both read one rule.

use crate::names::{Found, Holder, Kind, Names};

/// The person's one answer for all conflicts that prepare found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// A file at the destination is replaced; of two selected entries with
    /// one name, the second gets a number.
    Replace,
    /// A file at the destination stays, and the source is not copied or
    /// moved; of two selected entries with one name, the second is skipped.
    Skip,
    /// The new entry gets the first free number: `report (2).pdf`.
    KeepBoth,
}

/// How the run treats a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rule {
    /// `None` when prepare found no conflict, so the person was not asked:
    /// then every conflict the run meets is skipped and listed, because
    /// nobody chose to replace that file.
    pub(crate) choice: Option<Choice>,
    /// Continue after an interruption: a file already at the destination
    /// with the source's size and last write time is done.
    pub(crate) continuing: bool,
}

/// The source entry that wants a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub(crate) kind: Kind,
    pub(crate) size: u64,
    pub(crate) last_write: i64,
    /// A copy into the entry's own folder gets a number without a question,
    /// as in File Explorer, because Replace would replace the source with
    /// its own copy.
    pub(crate) own_folder: bool,
}

/// What prepare sees for one name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Clash {
    Free,
    /// A copy into the entry's own folder, which always gets a number.
    OwnFolder,
    /// A file, or a link, that the destination holds.
    Conflict,
    /// Another selected entry already took the name.
    SameName,
    /// A folder into a folder of its name: they merge, as in File Explorer.
    Merge,
    /// Continue: the file is already at the destination.
    Done,
    /// A file and a folder with one name, refused whatever the choice, as
    /// File Explorer refuses them.
    Mismatch,
}

pub(crate) fn classify(names: &Names, name: &[u16], wanted: Wanted, rule: Rule) -> Clash {
    let is_folder = wanted.kind != Kind::File;
    if rule.continuing && wanted.kind == Kind::File {
        let done = |holder| matches!(holder, Holder::Found(found) if is_done(found, wanted));
        // Keep both, and a copy into its own folder, wrote a numbered name.
        let numbered = wanted.own_folder || rule.choice == Some(Choice::KeepBoth);
        let plain = !wanted.own_folder && names.get(name).is_some_and(done);
        if plain || (numbered && names.numbered(name, is_folder).any(done)) {
            return Clash::Done;
        }
    }
    if wanted.own_folder {
        return Clash::OwnFolder;
    }
    match names.get(name) {
        None => Clash::Free,
        Some(Holder::Ours) => Clash::SameName,
        Some(Holder::Found(found)) => match (wanted.kind, found.kind) {
            (Kind::Folder, Kind::Folder) => Clash::Merge,
            (wanted, found) if wanted == found => Clash::Conflict,
            _ => Clash::Mismatch,
        },
    }
}

/// What the run does with one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Write the entry under `name`, which this operation now holds;
    /// `replace` renames it over the file found there.
    Write {
        name: Vec<u16>,
        replace: bool,
    },
    /// Merge the folder into the folder of its name.
    Merge,
    Done,
    /// Listed in the result as skipped.
    Skip,
    /// Listed as a failure with `ERROR_ALREADY_EXISTS`.
    Mismatch,
}

/// Applies `rule` to `name`, and takes the name it decides in `names`, so
/// no second entry gets it.
pub(crate) fn decide(names: &mut Names, name: &[u16], wanted: Wanted, rule: Rule) -> Decision {
    let is_folder = wanted.kind != Kind::File;
    let numbered = |names: &mut Names| Decision::Write {
        name: names.keep_both(name, is_folder),
        replace: false,
    };
    match classify(names, name, wanted, rule) {
        Clash::Free => {
            names.insert(name, Holder::Ours);
            Decision::Write {
                name: name.to_vec(),
                replace: false,
            }
        }
        Clash::OwnFolder => numbered(names),
        // Two selected entries never replace each other: on a move that
        // would delete one of the person's files, and on a copy the result
        // would depend on the order.
        Clash::SameName => match rule.choice {
            Some(Choice::Replace | Choice::KeepBoth) => numbered(names),
            Some(Choice::Skip) | None => Decision::Skip,
        },
        Clash::Conflict => match rule.choice {
            Some(Choice::Replace) => {
                names.insert(name, Holder::Ours);
                Decision::Write {
                    name: name.to_vec(),
                    replace: true,
                }
            }
            Some(Choice::KeepBoth) => numbered(names),
            Some(Choice::Skip) | None => Decision::Skip,
        },
        Clash::Merge => Decision::Merge,
        Clash::Done => Decision::Done,
        Clash::Mismatch => Decision::Mismatch,
    }
}

/// Continue's rule: the same size and a last write time within 2 seconds,
/// or within one hour and 2 seconds after a daylight saving change, because
/// FAT keeps times to 2 seconds in local time, as TinyTorrent2's
/// `Payload::IsCopy` compares them.
fn is_done(found: Found, wanted: Wanted) -> bool {
    const SECOND: i64 = 10_000_000;
    let apart = found.last_write.abs_diff(wanted.last_write);
    let close = |to: u64| apart.abs_diff(to) <= (2 * SECOND).unsigned_abs();
    found.kind == Kind::File
        && found.size == wanted.size
        && (close(0) || close((3600 * SECOND).unsigned_abs()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    const FILE: Wanted = Wanted {
        kind: Kind::File,
        size: 10,
        last_write: 1_000_000_000,
        own_folder: false,
    };

    fn found(kind: Kind, size: u64, last_write: i64) -> Holder {
        Holder::Found(Found {
            kind,
            size,
            last_write,
        })
    }

    /// Two selected entries with one name never replace each other: the
    /// second gets a number, or is skipped with Skip; and a copy into its
    /// own folder gets a number without a question. The real-folder tests
    /// cover each choice onto a file at the destination.
    #[test]
    fn selected_entries_never_replace_each_other() {
        let write = |name: &str| Decision::Write {
            name: utf16(name),
            replace: false,
        };
        for (choice, second) in [
            (Some(Choice::Replace), write("C (2).TXT")),
            (Some(Choice::Skip), Decision::Skip),
            (None, Decision::Skip),
        ] {
            let rule = Rule {
                choice,
                continuing: false,
            };
            let mut names = Names::default();
            assert_eq!(
                decide(&mut names, &utf16("c.txt"), FILE, rule),
                write("c.txt")
            );
            assert_eq!(decide(&mut names, &utf16("C.TXT"), FILE, rule), second);
        }
        let own = Wanted {
            own_folder: true,
            ..FILE
        };
        let rule = Rule {
            choice: None,
            continuing: false,
        };
        let mut names = Names::default();
        names.insert(&utf16("d.txt"), found(Kind::File, 10, 0));
        assert_eq!(
            decide(&mut names, &utf16("d.txt"), own, rule),
            write("d (2).txt")
        );
    }

    /// Continue takes a file of the source's size and time as done, at the
    /// name or, for Keep both, at a numbered name, and never takes a file
    /// that differs, nor the source itself in its own folder.
    #[test]
    fn continue_finds_what_is_done() {
        const HOUR: i64 = 3600 * 10_000_000;
        let mut names = Names::default();
        names.insert(
            &utf16("same.txt"),
            found(Kind::File, 10, FILE.last_write + 15_000_000),
        );
        names.insert(
            &utf16("dst.txt"),
            found(Kind::File, 10, FILE.last_write - HOUR),
        );
        names.insert(&utf16("other.txt"), found(Kind::File, 11, FILE.last_write));
        names.insert(
            &utf16("late.txt"),
            found(Kind::File, 10, FILE.last_write + 30_000_000),
        );
        names.insert(&utf16("kept.txt"), found(Kind::File, 1, 0));
        names.insert(
            &utf16("kept (2).txt"),
            found(Kind::File, 10, FILE.last_write),
        );
        let rule = |choice| Rule {
            choice,
            continuing: true,
        };
        let clash =
            |name: &str, wanted, choice| classify(&names, &utf16(name), wanted, rule(choice));
        assert_eq!(clash("same.txt", FILE, None), Clash::Done);
        assert_eq!(clash("dst.txt", FILE, None), Clash::Done);
        assert_eq!(clash("other.txt", FILE, None), Clash::Conflict);
        assert_eq!(clash("late.txt", FILE, None), Clash::Conflict);
        assert_eq!(clash("kept.txt", FILE, Some(Choice::KeepBoth)), Clash::Done);
        assert_eq!(clash("kept.txt", FILE, Some(Choice::Skip)), Clash::Conflict);
        let own = Wanted {
            own_folder: true,
            ..FILE
        };
        assert_eq!(clash("same.txt", own, None), Clash::OwnFolder);
        assert_eq!(clash("kept.txt", own, None), Clash::Done);
    }
}
