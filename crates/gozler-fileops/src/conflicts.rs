//! What a copy or a move does with each entry whose name a destination
//! folder already holds. Preparation and execution share [`classify`];
//! [`decide`] also reserves the chosen name for the operation.

use crate::names::{Found, Holder, Kind, Names};

/// The file conflict rule captured when a copy or move is accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Policy {
    #[default]
    UpdateChanged,
    UpdateNewer,
    /// Unconditional replacement retained for legacy interrupted operations.
    Replace,
    /// A file at the destination stays, and the source is not copied or
    /// moved; of two selected entries with one name, the second is skipped.
    Skip,
    /// The new entry gets the first free number: `report (2).pdf`.
    KeepBoth,
}

impl Policy {
    pub const ALL: [Policy; 4] = [
        Self::UpdateChanged,
        Self::UpdateNewer,
        Self::KeepBoth,
        Self::Skip,
    ];

    pub(crate) fn replaces(self, source_size: u64, source_time: i64, size: u64, time: i64) -> bool {
        match self {
            Self::UpdateChanged => source_size != size || source_time != time,
            Self::UpdateNewer => source_time > time,
            Self::Replace => true,
            Self::Skip | Self::KeepBoth => false,
        }
    }
}

/// How the run treats a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Rule {
    /// Copies and moves capture their policy before preparation; other work has none.
    pub(crate) choice: Option<Policy>,
    pub(crate) run: Run,
}

/// Which run of an operation the rule is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Run {
    First {
        /// When the run started, as a `FILETIME`; `None` while prepare
        /// prepares the selected entries, before anything is written.
        started: Option<i64>,
    },
    /// Continue after an interruption: a file already at the destination
    /// with the source's size and last write time is done, and a file there
    /// that is not done and was created at or after `started`, less
    /// [`FAT_TOLERANCE`], is unfinished.
    Continue {
        /// When the operation first ran, as a `FILETIME`; `None` for an
        /// operation saved before it was recorded, which finds no file
        /// unfinished.
        started: Option<i64>,
    },
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
    /// Its bytes are copied, so an interrupted run may have left it
    /// unfinished at the destination; a move by rename leaves nothing
    /// unfinished.
    pub(crate) copied: bool,
    /// Another selected entry has its name, or the name of the selected
    /// folder it is in. The run gives the plain and the numbered names in
    /// the order its workers meet the entries, so a file at its name that
    /// the run may have written may be the other entry's copy.
    pub(crate) shared: bool,
}

/// What prepare sees for one name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Clash {
    Free,
    /// A copy into the entry's own folder, which always gets a number.
    OwnFolder,
    /// A file, or a link, that the destination holds.
    Conflict,
    Skipped,
    /// Another selected entry already took the name.
    SameName,
    /// A folder into a folder of its name: they merge, as in File Explorer.
    Merge,
    /// Continue: the file is already at the destination, at `name`.
    Done {
        name: Vec<u16>,
    },
    /// Continue: the interrupted run was writing the file at `name`, which
    /// holds `size` bytes now.
    Unfinished {
        name: Vec<u16>,
        size: u64,
    },
    /// A file and a folder with one name, refused whatever the choice, as
    /// File Explorer refuses them.
    Mismatch,
}

