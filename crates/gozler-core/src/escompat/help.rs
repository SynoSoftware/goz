//! The help screen, generated from the vocabulary tables: [`SWITCHES`],
//! [`SORT_KEYS`], [`SEARCH_FUNCTIONS`], [`ExitCode::ALL`] and [`DIFFERENCES`].
//! A row added to a table is a line of the help.

use super::ExitCode;
use super::argv::{SORT_KEYS, SWITCHES, Spelling};
use crate::query::parse::SEARCH_FUNCTIONS;

/// gozler's version, as `-version` prints it.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The help screen `-h` and `-help` print.
pub fn help() -> String {
    let mut out = format!(
        "gozler {} - instant file search for Windows NTFS\n\n\
         Usage: gozler [switches] [search terms]\n\n",
        version()
    );
    wrap_into(&mut out, "  ", INTRO, 2);
    for section in sections() {
        out.push('\n');
        out.push_str(section.title);
        out.push('\n');
        render(&mut out, &section.entries);
    }
    out.push_str("\nNOTES\n");
    for note in NOTES {
        wrap_into(&mut out, "  ", note, 2);
    }
    out.push_str("  Differences from es:\n");
    for row in DIFFERENCES {
        wrap_into(&mut out, "  - ", &format!("{}.", row.gozler), 4);
        wrap_into(&mut out, "    es: ", &format!("{}.", row.es), 4);
        wrap_into(&mut out, "    Why: ", &format!("{}.", row.why), 4);
    }
    out.push_str("\nEXIT CODES\n");
    let codes: Vec<Entry> = ExitCode::ALL
        .iter()
        .map(|code| Entry {
            usage: code.code().to_string(),
            text: code.meaning(),
        })
        .collect();
    render(&mut out, &codes);
    out
}

const INTRO: &str = "Finds files and folders by name, path and size. Terms combine with \
     AND. Shows 20 results unless -n says otherwise. Results: one full path per \
     line on stdout; diagnostics on stderr. A switch starts with - or /, and each \
     dash inside its name is optional.";

