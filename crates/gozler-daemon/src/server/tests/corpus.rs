//! The golden corpus: command lines run over one fixture index, each with what
//! gozler prints for it, as the code behaves today. A row runs its command line
//! through `parse_argv` into the request gozler sends, and that request through
//! [`encode_query_binary`], the daemon's query path; the rows printed, the
//! warnings and the exit code then come from [`Answer`], as in gozler.
//!
//! A row records today's behavior, right or wrong. A row that a planned fix
//! changes on purpose carries a comment with its number in the bug table of
//! `docs/plans/es-superset-specs.md`. The rows for output formats (`--json`,
//! `-export-csv`, columns) are in `gozler-cli`'s corpus, which prints a fixed
//! result set.

use super::{decode_frames, dir_record, set, volume};
use crate::server::encode_query_binary;
use crate::volume_state::VolumeSet;
use gozler_core::escompat::{Answer, Command, EsPlan, parse_argv};
use gozler_core::index::{FrnMap, NTFS_ROOT_FRN, VolumeIndex};
use gozler_core::layout::{LayoutFile, LayoutName};
use gozler_core::proto::{QueryRequest, Response};
use gozler_core::types::{Frn, VolumePhase};
use std::collections::HashMap;

/// One entry of the fixture, by full path.
enum Entry {
    Folder(&'static str),
    /// A file with its size in bytes and the day it was modified.
    File(&'static str, u64, i64),
    /// A file with an unknown size and date.
    Unread(&'static str),
    /// One file with several hard links, its size and the day it was modified.
    HardLinks(&'static [&'static str], u64, i64),
}
use Entry::{File, Folder, HardLinks, Unread};

const C_DRIVE: &[Entry] = &[
    Folder(r"C:\docs"),
    File(r"C:\docs\readme.txt", 100, 1),
    Folder(r"C:\Program Files"),
    File(r"C:\Program Files\app.exe", 2_000_000, 2),
    Folder(r"C:\Program Files (x86)"),
    Folder(r"C:\Program Files (x86)\Microsoft"),
    File(r"C:\Program Files (x86)\Microsoft\edge.manifest", 1024, 2),
    Folder(r"C:\repo"),
    File(r"C:\repo\DESIGN.md", 30_000, 5),
    File(r"C:\repo\report.txt", 4096, 4),
    File(r"C:\repo\Report 2026.txt", 4096, 6),
    File(r"C:\repo\notes.md.bak", 10, 1),
    Folder(r"C:\repo\docs"),
    File(r"C:\repo\docs\testing.md", 2048, 3),
    Folder(r"C:\repo\docs\agents"),
    File(r"C:\repo\docs\agents\testing.md", 512, 3),
    Folder(r"C:\repo\src"),
    File(r"C:\repo\src\argv.rs", 15_000, 7),
    File(r"C:\repo\src\main.cs", 20_480, 2),
    HardLinks(&[r"C:\repo\docs\link.md", r"C:\repo\src\link.md"], 300, 3),
    Folder(r"C:\repo\node_modules"),
    File(r"C:\repo\node_modules\lib.cs", 700, 1),
    Folder(r"C:\tmp"),
    Unread(r"C:\tmp\unread.bin"),
];

const D_DRIVE: &[Entry] = &[Folder(r"D:\backup"), File(r"D:\backup\report.txt", 5000, 8)];

/// What gozler prints for a command line.
#[derive(Debug, PartialEq)]
enum Printed {
    /// Full paths on stdout, one per line, in result order, then each warning
    /// on stderr, and the exit code.
    Paths {
        paths: Vec<String>,
        warnings: Vec<String>,
        exit: u8,
    },
    /// `parse_argv` refuses the command line: gozler prints the message and
    /// exits with the code.
    ArgvError(u8, String),
    /// The command line asks for the help: gozler prints it and exits 0.
    Help,
    /// The daemon refuses the query: gozler prints the message and exits 4.
    QueryError(String),
}

/// A complete answer: the paths, no warning and exit 0.
fn prints(paths: &[&str]) -> Printed {
    prints_then(paths, &[], 0)
}

fn prints_then(paths: &[&str], warnings: &[&str], exit: u8) -> Printed {
    Printed::Paths {
        paths: paths.iter().map(ToString::to_string).collect(),
        warnings: warnings.iter().map(ToString::to_string).collect(),
        exit,
    }
}

fn argv_error(code: u8, message: &str) -> Printed {
    Printed::ArgvError(code, message.to_string())
}

fn query_error(message: &str) -> Printed {
    Printed::QueryError(message.to_string())
}

fn rows() -> Vec<(&'static [&'static str], Printed)> {
    vec![
        // Name terms: case-insensitive substrings of the name, AND-ed.
        (
            &["report"],
            prints(&[
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
            ]),
        ),
        (&["report", "2026"], prints(&[r"C:\repo\Report 2026.txt"])),
        // One argument with a space splits into terms.
        (&["Report 2026"], prints(&[r"C:\repo\Report 2026.txt"])),
        (&[r#""report 2026""#], prints(&[r"C:\repo\Report 2026.txt"])),
        (&["-case", "Report"], prints(&[r"C:\repo\Report 2026.txt"])),
        (
            &["case:", "report"],
            prints(&[r"C:\repo\report.txt", r"D:\backup\report.txt"]),
        ),
        // Wildcards match the whole name.
        (
            &["*.md"],
            prints(&[
                r"C:\repo\DESIGN.md",
                r"C:\repo\docs\link.md",
                r"C:\repo\src\link.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        (
            &["rep?rt*"],
            prints(&[
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
            ]),
        ),
        // Path terms: substrings of the full path.
        (
            &[r"docs\agents"],
            prints(&[r"C:\repo\docs\agents", r"C:\repo\docs\agents\testing.md"]),
        ),
        (
            &[r"\docs"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\docs",
                r"C:\repo\docs",
                r"C:\repo\docs\link.md",
                r"C:\docs\readme.txt",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        // Bug 1.
        (
            &[r"C:\repo\docs\testing.md"],
            prints(&[r"C:\repo\docs\testing.md"]),
        ),
        (
            &[r"C:\repo\src\"],
            prints(&[
                r"C:\repo\src\argv.rs",
                r"C:\repo\src\link.md",
                r"C:\repo\src\main.cs",
            ]),
        ),
        (&[r"C:\repo\notes.md"], prints(&[r"C:\repo\notes.md.bak"])),
        (
            &["path:agents"],
            prints(&[r"C:\repo\docs\agents", r"C:\repo\docs\agents\testing.md"]),
        ),
        // Path wildcards match the whole full path.
        (
            &[r"C:\repo\*.cs"],
            prints(&[r"C:\repo\node_modules\lib.cs", r"C:\repo\src\main.cs"]),
        ),
        (
            &["path:*.cs"],
            prints(&[r"C:\repo\node_modules\lib.cs", r"C:\repo\src\main.cs"]),
        ),
        // Bug 11.
        (
            &[r"*\docs\*"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\repo\docs\link.md",
                r"C:\docs\readme.txt",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        // Bug 11.
        (
            &[r"!*\node_modules\*"],
            query_error(
                r#"operator '!' is not supported yet; quote it to search for it literally (e.g. "a!b")"#,
            ),
        ),
        // Bug 9.
        (&["docs/agents/testing"], prints(&[])),
        // Bug 9.
        (&["path:docs/agents"], prints(&[])),
        // Bug 9.
        (&["C:/repo/docs/testing.md"], prints(&[])),
        // Bug 10.
        (&[r#"path:"C:\Program Files""#], prints(&[])),
        // Bug 10.
        (&[r#""C:\Program Files (x86)\Microsoft""#], prints(&[])),
        // Bug 10: a query in one argument splits into terms.
        (&["ext:rs size:>1kb"], prints(&[r"C:\repo\src\argv.rs"])),
        // Bug 3: fixed, /a-d is file: and /ad is folder:.
        (
            &["/a-d", "report"],
            prints(&[
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
            ]),
        ),
        (
            &["/a-d", "b"],
            prints(&[
                r"C:\repo\node_modules\lib.cs",
                r"C:\repo\notes.md.bak",
                r"C:\tmp\unread.bin",
            ]),
        ),
        (&["/ad", "b"], prints(&[r"D:\backup"])),
        (
            &["/aH", "report"],
            argv_error(
                4,
                "/aH needs the attribute column, which this release does not have",
            ),
        ),
        // Operators.
        (
            &["report", "!2026"],
            query_error(
                r#"operator '!' is not supported yet; quote it to search for it literally (e.g. "a!b")"#,
            ),
        ),
        (
            &["report|testing"],
            query_error(
                r#"operator '|' is not supported yet; quote it to search for it literally (e.g. "a|b")"#,
            ),
        ),
        (&["a<b"], prints(&[])),
        // Functions.
        (
            &["ext:CS;.txt"],
            prints(&[
                r"C:\repo\node_modules\lib.cs",
                r"C:\repo\src\main.cs",
                r"C:\docs\readme.txt",
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
            ]),
        ),
        (&["size:>1mb"], prints(&[r"C:\Program Files\app.exe"])),
        (
            &["size:..500"],
            prints(&[
                r"C:\repo\docs\link.md",
                r"C:\repo\src\link.md",
                r"C:\repo\notes.md.bak",
                r"C:\docs\readme.txt",
            ]),
        ),
        (&["size:abc"], query_error("malformed size:: abc")),
        // Bug 12 shows as this difference: `file:` finds a file with an
        // unknown size, and `size:0..` does not.
        (
            &["-path", r"C:\tmp", "file:"],
            prints(&[r"C:\tmp\unread.bin"]),
        ),
        (&["-path", r"C:\tmp", "size:0.."], prints(&[])),
        (
            &["folder:"],
            prints(&[
                r"C:\repo\docs\agents",
                r"D:\backup",
                r"C:\docs",
                r"C:\repo\docs",
                r"C:\Program Files (x86)\Microsoft",
                r"C:\repo\node_modules",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
                r"C:\repo",
                r"C:\repo\src",
                r"C:\tmp",
            ]),
        ),
        (
            &["file:report"],
            prints(&[
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
            ]),
        ),
        (&["dir:docs"], prints(&[r"C:\docs", r"C:\repo\docs"])),
        // Bug 2.
        (&["dm:today"], prints(&[])),
        // Scope: -path searches below a folder, never the folder itself.
        (&["-path", r"C:\tmp"], prints(&[r"C:\tmp\unread.bin"])),
        (
            &["-path", r"C:\repo", "testing"],
            prints(&[
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        (
            &["-path", r"C:\repo\docs\", "testing"],
            prints(&[
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        (
            &["-path", r"c:\REPO\SRC", "link"],
            prints(&[r"C:\repo\src\link.md"]),
        ),
        (
            &["-path", r"D:\", "report"],
            prints(&[r"D:\backup\report.txt"]),
        ),
        // A scope on no indexed volume is refused, not answered with nothing.
        (
            &["-path", r"E:\", "report"],
            query_error(r"E:\ is not on an indexed volume; gozlerd indexes fixed NTFS volumes"),
        ),
        (&["-case", "-path", r"C:\Repo", "testing"], prints(&[])),
        // Sort and limit.
        (
            &["-sort-path", "*.md"],
            prints(&[
                r"C:\repo\DESIGN.md",
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
                r"C:\repo\src\link.md",
            ]),
        ),
        (
            &["-sort", "size", "*.md"],
            prints(&[
                r"C:\repo\DESIGN.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
                r"C:\repo\docs\link.md",
                r"C:\repo\src\link.md",
            ]),
        ),
        // es's /os sorts smallest first.
        (
            &["/os", "*.md"],
            prints(&[
                r"C:\repo\docs\link.md",
                r"C:\repo\src\link.md",
                r"C:\repo\docs\agents\testing.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\DESIGN.md",
            ]),
        ),
        // Unknown sizes, a folder's included, sort last.
        (
            &["-sort-size-ascending", "b"],
            prints(&[
                r"C:\repo\notes.md.bak",
                r"C:\repo\node_modules\lib.cs",
                r"D:\backup",
                r"C:\tmp\unread.bin",
            ]),
        ),
        (
            &["-sort", "dm", "report"],
            prints(&[
                r"D:\backup\report.txt",
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
            ]),
        ),
        (
            &["-sort-name-descending", "report"],
            prints(&[
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
                r"C:\repo\Report 2026.txt",
            ]),
        ),
        // Bug 4.
        (
            &["report", "-n", "1"],
            prints_then(
                &[r"C:\repo\Report 2026.txt"],
                &["showing 1 of 3 matches; narrow the query or pass -n all"],
                10,
            ),
        ),
        // Bug 4.
        (
            &["-n", "0", "report"],
            prints_then(
                &[],
                &["showing 0 of 3 matches; narrow the query or pass -n all"],
                10,
            ),
        ),
        // The default limit: 20 rows when more match.
        (
            &[],
            prints_then(
                &[
                    r"C:\repo\docs\agents",
                    r"C:\Program Files\app.exe",
                    r"C:\repo\src\argv.rs",
                    r"D:\backup",
                    r"C:\repo\DESIGN.md",
                    r"C:\docs",
                    r"C:\repo\docs",
                    r"C:\Program Files (x86)\Microsoft\edge.manifest",
                    r"C:\repo\node_modules\lib.cs",
                    r"C:\repo\docs\link.md",
                    r"C:\repo\src\link.md",
                    r"C:\repo\src\main.cs",
                    r"C:\Program Files (x86)\Microsoft",
                    r"C:\repo\node_modules",
                    r"C:\repo\notes.md.bak",
                    r"C:\Program Files",
                    r"C:\Program Files (x86)",
                    r"C:\docs\readme.txt",
                    r"C:\repo",
                    r"C:\repo\Report 2026.txt",
                ],
                &["showing 20 of 27 matches; narrow the query or pass -n all"],
                10,
            ),
        ),
        (&["zzzzqqq", "-no-result-error"], prints_then(&[], &[], 9)),
        // Bug 7.
        (&["-help"], Printed::Help),
        // Command lines parse_argv refuses.
        (&["--bogus"], argv_error(6, "unknown switch: --bogus")),
        (
            &["-n", "abc"],
            argv_error(4, "expected a number or all after -n, got: abc"),
        ),
        (
            &["-path"],
            argv_error(4, "expected an additional parameter after -path"),
        ),
        (
            &["-sort", "bogus"],
            argv_error(4, "expected a sort key after -sort, got: bogus"),
        ),
    ]
}

/// Ticks of a Windows FILETIME in one day.
const TICKS_PER_DAY: i64 = 864_000_000_000;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;

/// The index of the volume mounted at `mount`, built from `entries` in order,
/// so a folder comes before what it holds.
fn index(mount: &str, entries: &'static [Entry]) -> VolumeIndex {
    let mut index = VolumeIndex::new(NTFS_ROOT_FRN, FrnMap::sparse());
    let mut folders: HashMap<&str, Frn> = HashMap::new();
    for (frn, entry) in (100..).map(Frn).zip(entries) {
        let (links, size, day) = match entry {
            Folder(path) => {
                let (parent, name) = place(mount, &folders, path);
                index.insert_enum(&dir_record(frn.0, parent.0, name));
                folders.insert(&path[mount.len()..], frn);
                continue;
            }
            File(path, size, day) => (std::slice::from_ref(path), Some(*size), Some(*day)),
            Unread(path) => (std::slice::from_ref(path), None, None),
            HardLinks(paths, size, day) => (*paths, Some(*size), Some(*day)),
        };
        let names = links
            .iter()
            .map(|path| {
                let (parent_frn, name) = place(mount, &folders, path);
                LayoutName {
                    parent_frn,
                    name: name.as_bytes().to_vec(),
                    name_lossy: false,
                    dos_only: false,
                }
            })
            .collect();
        index.enrich(&LayoutFile {
            frn,
            attributes: FILE_ATTRIBUTE_ARCHIVE,
            size,
            mtime_ft: day.map(|d| d * TICKS_PER_DAY),
            names,
        });
    }
    index
}

/// The folder on the volume mounted at `mount` that holds `path`, and the
/// name in it.
fn place<'p>(mount: &str, folders: &HashMap<&str, Frn>, path: &'p str) -> (Frn, &'p str) {
    let rel = path.strip_prefix(mount).expect("entry is on this volume");
    match rel.rsplit_once('\\') {
        Some((folder, name)) => (folders[folder], name),
        None => (NTFS_ROOT_FRN, rel),
    }
}

/// The request gozler sends for `plan`, as gozler-cli's `build_query` builds it.
/// A `-path` value goes as given, so rows give it absolute; gozler resolves a
/// relative one against its working directory.
fn request(plan: &EsPlan) -> QueryRequest {
    QueryRequest {
        query: plan.query.clone(),
        scope: plan.scope.clone(),
        sort: plan.sort,
        offset: 0,
        limit: plan.row_limit(),
        want_size: true,
        want_mtime: true,
        match_case: plan.match_case,
    }
}

fn print(volumes: &VolumeSet, args: &[&str]) -> Printed {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let plan = match parse_argv(&args) {
        Ok(Command::Run(plan)) => plan,
        Ok(Command::Help) => return Printed::Help,
        Ok(Command::Version) => panic!("{args:?}: no row asks for the version"),
        Err(fatal) => return Printed::ArgvError(fatal.exit_code.code(), fatal.message),
    };
    let mut paths = Vec::new();
    let mut answer = Answer::default();
    for frame in decode_frames(&encode_query_binary(volumes, request(&plan))) {
        match frame {
            Response::Results(page) => {
                answer.add_page(&page);
                paths.extend(page.items.into_iter().map(|i| i.path));
            }
            Response::Error { message, .. } => return Printed::QueryError(message),
            other => panic!("unexpected reply to a query: {other:?}"),
        }
    }
    Printed::Paths {
        paths,
        warnings: answer.warnings().iter().map(ToString::to_string).collect(),
        exit: answer.exit_code(plan.no_result_error).code(),
    }
}

/// Every row prints what it recorded. All changed rows are listed at once, so
/// a change shows every row it moves.
#[test]
fn every_row_prints_what_it_recorded() {
    let volumes = set(&[
        volume(r"C:\", index(r"C:\", C_DRIVE), VolumePhase::Live),
        volume(r"D:\", index(r"D:\", D_DRIVE), VolumePhase::Live),
    ]);
    let mut changed = Vec::new();
    for (args, recorded) in rows() {
        let printed = print(&volumes, args);
        if printed != recorded {
            changed.push(format!(
                "{args:?}\n  recorded: {recorded:?}\n  printed:  {printed:?}"
            ));
        }
    }
    assert!(
        changed.is_empty(),
        "{} row(s) changed:\n{}",
        changed.len(),
        changed.join("\n")
    );
}
