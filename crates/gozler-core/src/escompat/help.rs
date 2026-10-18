//! The quick help and the full reference, generated from the vocabulary
//! tables: [`SWITCHES`], [`SORT_KEYS`], [`SEARCH_FUNCTIONS`], the attribute
//! [`LETTERS`] and [`ExitCode::ALL`], and from this module's examples, search
//! terms and notes. A row added to a table is a line of the full reference;
//! a row marked quick is also a line of the quick help.

use super::ExitCode;
use super::argv::{Group, HelpScreen, SORT_KEYS, SWITCHES, Spelling};
use crate::attributes::LETTERS;
use crate::query::parse::SEARCH_FUNCTIONS;

/// gozler's version, as `-version` prints it; `build.rs` appends the build
/// date unless the build is a release.
pub fn version() -> &'static str {
    env!("GOZLER_VERSION")
}

/// The help screen a [`Command::Help`](super::Command::Help) asks for.
pub fn help(screen: HelpScreen) -> String {
    let mut out = format!(
        "gozler {} - Instant file search for Windows NTFS\n\n\
         Usage: gozler [options] [search terms]\n\n",
        version()
    );
    wrap_into(&mut out, "  ", INTRO, 2);
    match screen {
        HelpScreen::Quick => {
            for section in quick_sections() {
                render_section(&mut out, &section);
            }
            render_section(&mut out, &exit_codes(&QUICK_EXIT_CODES));
            wrap_into(&mut out, "  ", QUICK_EXIT_NOTE, 2);
            out.push('\n');
            wrap_into(&mut out, "", QUICK_FOOTER, 0);
        }
        HelpScreen::Full => {
            for section in full_sections() {
                render_section(&mut out, &section);
            }
            out.push_str("\nNotes:\n");
            for note in NOTES {
                wrap_into(&mut out, "  - ", note, 4);
            }
            render_section(&mut out, &exit_codes(&ExitCode::ALL));
        }
    }
    out
}

/// The quick help lists no precedence rule, so a script that sees 2 must
/// still learn from stderr that the list was also cut.
const QUICK_EXIT_NOTE: &str = "When 2 and 10 both apply, gozler exits 2; stderr still \
     reports every warning, including the shown and total counts.";

const INTRO: &str = "Find files and folders by name, path, size, date or attribute. \
     Search terms combine with AND. By default gozler prints one full path per line, \
     at most 20 unless -n says otherwise, and writes warnings and errors to stderr.";

const QUICK_FOOTER: &str = "Run gozler --help-full for every option, search function, \
     attribute letter, note and exit code.";

/// The exit codes the quick help lists: the ones a script checks after a
/// search that ran.
const QUICK_EXIT_CODES: [ExitCode; 3] = [
    ExitCode::Complete,
    ExitCode::Incomplete,
    ExitCode::Truncated,
];

/// One line of the examples, the search syntax or the quoting.
struct Line {
    usage: &'static str,
    text: &'static str,
    /// Whether the quick help lists it too.
    quick: bool,
}

const fn quick(usage: &'static str, text: &'static str) -> Line {
    Line {
        usage,
        text,
        quick: true,
    }
}

const fn full_only(usage: &'static str, text: &'static str) -> Line {
    Line {
        usage,
        text,
        quick: false,
    }
}