/// Search terms that use no function, with what each finds.
const SEARCH_TERMS: &[(&str, &str)] = &[
    ("report", "names containing \"report\""),
    ("*.cs", "whole names matching a wildcard (* and ?)"),
    (
        "\"two words\"",
        "a phrase; the shell must pass the double quotes",
    ),
    (r"docs\agents", r"full paths containing docs\agents"),
    (r"C:\repo\", "everything below that folder"),
    (r"C:\repo\*.cs", ".cs files below that folder"),
    (
        r"*\node_modules\*",
        r"a wildcard with \ matches the whole full path",
    ),
];

/// Complete command lines, each of which parses.
const EXAMPLES: &[&str] = &[
    "gozler readme",
    r"gozler -path C:\repo ext:cs",
    "gozler *.log -sort dm -n 10",
    "gozler -n all ext:cs",
    "gozler -timeout 5000 -no-result-error report",
    "gozler -export-csv files.csv -size -dm ext:cs",
    "gozler /a-d report -size -dm",
    "gozler -get-result-count ext:cs",
    r"gozler -get-folder-size C:\repo",
    "gozler --json report",
];

const NOTES: &[&str] = &[
    "A volume in scope that is indexing, rescanning or offline makes the \
     answer incomplete: gozler prints the results it has, warns on stderr and \
     exits 2. With -path, only the volume of that folder is in scope.",
    "Files whose size Windows did not report never match size: and add \
     nothing to -get-total-size or -get-folder-size.",
];

/// One way gozler answers differently from es. The help's NOTES print every
/// row, so a script written for es can find each difference.
struct Difference {
    gozler: &'static str,
    es: &'static str,
    why: &'static str,
}

const DIFFERENCES: &[Difference] = &[
    Difference {
        gozler: "In CSV and TSV, Filename comes after the other columns",
        es: "an export puts Filename first",
        why: "v0.2.0's CSV layout",
    },
    Difference {
        gozler: "Dates are ISO 8601 in local time, and plain output groups digits with \
                 commas, whatever the locale",
        es: "dates and digit grouping follow the locale",
        why: "scripts parse ISO dates without knowing the locale",
    },
    Difference {
        gozler: "Plain output pads only the Size and Date Modified columns",
        es: "every column is padded to a fixed width",
        why: "a name or folder has no fixed width",
    },
    Difference {
        gozler: "An export file starts with a UTF-8 byte order mark; -no-utf8-bom leaves it out",
        es: "a byte order mark only with -utf8-bom",
        why: "v0.2.0's default",
    },
    Difference {
        gozler: "/a letters other than d, and /oe, are refused with exit 4",
        es: "every attribute letter and sort by extension work",
        why: "the index holds no attributes, and gozler has no extension sort yet",
    },
    Difference {
        gozler: "One argument with a space splits into terms; write a phrase as \"two words\"",
        es: "one argument with a space is a phrase",
        why: "scripts build a query in one string",
    },
    Difference {
        gozler: "An unquoted ! or | is refused with exit 4; quote it to search for it",
        es: "! is not, and | is or",
        why: "this release does not implement them",
    },
    Difference {
        gozler: "Shows 20 results unless -n says otherwise, and exits 10 with a warning \
                 when the limit leaves out matches, -n 0 included",
        es: "prints every match, and -n cuts the list silently with exit 0",
        why: "a query that forgets -n on a volume of a million files floods a \
              console or an agent's context",
    },
    Difference {
        gozler: "-n all shows every result",
        es: "-n all is invalid input",
        why: "it is the way to lift the default limit",
    },
    Difference {
        gozler: "Each outcome has its own exit code, listed below",
        es: "exit codes are undocumented",
        why: "a script needs one code per outcome",
    },
    Difference {
        gozler: "size: never matches a folder, and -get-total-size and \
                 -get-folder-size add up the sizes of files only",
        es: "folder sizes can be indexed, matched and added up",
        why: "the index holds no folder sizes",
    },
];

/// A titled list of help entries, generated from one table.
struct Section {
    title: &'static str,
    entries: Vec<Entry>,
}

/// One help line: what to type, and what it does.
struct Entry {
    usage: String,
    text: &'static str,
}

fn sections() -> Vec<Section> {
    let entry = |usage: String, text| Entry { usage, text };
    let switches = |native: bool| -> Vec<Entry> {
        SWITCHES
            .iter()
            .filter(|switch| matches!(switch.spelling, Spelling::Native { .. }) == native)
            .map(|switch| entry(switch_usage(&switch.spelling), switch.help))
            .collect()
    };
    vec![
        Section {
            title: "SEARCH TERMS",
            entries: SEARCH_TERMS
                .iter()
                .map(|(term, text)| entry((*term).to_string(), *text))
                .collect(),
        },
        Section {
            title: "SEARCH FUNCTIONS",
            entries: SEARCH_FUNCTIONS
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
        },
        Section {
            title: "SWITCHES",
            entries: switches(false),
        },
        Section {
            title: "SORT KEYS (for -sort KEY and -sort-KEY)",
            entries: SORT_KEYS
                .iter()
                .map(|row| entry(row.names.join(", "), row.help))
                .collect(),
        },
        Section {
            title: "GOZLER OPTIONS",
            entries: switches(true),
        },
        Section {
            title: "EXAMPLES",
            entries: EXAMPLES
                .iter()
                .map(|example| entry((*example).to_string(), ""))
                .collect(),
        },
    ]
}

/// Every form of a switch as the help lists it, such as `-n N, -max-results N`.
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
/// The width the help wraps text to, so it fits an 80-column console.
const LINE_WIDTH: usize = 79;

/// Writes `entries` as two columns: the usage, and its text wrapped beside it.
fn render(out: &mut String, entries: &[Entry]) {
    let width = entries
        .iter()
        .map(|e| e.usage.len())
        .filter(|&len| len <= USAGE_COLUMN)
        .max()
        .unwrap_or(0)
        + 2;
    for entry in entries {
        let usage = format!("  {}", entry.usage);
        if entry.text.is_empty() {
            out.push_str(&usage);
            out.push('\n');
        } else if entry.usage.len() < width {
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
    use crate::query::{Test, parse_query};

    fn argv(tokens: &[&str]) -> Vec<String> {
        tokens.iter().map(ToString::to_string).collect()
    }

    fn entries(title: &str) -> Vec<String> {
        sections()
            .into_iter()
            .find(|section| section.title.starts_with(title))
            .unwrap_or_else(|| panic!("the help has a {title} section"))
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
                let parsed = parse_query(&plan.query).unwrap_or_else(|e| panic!("{tokens:?}: {e}"));
                assert!(
                    parsed.is_text_empty(),
                    "{tokens:?} searches {:?}",
                    plan.query
                );
            }
            Ok(Command::Help | Command::Version) => {}
            Err(fatal) => panic!("{tokens:?}: {fatal}"),
        }
    }

    /// Every switch, sort key, search function, search term and example the
    /// help lists parses as the help describes it, so the help cannot list
    /// something gozler refuses or searches as text.
    #[test]
    fn everything_the_help_lists_parses() {
        let sort_keys: Vec<String> = entries("SORT KEYS")
            .iter()
            .flat_map(|names| names.split(", ").map(str::to_string).collect::<Vec<_>>())
            .collect();
        for key in &sort_keys {
            for suffix in ["", "-ascending", "-descending"] {
                assert_switch_parses(&["-sort", &format!("{key}{suffix}")]);
                assert_switch_parses(&[&format!("-sort-{key}{suffix}")]);
            }
        }

        let switch_forms: Vec<String> = [entries("SWITCHES"), entries("GOZLER OPTIONS")]
            .concat()
            .iter()
            .flat_map(|usage| usage.split(", ").map(str::to_string).collect::<Vec<_>>())
            .collect();
        for form in &switch_forms {
            let mut tokens: Vec<&str> = form.split(' ').collect();
            match tokens.get(1).copied() {
                None if form.contains("KEY") => {
                    for key in &sort_keys {
                        assert_switch_parses(&[&form.replace("KEY", key)]);
                    }
                    continue;
                }
                None => {}
                Some("N") => tokens[1] = "5",
                Some("MS") => tokens[1] = "1000",
                Some("DIR") => tokens[1] = r"C:\repo",
                Some("FILE") => tokens[1] = "out.csv",
                Some("KEY") => tokens[1] = "name",
                Some(param) => panic!("{form}: the test has no value for {param}"),
            }
            assert_switch_parses(&tokens);
        }

        for usage in entries("SEARCH FUNCTIONS") {
            for form in usage.split(", ") {
                let parsed = parse_query(form).unwrap_or_else(|e| panic!("{form}: {e}"));
                let searched_as_text = parsed
                    .clauses
                    .iter()
                    .flat_map(|clause| &clause.terms)
                    .any(|term| matches!(&term.test, Test::Name(name) if name.contains(':')));
                assert!(
                    !searched_as_text,
                    "{form} is searched as text, not as a function"
                );
            }
        }

        for term in entries("SEARCH TERMS") {
            let parsed = parse_query(&term).unwrap_or_else(|e| panic!("{term}: {e}"));
            assert!(!parsed.is_text_empty(), "{term} searches for nothing");
        }

        for example in entries("EXAMPLES") {
            let tokens: Vec<&str> = example.split(' ').collect();
            assert_eq!(tokens[0], "gozler", "{example}");
            match parse_argv(&argv(&tokens[1..])) {
                Ok(Command::Run(plan)) => {
                    parse_query(&plan.query).unwrap_or_else(|e| panic!("{example}: {e}"));
                }
                other => panic!("{example}: {other:?}"),
            }
        }
    }
}
