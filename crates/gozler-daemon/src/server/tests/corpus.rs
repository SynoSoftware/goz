//! The golden corpus: command lines run over one fixture index, each with what
//! gozler prints for it, as the code behaves today. A row runs its command line
//! through `parse_argv` and [`build_search`] into the request gozler sends, and
//! that request through [`serve`](crate::server::serve), the daemon's request
//! path; the rows printed, the warnings and the exit code then come from
//! [`Answer`], as in gozler. One row also runs as protocol 1's text query,
//! which the daemon parses itself.
//!
//! A row records today's behavior, right or wrong. A row that a fix changed
//! on purpose carries a comment with its bug number from the "Golden corpus"
//! section of `docs/DESIGN.md`. The rows for output formats (`--json`,
//! `-export-csv`, columns) are in `gozler-cli`'s corpus, which prints a fixed
//! result set.

use super::{decode_frames, dir_record, reply_bytes, set, volume};
use crate::volume_state::VolumeSet;
use gozler_core::escompat::{Answer, Command, Count, EsPlan, build_search, parse_argv};
use gozler_core::index::{NTFS_ROOT_FRN, VolumeIndex};
use gozler_core::layout::{LayoutFile, LayoutName};
use gozler_core::proto::{ProtoError, QueryRequest, Request, Response, SearchRequest};
use gozler_core::query::{QueryError, QueryStart};
use gozler_core::types::{Frn, VolumePhase};
use std::collections::HashMap;