pub(crate) fn classify(names: &Names, name: &[u16], wanted: Wanted, rule: Rule) -> Clash {
    let is_folder = wanted.kind != Kind::File;
    if let Run::Continue { started } = rule.run
        && wanted.kind == Kind::File
    {
        // Keep both, and a copy into its own folder, wrote a numbered name.
        let numbered = wanted.own_folder || rule.choice == Some(Policy::KeepBoth);
        let mut written = Vec::new();
        if !wanted.own_folder
            && let Some(holder) = names.get(name)
        {
            written.push((name.to_vec(), holder));
        }
        if numbered {
            written.extend(names.numbered(name, is_folder));
        }
        let files: Vec<(Vec<u16>, Found)> = (written.into_iter())
            .filter_map(|(name, holder)| match holder {
                Holder::Found(found) if found.kind == Kind::File => Some((name, found)),
                _ => None,
            })
            .collect();
        if let Some((name, _)) = files.iter().find(|&&(_, found)| is_done(found, wanted)) {
            return Clash::Done { name: name.clone() };
        }
        // Both engines set the source's times last, so the interrupted run's
        // file keeps the time it was created. A file the person had there
        // keeps its older creation time even when they edit it during the
        // interruption, which its last write time would not show. Neither
        // engine writes a reparse point for a file it copies, and writing
        // into one would write wherever it points.
        if let Some(started) = started
            && wanted.copied
            && !wanted.shared
            && let Some((name, found)) = (files.into_iter())
                .find(|(_, found)| !found.reparse_point && found.created >= started - FAT_TOLERANCE)
        {
            return Clash::Unfinished {
                name,
                size: found.size,
            };
        }
    }
    if wanted.own_folder {
        return Clash::OwnFolder;
    }
    // Whether this operation may have written `found`: at Continue any file,
    // and in a first run one created since it started, which a folder that
    // two selected folders merge into lists when the other one's walk has
    // already copied there.
    let maybe_ours = |found: Found| match rule.run {
        Run::Continue { .. } => true,
        Run::First { started } => {
            started.is_some_and(|started| found.created >= started - FAT_TOLERANCE)
        }
    };
    match names.get(name) {
        None => Clash::Free,
        Some(Holder::Ours) => Clash::SameName,
        Some(Holder::Found(found)) => match (wanted.kind, found.kind) {
            (Kind::Folder, Kind::Folder) => Clash::Merge,
            // The other selected entry's copy is never replaced.
            (wanted_kind, found_kind)
                if wanted_kind == found_kind && wanted.shared && maybe_ours(found) =>
            {
                Clash::SameName
            }
            (wanted_kind, found_kind) if wanted_kind == found_kind => {
                let policy = rule.choice.unwrap_or_default();
                if wanted_kind == Kind::File
                    && matches!(policy, Policy::UpdateChanged | Policy::UpdateNewer)
                    && !policy.replaces(
                        wanted.size,
                        wanted.last_write,
                        found.size,
                        found.last_write,
                    )
                {
                    Clash::Skipped
                } else {
                    Clash::Conflict
                }
            }
            _ => Clash::Mismatch,
        },
    }
}

/// What the run does with one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Write the entry under `name`, which this operation now holds;
    /// `replace` writes it over the file found there.
    Write {
        name: Vec<u16>,
        replace: bool,
    },
    /// Merge the folder into the folder of its name.
    Merge,
    Done,
    /// Resume the unfinished file at `name`, which this operation now
    /// holds.
    Resume {
        name: Vec<u16>,
    },
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
        Clash::SameName => match rule.choice.unwrap_or_default() {
            Policy::Replace | Policy::UpdateChanged | Policy::UpdateNewer | Policy::KeepBoth => {
                numbered(names)
            }
            Policy::Skip => Decision::Skip,
        },
        Clash::Skipped => {
            names.insert(name, Holder::Ours);
            Decision::Skip
        }
        Clash::Conflict => match rule.choice.unwrap_or_default() {
            Policy::Replace | Policy::UpdateChanged | Policy::UpdateNewer => {
                names.insert(name, Holder::Ours);
                Decision::Write {
                    name: name.to_vec(),
                    replace: true,
                }
            }
            Policy::KeepBoth => numbered(names),
            Policy::Skip => Decision::Skip,
        },
        Clash::Merge => Decision::Merge,
        // So no later entry of this name takes the copy for its own.
        Clash::Done { name } => {
            names.insert(&name, Holder::Ours);
            Decision::Done
        }
        Clash::Unfinished { name, .. } => {
            names.insert(&name, Holder::Ours);
            Decision::Resume { name }
        }
        Clash::Mismatch => Decision::Mismatch,
    }
}

