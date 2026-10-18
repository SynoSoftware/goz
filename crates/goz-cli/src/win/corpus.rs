//! The golden corpus for output: command lines run through `parse_argv` and
//! [`emit`] over one fixed result set, with the exact text gozler writes to
//! stdout or to the `-export-csv` file, as the code behaves today. Dates render
//! in UTC, so every row passes in any time zone.
//!
//! A row records today's behavior, right or wrong. A row that a planned fix
//! changes on purpose carries a comment with its number in the bug table of
//! `docs/plans/es-superset-specs.md`. The rows for what a command line matches
//! are in the daemon's corpus, in `goz-daemon`'s `server` tests.

use super::emit;
use goz_core::escompat::parse_argv;
use goz_core::output::unix_ms_to_filetime;
use goz_core::proto::{QueryResults, ResultItem};

/// 2026-10-17T12:34:56 UTC.
const MODIFIED_UNIX_MS: i64 = 1_792_240_496_000;

/// The result set every row prints: a file, a folder, a file with an unknown
/// size and date, and a file whose name needs CSV quoting.
fn results() -> QueryResults {
    let modified = Some(unix_ms_to_filetime(MODIFIED_UNIX_MS));
    let item = |path: &str, size, mtime_ft, is_dir| ResultItem {
        path: path.to_string(),
        path_u16: None,
        size,
        mtime_ft,
        is_dir,
    };
    QueryResults {
        total: 4,
        items: vec![
            item(r"C:\repo\report.txt", Some(4096), modified, false),
            item(r"C:\repo\docs", None, modified, true),
            item(r"C:\tmp\unread.bin", None, None, false),
            item(r#"C:\repo\a, "b".txt"#, Some(7), modified, false),
        ],
        more: false,
        volumes_incomplete: false,
        metadata_pending: false,
        generation: 0,
    }
}

/// One command line and the exact text gozler writes for [`results`].
struct Row {
    args: &'static [&'static str],
    prints: &'static str,
}

const ROWS: &[Row] = &[
    // Plain output: one full path per line.
    Row {
        args: &["report"],
        prints: concat!(
            r"C:\repo\report.txt",
            "\n",
            r"C:\repo\docs",
            "\n",
            r"C:\tmp\unread.bin",
            "\n",
            r#"C:\repo\a, "b".txt"#,
            "\n",
        ),
    },
    // Bug 6: plain output ignores -size and -dm.
    Row {
        args: &["report", "-size", "-dm"],
        prints: concat!(
            r"C:\repo\report.txt",
            "\n",
            r"C:\repo\docs",
            "\n",
            r"C:\tmp\unread.bin",
            "\n",
            r#"C:\repo\a, "b".txt"#,
            "\n",
        ),
    },
    Row {
        args: &["--json", "report"],
        prints: r#"{
  "total": 4,
  "items": [
    {
      "path": "C:\\repo\\report.txt",
      "size": 4096,
      "mtime_ft": 134367140960000000,
      "is_dir": false
    },
    {
      "path": "C:\\repo\\docs",
      "mtime_ft": 134367140960000000,
      "is_dir": true
    },
    {
      "path": "C:\\tmp\\unread.bin",
      "is_dir": false
    },
    {
      "path": "C:\\repo\\a, \"b\".txt",
      "size": 7,
      "mtime_ft": 134367140960000000,
      "is_dir": false
    }
  ],
  "more": false,
  "volumes_incomplete": false,
  "metadata_pending": false,
  "generation": 0
}
"#,
    },
    // CSV: a byte order mark, a header, every filename quoted, CRLF lines.
    Row {
        args: &["-export-csv", "out.csv", "report"],
        prints: concat!(
            "\u{FEFF}Filename\r\n",
            r#""C:\repo\report.txt""#,
            "\r\n",
            r#""C:\repo\docs""#,
            "\r\n",
            r#""C:\tmp\unread.bin""#,
            "\r\n",
            r#""C:\repo\a, ""b"".txt""#,
            "\r\n",
        ),
    },
    // A folder and an unknown size print an empty Size; an unknown date an
    // empty Date Modified.
    Row {
        args: &["-export-csv", "out.csv", "-size", "-dm", "report"],
        prints: concat!(
            "\u{FEFF}Size,Date Modified,Filename\r\n",
            r#"4096,2026-10-17T12:34:56,"C:\repo\report.txt""#,
            "\r\n",
            r#",2026-10-17T12:34:56,"C:\repo\docs""#,
            "\r\n",
            r#",,"C:\tmp\unread.bin""#,
            "\r\n",
            r#"7,2026-10-17T12:34:56,"C:\repo\a, ""b"".txt""#,
            "\r\n",
        ),
    },
    // Columns follow the order of their switches.
    Row {
        args: &["-export-csv", "out.csv", "-dm", "-size", "report"],
        prints: concat!(
            "\u{FEFF}Date Modified,Size,Filename\r\n",
            r#"2026-10-17T12:34:56,4096,"C:\repo\report.txt""#,
            "\r\n",
            r#"2026-10-17T12:34:56,,"C:\repo\docs""#,
            "\r\n",
            r#",,"C:\tmp\unread.bin""#,
            "\r\n",
            r#"2026-10-17T12:34:56,7,"C:\repo\a, ""b"".txt""#,
            "\r\n",
        ),
    },
    // Without the mark, the header and the default quoting, only a filename
    // that needs quotes gets them.
    Row {
        args: &[
            "-export-csv",
            "out.csv",
            "-no-utf8-bom",
            "-no-header",
            "-no-double-quote",
            "-size",
            "report",
        ],
        prints: concat!(
            r"4096,C:\repo\report.txt",
            "\r\n",
            r",C:\repo\docs",
            "\r\n",
            r",C:\tmp\unread.bin",
            "\r\n",
            r#"7,"C:\repo\a, ""b"".txt""#,
            "\r\n",
        ),
    },
    // -export-csv wins over --json.
    Row {
        args: &["-export-csv", "out.csv", "--json", "report"],
        prints: concat!(
            "\u{FEFF}Filename\r\n",
            r#""C:\repo\report.txt""#,
            "\r\n",
            r#""C:\repo\docs""#,
            "\r\n",
            r#""C:\tmp\unread.bin""#,
            "\r\n",
            r#""C:\repo\a, ""b"".txt""#,
            "\r\n",
        ),
    },
];

/// Every row prints what it recorded. All changed rows are listed at once, so
/// a change shows every row it moves.
#[test]
fn every_row_prints_what_it_recorded() {
    let results = results();
    let mut changed = Vec::new();
    for row in ROWS {
        let args: Vec<String> = row.args.iter().map(ToString::to_string).collect();
        let plan = parse_argv(&args).expect("every output row parses");
        let mut out = Vec::new();
        assert!(
            emit(&plan, &results, &jiff::tz::TimeZone::UTC, &mut out).is_ok(),
            "{:?}: writing to memory cannot fail",
            row.args
        );
        let printed = String::from_utf8(out).expect("gozler writes UTF-8");
        if printed != row.prints {
            changed.push(format!(
                "{:?}\n  recorded: {:?}\n  printed:  {:?}",
                row.args, row.prints, printed
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