/// One entry of the fixture, by full path.
enum Entry {
    Folder(&'static str),
    /// A file with its size in bytes and the day it was modified.
    File(&'static str, u64, i64),
    /// A hidden file with its size in bytes and the day it was modified.
    Hidden(&'static str, u64, i64),
    /// A file with an unknown size and date.
    Unread(&'static str),
    /// One file with several hard links, its size and the day it was modified.
    HardLinks(&'static [&'static str], u64, i64),
}
use Entry::{File, Folder, HardLinks, Hidden, Unread};

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
    Hidden(r"C:\repo\notes.md.bak", 10, 1),
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
    /// gozler or the daemon refuses the query: gozler prints the message and
    /// exits 4.
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
        // Bug 11, and `!`, which was refused before stage 5: every entry
        // but the one below node_modules, 26 of 27.
        (
            &[r"!*\node_modules\*"],
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
                    r"C:\repo\report.txt",
                ],
                &["showing 20 of 26 matches; narrow the query or pass -n all"],
                10,
            ),
        ),
        // Bug 9: fixed, `/` is a separator, as `\` is.
        (
            &["docs/agents/testing"],
            prints(&[r"C:\repo\docs\agents\testing.md"]),
        ),
        // Bug 9: fixed.
        (
            &["path:docs/agents"],
            prints(&[r"C:\repo\docs\agents", r"C:\repo\docs\agents\testing.md"]),
        ),
        // Bug 9: fixed.
        (
            &["C:/repo/docs/testing.md"],
            prints(&[r"C:\repo\docs\testing.md"]),
        ),
        // An unknown `/` token is search text, as in es, and now a path term.
        (
            &["/docs"],
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
        // A path term that starts with `.\` or `..\` is below the working
        // directory, `WORKING_DIR`.
        (
            &[r".\"],
            prints(&[
                r"C:\repo\src\argv.rs",
                r"C:\repo\src\link.md",
                r"C:\repo\src\main.cs",
            ]),
        ),
        (
            &["../docs/*.md"],
            prints(&[
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        // -p matches text terms against the full path, and nopath: the name.
        (&["src", "rs"], prints(&[])),
        (&["-p", "src", "rs"], prints(&[r"C:\repo\src\argv.rs"])),
        (
            &["-p", "src", "nopath:link"],
            prints(&[r"C:\repo\src\link.md"]),
        ),
        // Whole words: no letter or digit right before or after the text.
        // `read` lies inside readme and unread.
        (&["ww:read"], prints(&[])),
        (
            &["-ww", "md"],
            prints(&[
                r"C:\repo\DESIGN.md",
                r"C:\repo\docs\link.md",
                r"C:\repo\src\link.md",
                r"C:\repo\notes.md.bak",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        (&["-ww", r"docs\agent"], prints(&[])),
        // Regular expressions match the name, in the query's case; their `|`
        // belongs to them, as in es. With `\\` they match the full path.
        (
            &[r"regex:^(argv|main)\."],
            prints(&[r"C:\repo\src\argv.rs", r"C:\repo\src\main.cs"]),
        ),
        (&["-i", "regex:^R"], prints(&[r"C:\repo\Report 2026.txt"])),
        (
            &["-r", r"^C:\\repo\\docs\\[^\\]+$"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
            ]),
        ),
        (
            &["regex:("],
            query_error(
                "regular expression: regex parse error:\n    (\n    ^\nerror: unclosed group",
            ),
        ),
        // parent: and -parent find the entries directly in a folder;
        // -parent-path searches below the folder that holds a path.
        (
            &[r"parent:C:\repo\docs"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
            ]),
        ),
        (
            &["parent:.."],
            prints(&[
                r"C:\repo\DESIGN.md",
                r"C:\repo\docs",
                r"C:\repo\node_modules",
                r"C:\repo\notes.md.bak",
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"C:\repo\src",
            ]),
        ),
        (
            &["-parent", r"..\docs"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
            ]),
        ),
        (
            &["-parent-path", "C:/repo/docs/testing.md"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        // Bug 10: fixed, quotes delimit a function's value, which matches as
        // a substring, so `Program Files (x86)` matches too.
        (
            &[r#"path:"C:\Program Files""#],
            prints(&[
                r"C:\Program Files\app.exe",
                r"C:\Program Files (x86)\Microsoft\edge.manifest",
                r"C:\Program Files (x86)\Microsoft",
                r"C:\Program Files",
                r"C:\Program Files (x86)",
            ]),
        ),
        // Bug 10: fixed, a quoted term with a separator is a path term.
        (
            &[r#""C:\Program Files (x86)\Microsoft""#],
            prints(&[
                r"C:\Program Files (x86)\Microsoft\edge.manifest",
                r"C:\Program Files (x86)\Microsoft",
            ]),
        ),
        // Bug 10: a wildcard in a quoted path term is still a wildcard.
        (
            &[r#""C:\Program Files\*.exe""#],
            prints(&[r"C:\Program Files\app.exe"]),
        ),
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
        // Stage 9 adds the other attribute letters, which exited 4 before.
        (&["/aH"], prints(&[r"C:\repo\notes.md.bak"])),
        (
            &["/a-h", "b"],
            prints(&[
                r"D:\backup",
                r"C:\repo\node_modules\lib.cs",
                r"C:\tmp\unread.bin",
            ]),
        ),
        // Each excluded letter leaves out what has it, as in dir.
        (&["/ah-s"], prints(&[r"C:\repo\notes.md.bak"])),
        (&["/ah-a"], prints(&[])),
        (&["attrib:ha"], prints(&[r"C:\repo\notes.md.bak"])),
        (
            &["!attrib:a", "-path", r"C:\repo\docs"],
            prints(&[r"C:\repo\docs\agents"]),
        ),
        (
            &["/ab", "report"],
            argv_error(
                4,
                "/ab is not an attribute option: write /a and attribute letters, with - \
                 before a letter to leave that attribute out, such as /ah or /a-h; gozler \
                 --help lists the letters",
            ),
        ),
        (
            &["attrib:0x2"],
            query_error(
                "attrib: 0 is not an attribute letter; the letters are \
                 RHSDAVXNTLCOIEUPM",
            ),
        ),
        // Operators. `!` and `|` were refused before stage 5.
        (
            &["report", "!2026"],
            prints(&[r"C:\repo\report.txt", r"D:\backup\report.txt"]),
        ),
        (
            &["report|testing"],
            prints(&[
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
                r"D:\backup\report.txt",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        // `|` binds tighter than the space, as in es.
        (&["DESIGN|argv rs"], prints(&[r"C:\repo\src\argv.rs"])),
        (
            &["DESIGN", "|", "argv", "rs"],
            prints(&[r"C:\repo\src\argv.rs"]),
        ),
        // An unquoted `<` was searched as text before stage 5, which no
        // Windows name contains.
        (
            &["a<b"],
            query_error("'<' is not supported: gozler does not group terms with < and >"),
        ),
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
        (
            &["size:abc"],
            query_error(
                "size: abc is not a size; write a number with kb, mb, gb or tb, such as 10mb",
            ),
        ),
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
        // Bug 2: stage 8 adds `dm:`. Each day of the fixture is a day after
        // 1601-01-01, and every row starts at noon UTC on day 7, 1601-01-08.
        (&["dm:today"], prints(&[r"C:\repo\src\argv.rs"])),
        // The last 3 days end at the query start, so day 8 is in the future
        // and left out.
        (
            &["dm:3days"],
            prints(&[
                r"C:\repo\src\argv.rs",
                r"C:\repo\DESIGN.md",
                r"C:\repo\Report 2026.txt",
            ]),
        ),
        (&["dm:>today"], prints(&[r"D:\backup\report.txt"])),
        (
            &["dm:1601-01-02..1601-01-03"],
            prints(&[
                r"C:\Program Files\app.exe",
                r"C:\Program Files (x86)\Microsoft\edge.manifest",
                r"C:\repo\node_modules\lib.cs",
                r"C:\repo\src\main.cs",
                r"C:\repo\notes.md.bak",
                r"C:\docs\readme.txt",
            ]),
        ),
        // Folders and the unread file have no known date, so the negation
        // does not report them as recent.
        (
            &["!dm:<1601-01-08"],
            prints(&[r"C:\repo\src\argv.rs", r"D:\backup\report.txt"]),
        ),
        (
            &["dm:1601-01-03..1601-01-02"],
            query_error(
                "dm: 1601-01-03..1601-01-02 is a reversed range; write the \
                 earlier date first",
            ),
        ),
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
        // A relative scope is below the working directory, `WORKING_DIR`.
        (&["-path", ".", "link"], prints(&[r"C:\repo\src\link.md"])),
        (
            &["-path", r"..\docs", "testing"],
            prints(&[
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
            ]),
        ),
        (
            &["-path", r"D:\", "report"],
            prints(&[r"D:\backup\report.txt"]),
        ),
        // A scope on no indexed volume is refused, not answered with nothing.
        (
            &["-path", r"E:\", "report"],
            query_error(
                r"E:\ is not on an indexed volume; gozlerd indexes fixed NTFS and ReFS volumes",
            ),
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
        // Stage 8 adds /oe: no extension first, then by extension, and ties
        // by name.
        (
            &["/oe", "-path", r"C:\repo"],
            prints(&[
                r"C:\repo\docs\agents",
                r"C:\repo\docs",
                r"C:\repo\node_modules",
                r"C:\repo\src",
                r"C:\repo\notes.md.bak",
                r"C:\repo\node_modules\lib.cs",
                r"C:\repo\src\main.cs",
                r"C:\repo\DESIGN.md",
                r"C:\repo\docs\link.md",
                r"C:\repo\src\link.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
                r"C:\repo\src\argv.rs",
                r"C:\repo\Report 2026.txt",
                r"C:\repo\report.txt",
            ]),
        ),
        // Stage 8 adds /o-e, which reverses the extensions, not the ties.
        (
            &["/o-e", "-path", r"C:\repo\docs"],
            prints(&[
                r"C:\repo\docs\link.md",
                r"C:\repo\docs\testing.md",
                r"C:\repo\docs\agents\testing.md",
                r"C:\repo\docs\agents",
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
        // The default limit: 20 rows when more match. Bare gozler prints the
        // overview, so the default sort stands in for an empty command line.
        (
            &["-sort", "name"],
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
        (&["--bogus"], argv_error(6, "unknown option: --bogus")),
        (
            &["-n", "abc"],
            argv_error(4, "-n needs a number or all, not abc"),
        ),
        (&["-path"], argv_error(4, "-path needs DIR after it")),
        (
            &["-sort", "bogus"],
            argv_error(
                4,
                "-sort needs a sort key such as name, size or dm, not bogus",
            ),
        ),
    ]
}

/// Ticks of a Windows FILETIME in one day.
const TICKS_PER_DAY: i64 = 864_000_000_000;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;

/// The index of the volume mounted at `mount`, built from `entries` in order,
/// so a folder comes before what it holds.
fn index(mount: &str, entries: &'static [Entry]) -> VolumeIndex {
    let mut index = VolumeIndex::new(NTFS_ROOT_FRN);
    let mut folders: HashMap<&str, Frn> = HashMap::new();
    for (frn, entry) in (100..).map(Frn).zip(entries) {
        let (links, size, day, attributes) = match entry {
            Folder(path) => {
                let (parent, name) = place(mount, &folders, path);
                index.insert_enum(&dir_record(frn.0, parent.0, name));
                folders.insert(&path[mount.len()..], frn);
                continue;
            }
            File(path, size, day) => (
                std::slice::from_ref(path),
                Some(*size),
                Some(*day),
                FILE_ATTRIBUTE_ARCHIVE,
            ),
            Hidden(path, size, day) => (
                std::slice::from_ref(path),
                Some(*size),
                Some(*day),
                FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_ARCHIVE,
            ),
            Unread(path) => (
                std::slice::from_ref(path),
                None,
                None,
                FILE_ATTRIBUTE_ARCHIVE,
            ),
            HardLinks(paths, size, day) => {
                (*paths, Some(*size), Some(*day), FILE_ATTRIBUTE_ARCHIVE)
            }
        };
        let names = links
            .iter()
            .map(|path| {
                let (parent_frn, name) = place(mount, &folders, path);
                LayoutName {
                    parent_frn,
                    name: name.as_bytes().to_vec(),
                    dos_only: false,
                }
            })
            .collect();
        index.enrich(&LayoutFile {
            frn,
            attributes,
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

/// The working directory every row runs in.
const WORKING_DIR: &str = r"C:\repo\src";

/// When every row starts: noon UTC on the fixture's day 7, so `dm:` rows do
/// not depend on the machine's clock or time zone.
fn query_start() -> QueryStart {
    QueryStart {
        now: "1601-01-08T12:00:00Z".parse().expect("an RFC 3339 instant"),
        time_zone: jiff::tz::TimeZone::UTC,
    }
}

/// The request gozler sends for `plan`.
fn search(plan: &EsPlan) -> Result<SearchRequest, QueryError> {
    build_search(plan, WORKING_DIR, &query_start())
}

/// The two ways a client sends a search.
#[derive(Clone, Copy)]
enum Protocol {
    /// From protocol 2: the query parsed by gozler.
    Parsed,
    /// Protocol 1: the query text, which the daemon parses.
    Text,
}

fn print(volumes: &VolumeSet, args: &[&str], protocol: Protocol) -> Printed {
    let args: Vec<String> = args.iter().map(ToString::to_string).collect();
    let plan = match parse_argv(&args) {
        Ok(Command::Run(plan)) => plan,
        Ok(Command::Help(_)) => return Printed::Help,
        Ok(Command::Version) => panic!("{args:?}: no row asks for the version"),
        Ok(Command::Overview) => panic!("no row runs gozler without arguments"),
        Err(fatal) => return Printed::ArgvError(fatal.exit_code.code(), fatal.message),
    };
    let search = match search(&plan) {
        Ok(search) => search,
        Err(e) => return Printed::QueryError(e.to_string()),
    };
    let mut answer = Answer::new(&plan, &search.query);
    let request = match protocol {
        Protocol::Parsed => Request::Search(search),
        Protocol::Text => Request::Query(text_request(&plan, search.scope)),
    };
    let mut paths = Vec::new();
    for frame in decode_frames(&reply_bytes(volumes, &request)) {
        match frame {
            Response::Results(page) => {
                answer.add_page(&page);
                paths.extend(page.items.into_iter().map(|i| i.path));
            }
            // A count prints its one number instead of the rows.
            Response::Totals(totals) => {
                answer.add_totals(&totals);
                let value = match plan.count {
                    Some(Count::TotalSize) => totals.size,
                    _ => totals.count,
                };
                paths.push(value.to_string());
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

/// The request protocol 1's client sends for `plan`, with `scope` made
/// absolute as gozler makes it.
fn text_request(plan: &EsPlan, scope: Option<String>) -> QueryRequest {
    QueryRequest {
        query: plan.query.clone(),
        scope,
        sort: plan.sort,
        offset: 0,
        limit: plan.row_limit(),
        want_size: true,
        want_mtime: true,
        match_case: plan.match_case,
    }
}

fn fixture() -> VolumeSet {
    set(&[
        volume(r"C:\", index(r"C:\", C_DRIVE), VolumePhase::Live),
        volume(r"D:\", index(r"D:\", D_DRIVE), VolumePhase::Live),
    ])
}

/// Every row prints what it recorded. All changed rows are listed at once, so
/// a change shows every row it moves.
#[test]
fn every_row_prints_what_it_recorded() {
    let volumes = fixture();
    let mut changed = Vec::new();
    for (args, recorded) in rows() {
        let printed = print(&volumes, args, Protocol::Parsed);
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

/// The daemon still parses the text of a protocol 1 query, with the parser
/// gozler runs, so the row prints the same through either protocol.
#[test]
fn the_text_path_prints_what_its_row_recorded() {
    let args: &[&str] = &["report", "-n", "1"];
    let (_, recorded) = rows()
        .into_iter()
        .find(|(row, _)| *row == args)
        .expect("the row exists");
    assert_eq!(print(&fixture(), args, Protocol::Text), recorded);
}

/// The daemon resolves a `dm:` in a protocol 1 query with its own clock and
/// time zone, instead of refusing it for want of the client's. Every date in
/// the fixture is before 9999 in any time zone, so the row does not depend on
/// the machine.
#[test]
fn the_text_path_resolves_dates_with_the_daemons_clock() {
    assert_eq!(
        print(&fixture(), &["report", "dm:<9999-01-01"], Protocol::Text),
        prints(&[
            r"C:\repo\Report 2026.txt",
            r"C:\repo\report.txt",
            r"D:\backup\report.txt",
        ])
    );
}

/// The totals of a query, which the daemon sums so that no row crosses the
/// pipe, against the count and the sizes of every row of the same query. The
/// fixture holds folders, a file whose size is unknown and one file under
/// two names.
#[test]
fn a_total_counts_and_adds_up_every_row_of_the_same_query() {
    let volumes = fixture();
    let queries: [&[&str]; 5] = [
        &[],
        &["report"],
        &["*.md"],
        &["-path", ".."],
        &["-path", r"..\..\tmp"],
    ];
    for args in queries {
        let search = |extra: &[&str]| {
            let args: Vec<String> = args.iter().chain(extra).map(ToString::to_string).collect();
            let Ok(Command::Run(plan)) = parse_argv(&args) else {
                panic!("{args:?} is a search");
            };
            Request::Search(search(&plan).expect("parses"))
        };
        let rows: Vec<_> = decode_frames(&reply_bytes(&volumes, &search(&["-n", "all"])))
            .into_iter()
            .flat_map(|frame| match frame {
                Response::Results(page) => page.items,
                other => panic!("{args:?}: {other:?}"),
            })
            .collect();
        let size: u64 = rows
            .iter()
            .filter(|row| !row.is_dir)
            .filter_map(|row| row.size)
            .sum();
        match decode_frames(&reply_bytes(&volumes, &search(&["-get-total-size"])))[..] {
            [Response::Totals(ref totals)] => {
                assert_eq!(
                    (totals.count, totals.size),
                    (rows.len() as u64, size),
                    "{args:?}"
                )
            }
            ref other => panic!("{args:?}: {other:?}"),
        }
    }
}

/// A total that passes `u64::MAX`, which only a corrupt size in the index can
/// cause, is refused rather than wrapped to a small number that looks valid.
#[test]
fn a_total_too_large_for_64_bits_is_refused() {
    const HUGE: &[Entry] = &[File(r"C:\a.bin", u64::MAX - 1, 1), File(r"C:\b.bin", 2, 1)];
    let volumes = set(&[volume(r"C:\", index(r"C:\", HUGE), VolumePhase::Live)]);
    let Ok(Command::Run(plan)) = parse_argv(&["-get-total-size".to_string()]) else {
        panic!("a search");
    };
    let request = Request::Search(search(&plan).expect("parses"));
    assert!(matches!(
        decode_frames(&reply_bytes(&volumes, &request))[..],
        [Response::Error {
            code: ProtoError::Overflow,
            ..
        }]
    ));
}

/// A size or date test never matches a file whose size or date is unknown,
/// and a size total adds nothing for it, so when the size and date pass
/// failed or stopped on a volume in scope, such a query warns and exits 2,
/// where its answer would otherwise look complete. A query that reads no
/// size or date, a count included, is unaffected.
#[test]
fn a_size_or_date_query_on_a_volume_with_unknown_sizes_exits_2() {
    let volumes = fixture();
    *volumes[0].metadata_pending.write() = true;
    let warning = "results may be incomplete: gozler could not read the sizes and dates on a \
                   volume in scope, so size and date tests and size totals leave those files \
                   out";
    let outcome = |args: &[&str]| match print(&volumes, args, Protocol::Parsed) {
        Printed::Paths { warnings, exit, .. } => (warnings, exit),
        other => panic!("{args:?}: {other:?}"),
    };
    for args in [
        &["size:>1kb", "-n", "all"][..],
        &["dm:today"],
        &["-get-total-size"],
    ] {
        assert_eq!(outcome(args), (vec![warning.to_string()], 2), "{args:?}");
    }
    for args in [&["report"][..], &["-get-result-count"]] {
        assert_eq!(outcome(args), (vec![], 0), "{args:?}");
    }
}