/// Continue: whether the file that `holder` holds, which the saved Replace
/// writes over, was last written since the run started, to FAT's 2 seconds:
/// then it is the interrupted run's Replace, which the ring resumes in
/// place. The last write time serves here, unlike for an unfinished file,
/// because the person chose Replace for this file anyway, so a wrong guess
/// costs speed, never a file.
pub(crate) fn interrupted_replace(holder: Option<Holder>, rule: Rule) -> bool {
    match (holder, rule.run) {
        (
            Some(Holder::Found(found)),
            Run::Continue {
                started: Some(started),
            },
        ) => found.last_write >= started - FAT_TOLERANCE,
        _ => false,
    }
}

/// One second as a `FILETIME` difference.
const SECOND: i64 = 10_000_000;

/// FAT keeps times to 2 seconds.
const FAT_TOLERANCE: i64 = 2 * SECOND;

/// Continue's rule: the same size and a last write time within 2 seconds,
/// or within one hour and 2 seconds after a daylight saving change, because
/// FAT keeps times to 2 seconds in local time, as TinyTorrent2's
/// `Payload::IsCopy` compares them.
fn is_done(found: Found, wanted: Wanted) -> bool {
    let apart = found.last_write.abs_diff(wanted.last_write);
    let close = |to: u64| apart.abs_diff(to) <= FAT_TOLERANCE.unsigned_abs();
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
        copied: true,
        shared: false,
    };

    /// When the interrupted run of `continue_finds_what_is_done` started.
    const STARTED: i64 = 2_000_000_000;

    /// A file created and last written at `time`.
    fn found(kind: Kind, size: u64, time: i64) -> Holder {
        Holder::Found(Found {
            kind,
            size,
            created: time,
            last_write: time,
            reparse_point: false,
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
            (Some(Policy::Replace), write("C (2).TXT")),
            (Some(Policy::Skip), Decision::Skip),
            (None, write("C (2).TXT")),
        ] {
            let rule = Rule {
                choice,
                run: Run::First { started: None },
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
            run: Run::First { started: None },
        };
        let mut names = Names::default();
        names.insert(&utf16("d.txt"), found(Kind::File, 10, 0));
        assert_eq!(
            decide(&mut names, &utf16("d.txt"), own, rule),
            write("d (2).txt")
        );
    }

    #[test]
    fn the_default_updates_changed_files_without_replacing_metadata_matches() {
        let rule = Rule {
            choice: None,
            run: Run::First { started: None },
        };
        let mut names = Names::default();
        names.insert(&utf16("same.txt"), found(Kind::File, 10, FILE.last_write));
        names.insert(&utf16("size.txt"), found(Kind::File, 11, FILE.last_write));
        names.insert(
            &utf16("newer.txt"),
            found(Kind::File, 10, FILE.last_write + 1),
        );
        assert_eq!(
            decide(&mut names, &utf16("same.txt"), FILE, rule),
            Decision::Skip
        );
        assert_eq!(
            decide(&mut names, &utf16("size.txt"), FILE, rule),
            Decision::Write {
                name: utf16("size.txt"),
                replace: true,
            }
        );
        assert_eq!(
            decide(&mut names, &utf16("newer.txt"), FILE, rule),
            Decision::Write {
                name: utf16("newer.txt"),
                replace: true,
            }
        );
    }

    #[test]
    fn update_newer_never_replaces_an_equal_or_newer_destination() {
        let rule = Rule {
            choice: Some(Policy::UpdateNewer),
            run: Run::First { started: None },
        };
        let mut names = Names::default();
        names.insert(&utf16("equal.txt"), found(Kind::File, 99, FILE.last_write));
        names.insert(
            &utf16("newer.txt"),
            found(Kind::File, 99, FILE.last_write + 1),
        );
        names.insert(
            &utf16("older.txt"),
            found(Kind::File, 10, FILE.last_write - 1),
        );
        assert_eq!(
            decide(&mut names, &utf16("equal.txt"), FILE, rule),
            Decision::Skip
        );
        assert_eq!(
            decide(&mut names, &utf16("newer.txt"), FILE, rule),
            Decision::Skip
        );
        assert_eq!(
            decide(&mut names, &utf16("older.txt"), FILE, rule),
            Decision::Write {
                name: utf16("older.txt"),
                replace: true,
            }
        );
    }

    /// Continue takes a file of the source's size and time as done, at the
    /// name or, for Keep both, at a numbered name, and never takes a file
    /// that differs, nor the source itself in its own folder. A file that is
    /// not done and was created since the run started, to FAT's 2 seconds,
    /// is unfinished at the same names. An older file last written since is
    /// not, and neither is a reparse point, a file for a move by rename, or
    /// one for an operation saved without its start. An entry that shares
    /// its name with another selected entry takes no file for its own and
    /// never replaces one.
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
        let tolerance = 2 * 10_000_000;
        names.insert(
            &utf16("half.txt"),
            found(Kind::File, 4, STARTED - tolerance),
        );
        names.insert(
            &utf16("theirs.txt"),
            found(Kind::File, 4, STARTED - tolerance - 1),
        );
        names.insert(&utf16("theirs (2).txt"), found(Kind::File, 4, STARTED + 1));
        names.insert(
            &utf16("edited.txt"),
            Holder::Found(Found {
                kind: Kind::File,
                size: 4,
                created: STARTED - tolerance - 1,
                last_write: STARTED + 1,
                reparse_point: false,
            }),
        );
        names.insert(
            &utf16("link.txt"),
            Holder::Found(Found {
                kind: Kind::File,
                size: 0,
                created: STARTED,
                last_write: STARTED,
                reparse_point: true,
            }),
        );
        let rule = |choice| Rule {
            choice,
            run: Run::Continue {
                started: Some(STARTED),
            },
        };
        let clash =
            |name: &str, wanted, choice| classify(&names, &utf16(name), wanted, rule(choice));
        let done = |name: &str| Clash::Done { name: utf16(name) };
        assert_eq!(clash("same.txt", FILE, None), done("same.txt"));
        assert_eq!(clash("dst.txt", FILE, None), done("dst.txt"));
        assert_eq!(clash("other.txt", FILE, None), Clash::Conflict);
        assert_eq!(clash("late.txt", FILE, None), Clash::Conflict);
        assert_eq!(
            clash("kept.txt", FILE, Some(Policy::KeepBoth)),
            done("kept (2).txt")
        );
        assert_eq!(clash("kept.txt", FILE, Some(Policy::Skip)), Clash::Conflict);
        let own = Wanted {
            own_folder: true,
            ..FILE
        };
        assert_eq!(clash("same.txt", own, None), Clash::OwnFolder);
        assert_eq!(clash("kept.txt", own, None), done("kept (2).txt"));

        let unfinished = |name: &str| Clash::Unfinished {
            name: utf16(name),
            size: 4,
        };
        assert_eq!(clash("half.txt", FILE, None), unfinished("half.txt"));
        assert_eq!(clash("theirs.txt", FILE, None), Clash::Conflict);
        assert_eq!(clash("edited.txt", FILE, None), Clash::Conflict);
        assert_eq!(
            clash("theirs.txt", FILE, Some(Policy::KeepBoth)),
            unfinished("theirs (2).txt")
        );
        let renamed = Wanted {
            copied: false,
            ..FILE
        };
        assert_eq!(clash("half.txt", renamed, None), Clash::Conflict);
        assert_eq!(clash("link.txt", FILE, None), Clash::Conflict);
        let shared = Wanted {
            shared: true,
            ..FILE
        };
        assert_eq!(
            clash("half.txt", shared, Some(Policy::Replace)),
            Clash::SameName
        );
        // In a first run, only a file created since the run started may be
        // the other entry's copy.
        let first = Rule {
            choice: Some(Policy::Replace),
            run: Run::First {
                started: Some(STARTED),
            },
        };
        let first_clash = |name: &str| classify(&names, &utf16(name), shared, first);
        assert_eq!(first_clash("half.txt"), Clash::SameName);
        assert_eq!(first_clash("theirs.txt"), Clash::Conflict);
        let unsaved = Rule {
            choice: None,
            run: Run::Continue { started: None },
        };
        assert_eq!(
            classify(&names, &utf16("half.txt"), FILE, unsaved),
            Clash::Conflict
        );
    }
}
