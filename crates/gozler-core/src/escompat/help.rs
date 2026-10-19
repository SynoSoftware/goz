//! The quick help and the full reference. The quick help is the examples
//! marked quick, so it fits one console screen. The full reference is
//! generated from the vocabulary tables: [`SWITCHES`], [`SORT_KEYS`],
//! [`SEARCH_FUNCTIONS`], the attribute [`LETTERS`] and [`ExitCode::ALL`], and
//! from this module's examples, search terms and notes, so a row added to a
//! table is a line of the full reference. [`search_syntax`] renders what of
//! it holds for the window's search box.

use super::ExitCode;
use super::argv::{Group, HelpScreen, SORT_KEYS, SWITCHES, Spelling};
use crate::attributes::LETTERS;
use crate::output::style::{ColorToken, paint};
use crate::query::parse::SEARCH_FUNCTIONS;

/// gozler's version, as `-version` prints it; `build.rs` appends the build
/// date unless the build is a release.
pub fn version() -> &'static str {
    env!("GOZLER_VERSION")
}

/// The help screen a [`Command::Help`](super::Command::Help) asks for, with
/// ANSI colors when `color` is set.
pub fn help(screen: HelpScreen, color: bool) -> String {
    let mut out = String::new();
    paint(
        &mut out,
        ColorToken::Strong,
        &format!("gozler {}", version()),
        color,
    );
    out.push_str(
        " - Instant file search for Windows NTFS and ReFS\nUsage: gozler [options] [search terms]\n",
    );
    match screen {
        HelpScreen::Quick => {
            out.push('\n');
            let examples: Vec<Entry> = EXAMPLES
                .iter()
                .filter(|example| example.quick)
                .map(|example| entry(example.usage.to_string(), example.text))
                .collect();
            render(&mut out, &examples, QUICK_EXAMPLE_COLUMN, color);
            out.push('\n');
            wrap(&mut out, 0, QUICK_QUOTING, 0, color);
            quick_exit_codes(&mut out, color);
            out.push_str("For every option, search function and exit code, run ");
            paint(&mut out, ColorToken::Primary, "gozler --help", color);
            out.push_str(".\n");
        }
        HelpScreen::Full => {
            out.push_str("\n  ");
            wrap(&mut out, 2, INTRO, 2, color);
            for section in full_sections() {
                render_section(&mut out, &section, color);
            }
            render_notes(&mut out, Reader::CommandLine, color);
            render_section(&mut out, &exit_codes(&ExitCode::ALL), color);
        }
    }
    // A blank line before the shell's next prompt.
    out.push('\n');
    out
}

/// When to quote a term for the shell, which the examples show.
const QUICK_QUOTING: &str = "Put quotes around a term that contains `|`, `<`, `>` or `;`.";

/// Writes the exit codes a script checks after a search that ran, in short;
/// the full reference gives each one's whole meaning.
fn quick_exit_codes(out: &mut String, color: bool) {
    let code = |out: &mut String, code: ExitCode| {
        paint(out, code.token(), &code.code().to_string(), color);
    };
    out.push_str("gozler exits ");
    code(out, ExitCode::Complete);
    out.push_str(" when done, ");
    code(out, ExitCode::Incomplete);
    out.push_str(" if results may be incomplete, ");
    code(out, ExitCode::Truncated);
    out.push_str(" if some are hidden.\n");
}

const INTRO: &str = "Find files and folders by name, path, size, date or attribute. \
     Search terms combine with AND. By default gozler prints one full path per line, \
     at most 20 unless `-n` says otherwise, and writes warnings and errors to stderr.";

/// One example: a complete command line and what it finds.
struct Example {
    usage: &'static str,
    text: &'static str,
    /// Whether the quick help lists it.
    quick: bool,
    /// The same search as typed in the window's search box, for an example
    /// that uses no option; [`search_syntax`] lists it.
    search_box: Option<&'static str>,
}

const fn quick(usage: &'static str, text: &'static str) -> Example {
    Example {
        usage,
        text,
        quick: true,
        search_box: None,
    }
}