/// Complete command lines that work as typed in cmd and in PowerShell.
const EXAMPLES: &[Line] = &[
    quick("gozler report", "Find names that contain report"),
    quick("gozler *.cs", "Find .cs files"),
    quick(
        r"gozler -path C:\repo ext:cs",
        r"Find .cs files below C:\repo",
    ),
    quick(
        "gozler *.log dm:7days",
        "Find .log files modified in the last 7 days",
    ),
    quick(r#"gozler "size:>1gb""#, "Find files larger than 1 GB"),
    quick(
        "gozler *.iso -sort size -n 10",
        "Show the 10 largest .iso files",
    ),
    quick(
        "gozler -n all report",
        "Show every match, not only the first 20",
    ),
    full_only(
        "gozler /a-d report",
        "Find only files whose names contain report",
    ),
    full_only(
        r"gozler -parent C:\repo -n all",
        r"List everything directly in C:\repo",
    ),
    full_only(
        r#"gozler -r "^test[0-9]+\.cs$" -path C:\repo"#,
        "Find names that match a regular expression",
    ),
    full_only(
        r"gozler /ah -attributes -path C:\repo",
        r"Find hidden items below C:\repo, with their attributes",
    ),
    full_only(
        "gozler -export-csv files.csv -size -dm ext:cs",
        "Write every .cs file to files.csv, with sizes and dates",
    ),
    full_only("gozler -get-result-count ext:cs", "Count the .cs files"),
    full_only(
        r"gozler -get-folder-size C:\repo",
        r"Show the total size of the files below C:\repo",
    ),
    full_only("gozler --json report", "Print the matches as JSON"),
    full_only(
        "gozler -timeout 5000 -no-result-error report",
        "Wait up to 5 seconds for indexing, and exit 9 if nothing matches",
    ),
];

/// Search terms as gozler receives them, after the shell removes its quotes.
const SEARCH_TERMS: &[Line] = &[
    quick("report", "Name contains report"),
    quick("*.cs", "Whole name matches a wildcard (* and ?)"),
    quick("\"two words\"", "Name contains the phrase"),
    quick("!test", "Exclude names that contain test"),
    quick("a|b", "Either term; | binds tighter than a space"),
    quick(r"C:\repo\", "Everything below that folder"),
    quick("ext:cs;txt", "Extension is cs or txt"),
    quick("dm:7days", "Modified in the last 7 days"),
    quick("size:>1mb", "Larger than 1 MB"),
    full_only(
        r"docs\agents",
        r"Full path contains docs\agents; / works as \ too",
    ),
    full_only(r"C:\repo\*.cs", ".cs files below that folder"),
    full_only(
        r".\src\",
        r"Everything below src in the current folder; ..\ works too",
    ),
    full_only(
        r#""C:\Program Files (x86)\Microsoft""#,
        "A quoted path keeps its spaces",
    ),
    full_only(
        r"*\node_modules\*",
        r"A wildcard with \ matches the whole full path",
    ),
    full_only(
        r#"path:"C:\Program Files""#,
        "Quotes keep the spaces in a function's value",
    ),
];

/// How to type in cmd and PowerShell the search terms that a shell would
/// otherwise change.
const QUOTING: &[Line] = &[
    quick(
        r#"gozler "a|b""#,
        "Quote a term with |, < or > in cmd and PowerShell",
    ),
    quick(
        r#"gozler \"two words\""#,
        "Search for a phrase in cmd or Windows PowerShell 5.1",
    ),
    quick(
        r#"gozler '"two words"'"#,
        "Search for a phrase in PowerShell 7.3 or later",
    ),
];

const NOTES: &[&str] = &[
    "Options start with - or /, and dashes inside an option name are \
     optional. A token that starts with / and names an option is that \
     option, as /p is -p, so write a relative path without a leading /: \
     docs/agents.",
    "gozler joins its arguments with spaces and splits them into terms again, \
     so the single argument \"ext:cs report\" is two terms.",
    "Without -n, gozler shows 20 results. When the limit hides matches, gozler \
     writes the shown and total counts to stderr and exits 10. -export-* writes \
     every match unless -n is given, and -get-result-count and the size totals \
     count every match.",
    "When a volume in scope is not ready (indexing, rescanning, offline or \
     failed), gozler prints the results it has, warns on stderr and exits 2. With -path, only the \
     volume of that folder is in scope. --status exits 2 when any volume is not \
     ready or its sizes and dates could not be read.",
    "When several exit codes apply, the most serious wins: a failure, then 2, \
     then 9 or 10, then 0. So no matches while a volume in scope is indexing \
     exits 2, because the matches may still appear.",
    "Files whose size or date Windows did not report never match size: or dm:, \
     even negated, and add nothing to the size totals. When reading sizes and \
     dates failed on a volume in scope, a search that uses them warns and exits \
     2. When the read succeeded, a file it could not read is rare and is left \
     out without a warning.",
    "dm:today, dm:yesterday and dm:YYYY-MM-DD are calendar days in the local \
     time zone. 7days is the 7 times 24 hours before the search starts, and dm:<30days finds files older than 30 \
     days.",
    r"regex: and -r use Rust regular expressions, without look-around or backreferences. A pattern matches the name; with -p, or when it contains \\, it matches the full path, whose separators are \, so write \\ or [\\/] for a separator. | and ! inside a pattern belong to the expression.",
    "Dates print as ISO 8601 in local time, whatever the locale. Plain output \
     groups the digits of a size with commas; CSV and TSV print plain bytes. \
     Attributes print as letters, such as HA, and in CSV and TSV as their \
     FILE_ATTRIBUTE_* number, such as 34.",
    "An unknown search function, such as dc:, and an unquoted < or > outside a \
     function value are invalid input and exit 4.",
];

/// The full reference's option sections, in the order it lists them.
const GROUPS: [(Group, &str); 5] = [
    (Group::Search, "Search options:"),
    (Group::LimitAndSort, "Limit and sort:"),
    (Group::Output, "Output:"),
    (Group::Totals, "Totals:"),
    (Group::Other, "Other options:"),
];

/// A titled list of help entries, generated from one table.
struct Section {
    title: &'static str,
    entries: Vec<Entry>,
    /// A usage longer than this puts its text on the next line.
    usage_column: usize,
}

/// One help line: what to type, and what it does.
struct Entry {
    usage: String,
    text: &'static str,
}

/// The examples, the search syntax and the quoting, which open both screens;
/// the quick help lists only their quick lines.
fn opening_sections(screen: HelpScreen) -> Vec<Section> {
    let listed = |table: &[Line]| -> Vec<Entry> {
        table
            .iter()
            .filter(|line| screen == HelpScreen::Full || line.quick)
            .map(|line| entry(line.usage.to_string(), line.text))
            .collect()
    };
    vec![
        Section {
            usage_column: EXAMPLE_COLUMN,
            ..section("Examples:", listed(EXAMPLES))
        },
        section(
            "Search syntax (what gozler receives; quote it for the shell as shown below):",
            listed(SEARCH_TERMS),
        ),
        Section {
            usage_column: EXAMPLE_COLUMN,
            ..section("Quoting in the shell:", listed(QUOTING))
        },
    ]
}

/// The quick help's sections above its exit codes.
fn quick_sections() -> Vec<Section> {
    let options = SWITCHES
        .iter()
        .filter(|switch| switch.quick)
        .map(|switch| {
            entry(
                switch_usage(&switch.spelling, HelpScreen::Quick),
                switch.help,
            )
        })
        .collect();
    let mut sections = opening_sections(HelpScreen::Quick);
    sections.push(section("Common options:", options));
    sections
}

/// The full reference's sections above its notes and exit codes.
fn full_sections() -> Vec<Section> {
    let mut sections = opening_sections(HelpScreen::Full);
    sections.push(section(
        "Search functions:",
        SEARCH_FUNCTIONS
            .iter()
            .map(|function| {
                let forms: Vec<String> = function
                    .names
                    .iter()
                    .map(|name| format!("{name}:{}", function.example))
                    .collect();
                entry(forms.join(", "), function.help)
            })
            .collect(),
    ));
    sections.push(section(
        "Attribute letters (for attrib: and /a):",
        LETTERS
            .iter()
            .map(|row| entry(row.letter.to_string(), row.meaning))
            .collect(),
    ));
    for (group, title) in GROUPS {
        let options = SWITCHES
            .iter()
            .filter(|switch| switch.group == group)
            .map(|switch| {
                entry(
                    switch_usage(&switch.spelling, HelpScreen::Full),
                    switch.help,
                )
            })
            .collect();
        sections.push(section(title, options));
        if group == Group::LimitAndSort {
            sections.push(section(
                "Sort keys (for -sort KEY and -sort-KEY):",
                SORT_KEYS
                    .iter()
                    .map(|row| entry(row.names.join(", "), row.help))
                    .collect(),
            ));
        }
    }
    sections
}

fn exit_codes(codes: &[ExitCode]) -> Section {
    section(
        "Exit codes:",
        codes
            .iter()
            .map(|code| entry(code.code().to_string(), code.meaning()))
            .collect(),
    )
}

fn entry(usage: String, text: &'static str) -> Entry {
    Entry { usage, text }
}

fn section(title: &'static str, entries: Vec<Entry>) -> Section {
    Section {
        title,
        entries,
        usage_column: USAGE_COLUMN,
    }
}

/// The forms of a switch the help lists, such as `-n N, -max-results N`; the
/// quick help lists only the first.
fn switch_usage(spelling: &Spelling, screen: HelpScreen) -> String {
    match spelling {
        Spelling::Es { names, action } => {
            let param = action.param().map(|p| format!(" {p}")).unwrap_or_default();
            let listed = match screen {
                HelpScreen::Quick => &names[..1],
                HelpScreen::Full => &names[..],
            };
            let forms: Vec<String> = listed.iter().map(|n| format!("-{n}{param}")).collect();
            forms.join(", ")
        }
        Spelling::Family { usage, .. } => (*usage).to_string(),
        Spelling::Native { name, action } => {
            let param = action.param().map(|p| format!(" {p}")).unwrap_or_default();
            format!("--{name}{param}")
        }
    }
}

/// A usage longer than this puts its text on the next line.
const USAGE_COLUMN: usize = 24;
/// [`USAGE_COLUMN`] for the examples, whose usage is a whole command line.
const EXAMPLE_COLUMN: usize = 30;
/// The width the help wraps text to, so it fits an 80-column console.
const LINE_WIDTH: usize = 79;

/// Writes the section's title and its entries, after a blank line.
fn render_section(out: &mut String, section: &Section) {
    out.push('\n');
    out.push_str(section.title);
    out.push('\n');
    render(out, &section.entries, section.usage_column);
}

/// Writes `entries` as two columns: the usage, and its text wrapped beside it.
fn render(out: &mut String, entries: &[Entry], usage_column: usize) {
    let width = entries
        .iter()
        .map(|e| e.usage.len())
        .filter(|&len| len <= usage_column)
        .max()
        .unwrap_or(0)
        + 2;
    for entry in entries {
        let usage = format!("  {}", entry.usage);
        if entry.usage.len() < width {
            wrap_into(
                out,
                &format!("{usage:<w$}", w = 2 + width),
                entry.text,
                2 + width,
            );
        } else {
            out.push_str(&usage);
            out.push('\n');
            wrap_into(out, &" ".repeat(2 + width), entry.text, 2 + width);
        }
    }
}

/// Appends `start` and then `text` word-wrapped to [`LINE_WIDTH`], with every
/// line after the first indented by `indent` spaces.
fn wrap_into(out: &mut String, start: &str, text: &str, indent: usize) {
    out.push_str(start);
    let mut column = start.len();
    let mut at_line_start = true;
    for word in text.split(' ') {
        if !at_line_start && column + 1 + word.len() > LINE_WIDTH {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            column = indent;
            at_line_start = true;
        }
        if !at_line_start {
            out.push(' ');
            column += 1;
        }
        out.push_str(word);
        column += word.len();
        at_line_start = false;
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::escompat::{Command, parse_argv};
    use crate::query::{ParseOptions, ParsedQuery, QueryError, QueryStart, Test, parse_query_with};

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(ToString::to_string).collect()
    }

    /// Splits a command line into arguments as Windows does for a program,
    /// for the quoting the examples use: `"` groups spaces and is removed,
    /// and `\"` is a literal `"`.
    fn command_line_args(line: &str) -> Vec<String> {
        let (mut args, mut arg, mut quoted) = (Vec::new(), String::new(), false);
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\\' if chars.peek() == Some(&'"') => {
                    chars.next();
                    arg.push('"');
                }
                '"' => quoted = !quoted,
                ' ' if !quoted => {
                    if !arg.is_empty() {
                        args.push(std::mem::take(&mut arg));
                    }
                }
                c => arg.push(c),
            }
        }
        if !arg.is_empty() {
            args.push(arg);
        }
        args
    }

    /// `query` parsed in `C:\repo`, as gozler parses it, so relative paths
    /// and `dm:` values resolve.
    fn parse(query: &str) -> Result<ParsedQuery, QueryError> {
        let start = QueryStart {
            now: jiff::Timestamp::UNIX_EPOCH,
            time_zone: jiff::tz::TimeZone::UTC,
        };
        let options = ParseOptions {
            working_dir: Some(r"C:\repo"),
            query_start: Some(&start),
            ..ParseOptions::default()
        };
        parse_query_with(query, options)
    }

    /// The usages of the full reference's section whose title starts with
    /// `title`.
    fn entries(title: &str) -> Vec<String> {
        full_sections()
            .into_iter()
            .find(|section| section.title.starts_with(title))
            .unwrap_or_else(|| panic!("the full reference has a {title} section"))
            .entries
            .into_iter()
            .map(|entry| entry.usage)
            .collect()
    }

    /// A search that searches no text: a listed switch the parser does not
    /// know would fail, or with `/` become search text. A switch may add a
    /// function, as `/ad` adds `folder:`.
    fn assert_switch_parses(tokens: &[&str]) {
        match parse_argv(&argv(tokens)) {
            Ok(Command::Run(plan)) => {
                let parsed = parse(&plan.query).unwrap_or_else(|e| panic!("{tokens:?}: {e}"));
                assert!(
                    parsed.is_text_empty(),
                    "{tokens:?} searches {:?}",
                    plan.query
                );
            }
            Ok(Command::Help(_) | Command::Version) => {}
            Err(fatal) => panic!("{tokens:?}: {fatal}"),
        }
    }

    /// Every switch, sort key, search function, search term and example the
    /// full reference lists parses as the help describes it, so the help
    /// cannot list something gozler refuses or searches as text. The quick
    /// help lists a subset of the same rows.
    #[test]
    fn everything_the_help_lists_parses() {
        let sort_keys: Vec<String> = entries("Sort keys")
            .iter()
            .flat_map(|names| names.split(", ").map(str::to_string).collect::<Vec<_>>())
            .collect();
        for key in &sort_keys {
            for suffix in ["", "-ascending", "-descending"] {
                assert_switch_parses(&["-sort", &format!("{key}{suffix}")]);
                assert_switch_parses(&[&format!("-sort-{key}{suffix}")]);
            }
        }

        let options: Vec<String> = GROUPS
            .iter()
            .flat_map(|(_, title)| entries(title))
            .collect();
        assert_eq!(
            options.len(),
            SWITCHES.len(),
            "every switch is in a group the full reference lists"
        );
        let switch_forms: Vec<String> = options
            .iter()
            .flat_map(|usage| usage.split(", ").map(str::to_string).collect::<Vec<_>>())
            .collect();
        let letters = entries("Attribute letters");
        for form in &switch_forms {
            let mut tokens: Vec<&str> = form.split(' ').collect();
            match tokens.get(1).copied() {
                None if form.contains("KEY") => {
                    for key in &sort_keys {
                        assert_switch_parses(&[&form.replace("KEY", key)]);
                    }
                    continue;
                }
                None if form.contains("LETTERS") => {
                    for letter in &letters {
                        assert_switch_parses(&[&form.replace("LETTERS", letter)]);
                    }
                    continue;
                }
                None => {}
                Some("N") => tokens[1] = "5",
                Some("MS") => tokens[1] = "1000",
                Some("DIR") => tokens[1] = r"C:\repo",
                Some("PATH") => tokens[1] = r"C:\repo\x",
                Some("FILE") => tokens[1] = "out.csv",
                Some("KEY") => tokens[1] = "name",
                Some("PATTERN") => tokens[1] = r"^test\d+\.cs$",
                Some(param) => panic!("{form}: the test has no value for {param}"),
            }
            assert_switch_parses(&tokens);
        }

        let searched_as_text = |parsed: &ParsedQuery| {
            parsed
                .clauses
                .iter()
                .flat_map(|clause| &clause.terms)
                .any(|term| matches!(&term.test, Test::Name(name) if name.contains(':')))
        };
        for usage in entries("Search functions") {
            for form in usage.split(", ") {
                let parsed = parse(form).unwrap_or_else(|e| panic!("{form}: {e}"));
                assert!(
                    !searched_as_text(&parsed),
                    "{form} is searched as text, not as a function"
                );
            }
        }

        for term in entries("Search syntax") {
            let parsed = parse(&term).unwrap_or_else(|e| panic!("{term}: {e}"));
            assert!(!parsed.clauses.is_empty(), "{term} searches for nothing");
            assert!(
                !searched_as_text(&parsed),
                "{term} searches a function as text"
            );
        }

        for example in entries("Examples") {
            let args = command_line_args(&example);
            assert_eq!(args[0], "gozler", "{example}");
            match parse_argv(&args[1..]) {
                Ok(Command::Run(plan)) => {
                    parse(&plan.query).unwrap_or_else(|e| panic!("{example}: {e}"));
                }
                other => panic!("{example}: {other:?}"),
            }
        }
    }
}