const fn full_only(usage: &'static str, text: &'static str) -> Example {
    Example {
        usage,
        text,
        quick: false,
        search_box: None,
    }
}

impl Example {
    /// The example, which the window also lists as `query`.
    const fn typed(self, query: &'static str) -> Example {
        Example {
            search_box: Some(query),
            ..self
        }
    }
}

/// Complete command lines that work as typed in cmd and in PowerShell.
const EXAMPLES: &[Example] = &[
    quick("gozler report", "Find names that contain report").typed("report"),
    quick(
        "gozler readme*.txt",
        "Find .txt files that start with readme",
    )
    .typed("readme*.txt"),
    quick(
        "gozler report !draft",
        "Find names with report but not draft",
    )
    .typed("report !draft"),
    quick(r#"gozler "ext:cs;txt""#, "Find .cs and .txt files").typed("ext:cs;txt"),
    quick(
        r"gozler C:\repo\ *.cs",
        r"Find .cs files anywhere in C:\repo",
    )
    .typed(r"C:\repo\ *.cs"),
    quick(
        "gozler *.log dm:7days",
        "Find .log files changed in the last 7 days",
    )
    .typed("*.log dm:7days"),
    quick(
        r#"gozler "size:>1gb""#,
        "Find files and folders larger than 1 GB",
    )
    .typed("size:>1gb"),
    quick(r#"gozler "*.jpg|*.png""#, "Find .jpg and .png files").typed("*.jpg|*.png"),
    quick(
        "gozler *.iso -sort size -n 10",
        "Show the 10 largest .iso files",
    ),
    quick(
        "gozler -n all report",
        "Show every match, not just the first 20",
    ),
    quick("gozler -i README", "Find README but not readme"),
    quick(
        "gozler -export-csv out.csv ext:cs",
        "Save the .cs files to out.csv",
    ),
    quick("gozler --status", "Check whether the index is ready"),
    full_only(
        r"gozler -path C:\repo ext:cs",
        r"Find .cs files anywhere in C:\repo, with -path",
    ),
    full_only(
        "gozler /a-d report",
        "Find files, not folders, that contain report",
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
        r"Find hidden items anywhere in C:\repo, with their attributes",
    ),
    full_only(
        "gozler -export-csv out.csv -size -dm ext:cs",
        "Save every .cs file to out.csv, with sizes and dates",
    ),
    full_only("gozler -get-result-count ext:cs", "Count the .cs files"),
    full_only(
        r"gozler -get-folder-size C:\repo",
        r"Show the total size of everything in C:\repo",
    ),
    full_only("gozler --json report", "Print the matches as JSON"),
    full_only(
        "gozler -timeout 5000 -no-result-error report",
        "Wait up to 5 seconds for indexing, and exit 9 if nothing matches",
    ),
];

/// Search terms as gozler receives them, after the shell removes its quotes.
const SEARCH_TERMS: &[(&str, &str)] = &[
    ("report", "Name contains report"),
    ("*.cs", "Whole name matches a wildcard, `*` or `?`"),
    ("\"two words\"", "Name contains the phrase"),
    ("!test", "Name does not contain test"),
    ("a|b", "Either term; `|` binds tighter than a space"),
    (r"C:\repo\", "Everything below that folder"),
    ("ext:cs;txt", "Extension is cs or txt"),
    ("dm:7days", "Modified in the last 7 days"),
    ("size:>1mb", "Larger than 1 MB"),
    (
        r"docs\agents",
        r"Full path contains `docs\agents`; `/` works as `\` too",
    ),
    (r"C:\repo\*.cs", ".cs files below that folder"),
    (
        r".\src\",
        r"Everything below src in the current folder; `..\` works too",
    ),
    (
        r#""C:\Program Files (x86)\Microsoft""#,
        "A quoted path keeps its spaces",
    ),
    (
        r"*\node_modules\*",
        r"A wildcard with \ matches the whole full path",
    ),
    (
        r#"path:"C:\Program Files""#,
        "Quotes keep the spaces in a function's value",
    ),
];

/// How to type in cmd and PowerShell the search terms that a shell would
/// otherwise change.
const QUOTING: &[(&str, &str)] = &[
    (
        r#"gozler "a|b""#,
        "Quote a term with |, < or > in cmd and PowerShell",
    ),
    (
        r#"gozler \"two words\""#,
        "Search for a phrase in cmd or Windows PowerShell 5.1",
    ),
    (
        r#"gozler '"two words"'"#,
        "Search for a phrase in PowerShell 7.3 or later",
    ),
];

/// Which help a note is for. A note whose text holds for both is written
/// once, so the two helps cannot drift apart.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Reader {
    /// The full reference.
    CommandLine,
    /// The window's [`search_syntax`].
    SearchBox,
    Both,
}

const NOTES: &[(Reader, &str)] = &[
    (
        Reader::CommandLine,
        "Options start with `-` or `/`, and dashes inside an option name are \
         optional. A token that starts with / and names an option is that \
         option, as `/p` is `-p`, so write a relative path without a leading `/`: \
         `docs/agents`.",
    ),
    (
        Reader::CommandLine,
        "gozler joins its arguments with spaces and splits them into terms again, \
         so the single argument `\"ext:cs report\"` is two terms.",
    ),
    (
        Reader::CommandLine,
        "Without `-n`, gozler shows 20 results. When the limit hides matches, gozler \
         writes the shown and total counts to stderr and exits 10. `-export-*` writes \
         every match unless `-n` is given, and `-get-result-count` and the size totals \
         count every match.",
    ),
    (
        Reader::CommandLine,
        "When a volume in scope is not ready (indexing, rescanning, offline or \
         failed), gozler prints the results it has, warns on stderr and exits 2. With `-path`, only the \
         volume of that folder is in scope. `--status` exits 2 when any volume is not \
         ready or its sizes and dates could not be read.",
    ),
    (
        Reader::CommandLine,
        "When several exit codes apply, the most serious wins: a failure, then 2, \
         then 9 or 10, then 0. So no matches while a volume in scope is indexing \
         exits 2, because the matches may still appear.",
    ),
    (
        Reader::SearchBox,
        "The Match list decides how a plain name term matches. The descriptions \
         here say \"contains\", as Anywhere matches; the default, Start of words, \
         matches only where a word of the name starts.",
    ),
    (
        Reader::Both,
        "Files whose size or date Windows did not report never match `size:` or \
         `dm:`, even negated.",
    ),
    (
        Reader::CommandLine,
        "Files whose size or date Windows did not report add nothing to the size \
         totals. When reading sizes and dates failed on a volume in scope, a search \
         that uses them warns and exits 2. When the read succeeded, a file it could \
         not read is rare and is left out without a warning.",
    ),
    (
        Reader::CommandLine,
        "A folder's size is the total of the known sizes of the files below it, \
         each name of a hard-linked file counted; `size:`, `-size` and `-sort size` \
         use it. `-get-total-size` and `-get-folder-size` add only files, so no file \
         is counted twice.",
    ),
    (
        Reader::Both,
        "`dm:today`, `dm:yesterday` and `dm:YYYY-MM-DD` are calendar days in the local \
         time zone. `7days` is the 7 times 24 hours before the search starts, and `dm:<30days` finds files older than 30 \
         days.",
    ),
    (
        Reader::CommandLine,
        r"`regex:` and `-r` use Rust regular expressions, without look-around or backreferences. A pattern matches the name; with `-p`, or when it contains `\\`, it matches the full path, whose separators are `\`, so write `\\` or `[\\/]` for a separator. `|` and `!` inside a pattern belong to the expression.",
    ),
    (
        Reader::SearchBox,
        r"With Regular expression in the Match list, the whole search box is one pattern. `regex:` and that pattern use Rust regular expressions, without look-around or backreferences. A pattern matches the name; with Match path, or when it contains `\\`, it matches the full path, whose separators are `\`, so write `\\` or `[\\/]` for a separator.",
    ),
    (
        Reader::CommandLine,
        "Dates print as ISO 8601 in local time, whatever the locale. Plain output \
         groups the digits of a size with commas; CSV and TSV print plain bytes. \
         Attributes print as letters, such as HA, and in CSV and TSV as their \
         FILE_ATTRIBUTE_* number, such as 34.",
    ),
    (
        Reader::CommandLine,
        "An unknown search function, such as `dc:`, and an unquoted `<` or `>` outside a \
         function value are invalid input and exit 4.",
    ),
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
    /// The color of the usage.
    token: ColorToken,
}

/// The examples that use no option, the search terms, the search functions,
/// the attribute letters and the notes for a search box, without color: what
/// a search box accepts, with nothing about options, shell quoting or exit
/// codes. Its rows are the full reference's, except the notes about the
/// window's own match controls.
pub fn search_syntax() -> String {
    let examples = EXAMPLES
        .iter()
        .filter_map(|example| {
            example
                .search_box
                .map(|query| entry(query.to_string(), example.text))
        })
        .collect();
    let mut out = String::new();
    for section in [
        section("Examples:", examples),
        search_terms("Search terms:"),
        search_functions(),
        attribute_letters("Attribute letters:"),
    ] {
        render_section(&mut out, &section, false);
    }
    render_notes(&mut out, Reader::SearchBox, false);
    // Each section starts with a blank line, which the first one does not
    // need.
    out.remove(0);
    out
}

/// The full reference's sections above its notes and exit codes.
fn full_sections() -> Vec<Section> {
    let examples = EXAMPLES
        .iter()
        .map(|example| entry(example.usage.to_string(), example.text))
        .collect();
    let mut sections = vec![
        Section {
            usage_column: EXAMPLE_COLUMN,
            ..section("Examples:", examples)
        },
        search_terms(
            "Search syntax (what gozler receives; quote it for the shell as shown below):",
        ),
        Section {
            usage_column: EXAMPLE_COLUMN,
            ..section("Quoting in the shell:", pairs(QUOTING))
        },
        search_functions(),
        attribute_letters("Attribute letters (for attrib: and /a):"),
    ];
    for (group, title) in GROUPS {
        let options = SWITCHES
            .iter()
            .filter(|switch| switch.group == group)
            .map(|switch| entry(switch_usage(&switch.spelling), switch.help))
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

fn search_terms(title: &'static str) -> Section {
    section(title, pairs(SEARCH_TERMS))
}

fn search_functions() -> Section {
    section(
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
    )
}

fn attribute_letters(title: &'static str) -> Section {
    section(
        title,
        LETTERS
            .iter()
            .map(|row| entry(row.letter.to_string(), row.meaning))
            .collect(),
    )
}

/// The entries of a table of usages and their texts.
fn pairs(table: &[(&str, &'static str)]) -> Vec<Entry> {
    table
        .iter()
        .map(|&(usage, text)| entry(usage.to_string(), text))
        .collect()
}

fn exit_codes(codes: &[ExitCode]) -> Section {
    section(
        "Exit codes:",
        codes
            .iter()
            .map(|&code| Entry {
                token: code.token(),
                ..entry(code.code().to_string(), code.meaning())
            })
            .collect(),
    )
}

/// An entry whose usage is text the reader types.
fn entry(usage: String, text: &'static str) -> Entry {
    Entry {
        usage,
        text,
        token: ColorToken::Primary,
    }
}

fn section(title: &'static str, entries: Vec<Entry>) -> Section {
    Section {
        title,
        entries,
        usage_column: USAGE_COLUMN,
    }
}

/// The forms of a switch the help lists, such as `-n N, -max-results N`.
fn switch_usage(spelling: &Spelling) -> String {
    match spelling {
        Spelling::Es { names, action } => {
            let param = action.param().map(|p| format!(" {p}")).unwrap_or_default();
            let forms: Vec<String> = names.iter().map(|n| format!("-{n}{param}")).collect();
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
/// [`EXAMPLE_COLUMN`] for the quick help, which is only examples, so every
/// one of them keeps its text on its line.
const QUICK_EXAMPLE_COLUMN: usize = 36;
/// The width the help wraps text to, so it fits an 80-column console.
const LINE_WIDTH: usize = 79;

/// Writes the section's title and its entries, after a blank line.
fn render_section(out: &mut String, section: &Section, color: bool) {
    out.push('\n');
    paint(out, ColorToken::Strong, section.title, color);
    out.push('\n');
    render(out, &section.entries, section.usage_column, color);
}

/// Writes the notes for `reader` and for both helps, after a blank line.
fn render_notes(out: &mut String, reader: Reader, color: bool) {
    out.push('\n');
    paint(out, ColorToken::Strong, "Notes:", color);
    out.push('\n');
    for &(_, note) in NOTES
        .iter()
        .filter(|(note_reader, _)| [reader, Reader::Both].contains(note_reader))
    {
        out.push_str("  - ");
        wrap(out, 4, note, 4, color);
    }
}

/// Writes `entries` as two columns: the usage, in its token's color when
/// `color` is set, and its text wrapped beside it.
fn render(out: &mut String, entries: &[Entry], usage_column: usize, color: bool) {
    let width = entries
        .iter()
        .map(|e| e.usage.len())
        .filter(|&len| len <= usage_column)
        .max()
        .unwrap_or(0)
        + 2;
    for entry in entries {
        out.push_str("  ");
        paint(out, entry.token, &entry.usage, color);
        if entry.usage.len() < width {
            out.push_str(&" ".repeat(width - entry.usage.len()));
        } else {
            out.push('\n');
            out.push_str(&" ".repeat(2 + width));
        }
        wrap(out, 2 + width, entry.text, 2 + width, color);
    }
}

/// Appends `text` word-wrapped to [`LINE_WIDTH`], starting at `column` of the
/// current line, with every line after the first indented by `indent` spaces.
///
/// Text between backticks is text the reader types, such as `` `-n` ``: the
/// backticks are dropped, and the text is colored when `color` is set.
fn wrap(out: &mut String, column: usize, text: &str, indent: usize, color: bool) {
    let mut column = column;
    let mut at_line_start = true;
    let mut typed = false;
    for word in text.split(' ') {
        let width = word.chars().filter(|&c| c != '`').count();
        if !at_line_start && column + 1 + width > LINE_WIDTH {
            out.push('\n');
            out.push_str(&" ".repeat(indent));
            column = indent;
            at_line_start = true;
        }
        if !at_line_start {
            out.push(' ');
            column += 1;
        }
        for (i, part) in word.split('`').enumerate() {
            if i > 0 {
                typed = !typed;
            }
            if typed {
                paint(out, ColorToken::Primary, part, color);
            } else {
                out.push_str(part);
            }
        }
        column += width;
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
            Ok(Command::Overview) => panic!("the help lists no empty command line"),
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

        // The window's form of an example is the query its command line
        // gives, so the two cannot drift apart.
        for example in EXAMPLES {
            let usage = example.usage;
            let args = command_line_args(usage);
            assert_eq!(args[0], "gozler", "{usage}");
            match parse_argv(&args[1..]) {
                Ok(Command::Run(plan)) => {
                    parse(&plan.query).unwrap_or_else(|e| panic!("{usage}: {e}"));
                    if let Some(query) = example.search_box {
                        assert_eq!(plan.query, query, "{usage}");
                    }
                }
                other => panic!("{usage}: {other:?}"),
            }
        }
    }

    /// The quick help fits an 80 by 25 console with the command above it, a
    /// blank line and the next prompt below, so it never scrolls.
    #[test]
    fn the_quick_help_fits_one_console_screen() {
        let text = help(HelpScreen::Quick, false);
        assert!(text.lines().count() <= 22, "{text}");
        assert!(
            text.lines().all(|line| line.chars().count() <= 80),
            "{text}"
        );
    }

    /// Color adds only ANSI codes: the columns line up as they do in plain
    /// text, because the padding is measured on the text alone.
    #[test]
    fn color_changes_no_text_or_alignment() {
        for screen in [HelpScreen::Quick, HelpScreen::Full] {
            let mut stripped = String::new();
            let mut in_code = false;
            for c in help(screen, true).chars() {
                match c {
                    '\x1b' => in_code = true,
                    'm' if in_code => in_code = false,
                    c if !in_code => stripped.push(c),
                    _ => {}
                }
            }
            assert_eq!(stripped, help(screen, false), "{screen:?}");
        }
    }
}
