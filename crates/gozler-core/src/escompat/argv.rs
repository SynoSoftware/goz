//! es.exe-compatible argv parsing: `parse_argv(args) -> Result<Command, EsFatal>`.
//!
//! Rules implemented from the voidtools es source:
//!
//! - A token starting with `-` or `/` is a switch candidate. Name matching is
//!   case-sensitive and every dash inside the canonical name is
//!   optional in the input (`-sort-path` == `-sortpath`, `-max-results` ==
//!   `-maxresults`). An input dash is accepted only where the canonical name
//!   has one, so `-d-m` does not match `-dm`.
//! - An unknown `-` switch is fatal (exit 6). An unknown `/` token falls
//!   through to the search text verbatim (es source comment: "allow
//!   /downloads to search for \downloads"), except a token that starts with
//!   `/a` or `/o`: the attribute and sort families own every such token, and
//!   refuse one they do not support with exit 4, because as search text it
//!   would match nothing.
//! - A token consisting solely of dashes (`-`, `--`, `---`, ...) is search
//!   text, never a switch (es `es_is_literal_switch`).
//! - Other `--`-prefixed tokens are gozler's native namespace, matched exactly
//!   (case-sensitive, dashes required). Anything else is fatal (exit 6).
//! - A parameter-taking switch consumes the next token; a missing or
//!   unparseable parameter is fatal (exit 4).
//! - The help switches and `-version` end parsing where they appear, as in
//!   es, so a token before them can still fail and a token after them is
//!   ignored.
//! - Non-switch tokens become the query: joined with single spaces, in
//!   command-line order, each token preserved as received. Flags may
//!   interleave with query tokens freely.
//!
//! Every switch and sort key is one row of [`SWITCHES`] or [`SORT_KEYS`],
//! which both the parser and the help read, so a switch the parser accepts is
//! a switch the help lists.
//!
//! Conservative choices where the research is silent (documented, not
//! observed from es): the `-sort` parameter is matched with the same
//! case-sensitive dash-optional rule as the combined `-sort-*` switches (es
//! uses the same name table for both); an unknown `-sort` value and a
//! non-`u32` `-n` value both map to exit 4 ("bad switch parameter" in modern
//! es terms); when several sort switches appear, the last one wins.

use super::ExitCode;
use crate::attributes::Attributes;
use crate::output::{Format, ResultFormat, TableOptions};
use crate::types::{EsColumn, SortDir, SortKey, SortSpec};
use std::borrow::Cow;
use std::time::Duration;

/// What a command line asks gozler to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// No arguments: print the overview, which says what gozler is, whether
    /// gozlerd runs and how to start.
    Overview,
    /// Print a help screen.
    Help(HelpScreen),
    /// `-version`: print the version.
    Version,
    /// Search, or with `--status` report the daemon's state.
    Run(EsPlan),
}

/// Which help screen a command line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpScreen {
    /// `-h`, `-?` or `--h`: common tasks, with one spelling of each common
    /// option.
    Quick,
    /// `--help` or `-help`: every option and alias, search function,
    /// attribute letter, note and exit code.
    Full,
}

/// The fully-resolved plan for one es-compatible invocation.
///
/// Produced by [`parse_argv`]; consumed by the CLI, which resolves
/// process-level concerns (the working directory that
/// [`build_search`](super::build_search) makes folders absolute against,
/// creating the export file, talking to the daemon).
#[derive(Clone, Debug, PartialEq)]
pub struct EsPlan {
    /// Search text: every non-switch token, joined with single spaces in
    /// command-line order, each token preserved as received.
    pub query: String,
    /// The folder of `-path`, `-get-folder-size` or `-parent-path`, from
    /// whichever comes last.
    pub scope: Option<ScopeFolder>,
    /// `-parent DIR`, verbatim: only the entries directly in DIR.
    pub parent: Option<String>,
    /// How many results to show, from `-n`; [`EsPlan::row_limit`] applies it.
    pub limit: Limit,
    /// The columns the column switches ask for, in their order with es dedupe
    /// semantics (a repeated switch moves its column to the end). The writer
    /// adds the Filename column; see [`crate::output::table`].
    pub columns: Vec<EsColumn>,
    /// Result order. Defaults to name-ascending (es with no `-sort` flag);
    /// unsuffixed sort switches use es's per-key default direction.
    pub sort: SortSpec,
    /// How the rows are written: the last of `-csv`, `-tsv`, `-txt` and the
    /// `-export-*` switches, as in es, or plain text without any.
    pub format: Format,
    /// The file of the last `-export-*` switch, verbatim. `None` means write
    /// the results to stdout.
    pub export: Option<String>,
    /// Whether an export file starts with a UTF-8 BOM. Default true: a
    /// documented gozler deviation from stock es (which defaults BOM off).
    /// `-no-utf8-bom` clears it; `-utf8-bom` is accepted and sets it.
    pub bom: bool,
    /// Whether a CSV or TSV includes the header row. `-no-header` clears it.
    pub header: bool,
    /// Whether CSV text columns are always double-quoted (es default).
    /// `-no-double-quote` downgrades to quote-only-when-needed.
    pub double_quote: bool,
    /// `-get-result-count`, `-get-total-size` or `-get-folder-size`, the last
    /// of them: print a count instead of the results.
    pub count: Option<Count>,
    /// Case-sensitive matching, from `-i`, `-case` or `-match-case`. Default
    /// off.
    pub match_case: bool,
    /// `-p` / `-match-path`: text terms match the full path. Default off.
    pub match_path: bool,
    /// `-ww` / `-whole-word`: text terms match whole words. Default off.
    pub whole_word: bool,
    /// The pattern of each `-r` / `-regex`, verbatim, in command-line order;
    /// each must match, as a `regex:` term must.
    pub regexes: Vec<String>,
    /// `-no-result-error`: exit 9 when nothing matches.
    pub no_result_error: bool,
    /// `-timeout MS`: how long to wait for the volumes in scope to become
    /// complete before searching. `None` searches at once.
    pub timeout: Option<Duration>,
    /// Native `--json`: print the results to stdout as JSON, whatever the
    /// format; an export file keeps its format.
    pub json: bool,
    /// Native `--status`: report daemon/index status instead of searching.
    pub status: bool,
    /// Native `--insecure-no-server-check`: skip the pipe-server owner-SID
    /// verification (dev escape hatch).
    pub insecure_no_server_check: bool,
}

impl Default for EsPlan {
    /// The plan for an empty command line: name-ascending sort and es's
    /// default output shape (header on, always-quote on, BOM on per gozler's
    /// deviation).
    fn default() -> Self {
        Self {
            query: String::new(),
            scope: None,
            parent: None,
            limit: Limit::Default,
            columns: Vec::new(),
            sort: SortSpec::default(),
            format: Format::Plain,
            export: None,
            bom: true,
            header: true,
            double_quote: true,
            count: None,
            match_case: false,
            match_path: false,
            whole_word: false,
            regexes: Vec::new(),
            no_result_error: false,
            timeout: None,
            json: false,
            status: false,
            insecure_no_server_check: false,
        }
    }
}

impl EsPlan {
    /// The most rows to ask the daemon for, or `None` for every match.
    ///
    /// Without `-n`, stdout gets [`DEFAULT_LIMIT`] rows, so a query that
    /// forgets `-n` on a volume of a million files cannot flood a console or
    /// an agent's context, while an export file, which floods nothing, gets
    /// every match.
    pub fn row_limit(&self) -> Option<u32> {
        match self.limit {
            Limit::Default if self.export.is_some() => None,
            Limit::Default => Some(DEFAULT_LIMIT),
            Limit::All => None,
            Limit::Rows(rows) => Some(rows),
        }
    }

    /// How the rows are written. Only an export file gets the byte order
    /// mark, as in es, because on stdout it would reach the next program in a
    /// pipe as part of the first line.
    pub fn table_options(&self) -> TableOptions {
        TableOptions {
            format: self.format,
            columns: self.columns.clone(),
            bom: self.bom && self.export.is_some(),
            header: self.header,
            double_quote: self.double_quote,
        }
    }

    /// How the results are written: `--json` prints JSON, except into an
    /// export file, which always holds the export's format.
    pub fn result_format(&self) -> ResultFormat {
        if self.json && self.export.is_none() {
            ResultFormat::Json
        } else {
            ResultFormat::Table(self.table_options())
        }
    }
}

/// The folder a search is below, as the command line names it, verbatim.
/// [`build_search`](super::build_search) makes it absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScopeFolder {
    /// `-path DIR` or `-get-folder-size DIR`: below DIR.
    Dir(String),
    /// `-parent-path PATH`: below the folder that holds PATH.
    ParentOf(String),
}

impl ScopeFolder {
    /// The path as the command line gave it, for the CLI to resolve a
    /// drive-relative one.
    pub fn path_mut(&mut self) -> &mut String {
        match self {
            ScopeFolder::Dir(path) | ScopeFolder::ParentOf(path) => path,
        }
    }
}

/// The rows gozler shows without `-n`.
pub const DEFAULT_LIMIT: u32 = 20;

/// The count a command line prints instead of the results. Each covers every
/// match, whatever `-n` says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Count {
    /// `-get-result-count`: how many entries match.
    Results,
    /// `-get-total-size` or `-get-folder-size`: the summed size of the
    /// matching files in bytes.
    TotalSize,
}

/// How many results a command line asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Limit {
    /// No `-n`.
    Default,
    /// `-n all`.
    All,
    /// `-n N`.
    Rows(u32),
}

/// A command line gozler refuses, with the code it exits with.
///
/// The message names the offending token so the CLI can report it on stderr
/// and exit with `exit_code`.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct EsFatal {
    /// [`ExitCode::UnknownSwitch`] or [`ExitCode::InvalidInput`].
    pub exit_code: ExitCode,
    /// Human-readable description, including the offending token.
    pub message: String,
}

impl EsFatal {
    /// The token looked like a switch but matched nothing.
    fn unknown_switch(token: &str) -> Self {
        Self {
            exit_code: ExitCode::UnknownSwitch,
            message: format!("unknown option: {token}"),
        }
    }

    /// The switch requires a parameter and none followed; `param` is the
    /// help's name for it, such as `DIR`.
    fn missing_param(token: &str, param: &str) -> Self {
        Self {
            exit_code: ExitCode::InvalidInput,
            message: format!("{token} needs {param} after it"),
        }
    }

    /// The switch got a parameter it cannot interpret.
    fn bad_param(token: &str, value: &str, expected: &str) -> Self {
        Self {
            exit_code: ExitCode::InvalidInput,
            message: format!("{token} needs {expected}, not {value}"),
        }
    }

    /// A switch family refuses the token; `reason` completes the sentence
    /// that starts with it.
    fn refused(token: &str, reason: &str) -> Self {
        Self {
            exit_code: ExitCode::InvalidInput,
            message: format!("{token} {reason}"),
        }
    }
}

/// One switch, or one family of switches: how the command line spells it,
/// and its line in the help.
pub(super) struct Switch {
    pub(super) spelling: Spelling,
    pub(super) group: Group,
    pub(super) help: &'static str,
}

/// The section of the full help that lists a [`Switch`], by what the user
/// wants to do.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Group {
    Search,
    LimitAndSort,
    Output,
    Totals,
    Other,
}

/// How a [`Switch`] is spelled.
pub(super) enum Spelling {
    /// `-name` or `/name`, each dash inside a name optional. The first name is
    /// the one the help lists first.
    Es {
        names: &'static [&'static str],
        action: SwitchAction,
    },
    /// A family of es switches with its own parser, such as `-sort-KEY`.
    /// `usage` is the help's form, and `parse` gets the name without its
    /// leading `-` or `/`.
    Family {
        usage: &'static str,
        parse: fn(&str) -> Option<SwitchAction>,
    },
    /// A gozler option, `--name`, matched exactly.
    Native {
        name: &'static str,
        action: SwitchAction,
    },
}

/// What a matched switch does to the plan.
#[derive(Clone, Copy, Debug)]
pub(super) enum SwitchAction {
    /// `-n` / `-max-results` / `-count`: consume the next token as a `u32`
    /// limit or `all`.
    Limit,
    /// `-timeout`: consume the next token as milliseconds to wait.
    Timeout,
    /// `-no-result-error`: exit 9 when nothing matches.
    NoResultError,
    /// `-path`: consume the next token as the scope, verbatim.
    Scope,
    /// `-parent-path`: consume the next token as the path whose folder is
    /// the scope, verbatim.
    ParentPath,
    /// `-parent`: consume the next token as the folder whose direct entries
    /// match, verbatim.
    Parent,
    /// `-csv`, `-tsv`, `-txt`: write the rows in this format.
    Format(Format),
    /// `-export-csv`, `-export-tsv`, `-export-txt`: consume the next token as
    /// the file to write the rows to in this format, verbatim.
    Export(Format),
    /// `-get-result-count` or `-get-total-size`: print this count.
    Count(Count),
    /// `-get-folder-size`: consume the next token as the scope, verbatim,
    /// and print the total size.
    FolderSize,
    /// `/a` and attribute letters: add the query text they stand for.
    Attributes(AttributeSwitch),
    /// A family's token gozler does not support; the text says why, after
    /// the token.
    Refused(&'static str),
    /// `-sort`: consume the next token as a sort-key name with an optional
    /// `-ascending` / `-descending` suffix.
    SortParam,
    /// `-size` / `-dm` / `-dc` and the like: append a column (es dedupe).
    Column(EsColumn),
    /// A combined `-sort-<key>[-ascending|-descending]` switch.
    SortFixed(SortSpec),
    /// `-utf8-bom`: set BOM on (a no-op against gozler's default).
    Utf8Bom,
    /// `-no-utf8-bom`: set BOM off.
    NoUtf8Bom,
    /// `-no-header`: suppress the CSV header row.
    NoHeader,
    /// `-no-double-quote`: quote string columns only when needed.
    NoDoubleQuote,
    /// `-i` / `-case` / `-match-case`: case-sensitive matching.
    MatchCase,
    /// `-p` / `-match-path`: text terms match the full path.
    MatchPath,
    /// `-ww` / `-whole-word`: text terms match whole words.
    WholeWord,
    /// `-r` / `-regex`: consume the next token as a regular expression that
    /// must match, verbatim.
    Regex,
    /// `--json`: print results as JSON.
    Json,
    /// `--status`: report the daemon's state instead of searching.
    Status,
    /// `--insecure-no-server-check`: skip the pipe server's trust check.
    InsecureNoServerCheck,
    /// `-h` / `-?` / `--h` / `-help` / `--help`: stop parsing and print that
    /// help screen.
    Help(HelpScreen),
    /// `-version`: stop parsing and print the version.
    Version,
}

impl SwitchAction {
    /// The help's name for the parameter the switch consumes, or `None` for a
    /// switch without one.
    pub(super) fn param(self) -> Option<&'static str> {
        match self {
            SwitchAction::Limit => Some("N"),
            SwitchAction::Timeout => Some("MS"),
            SwitchAction::Scope | SwitchAction::Parent | SwitchAction::FolderSize => Some("DIR"),
            SwitchAction::ParentPath => Some("PATH"),
            SwitchAction::Export(_) => Some("FILE"),
            SwitchAction::SortParam => Some("KEY"),
            SwitchAction::Regex => Some("PATTERN"),
            SwitchAction::Column(_)
            | SwitchAction::Format(_)
            | SwitchAction::Count(_)
            | SwitchAction::Attributes(_)
            | SwitchAction::Refused(_)
            | SwitchAction::SortFixed(_)
            | SwitchAction::Utf8Bom
            | SwitchAction::NoUtf8Bom
            | SwitchAction::NoHeader
            | SwitchAction::NoDoubleQuote
            | SwitchAction::MatchCase
            | SwitchAction::MatchPath
            | SwitchAction::WholeWord
            | SwitchAction::NoResultError
            | SwitchAction::Json
            | SwitchAction::Status
            | SwitchAction::InsecureNoServerCheck
            | SwitchAction::Help(_)
            | SwitchAction::Version => None,
        }
    }
}

/// Every switch gozler accepts, in the order the help lists them within
/// each [`Group`].
pub(super) const SWITCHES: &[Switch] = &[
    Switch {
        spelling: Spelling::Es {
            names: &["path"],
            action: SwitchAction::Scope,
        },
        group: Group::Search,
        help: "Search only below DIR",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["parent-path"],
            action: SwitchAction::ParentPath,
        },
        group: Group::Search,
        help: "Search below the folder that contains PATH",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["parent"],
            action: SwitchAction::Parent,
        },
        group: Group::Search,
        help: "Find only the files and folders directly in DIR",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["i", "case", "match-case"],
            action: SwitchAction::MatchCase,
        },
        group: Group::Search,
        help: "Match case",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["p", "match-path"],
            action: SwitchAction::MatchPath,
        },
        group: Group::Search,
        help: "Match text against the full path, not only the name",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["ww", "whole-word", "whole-words"],
            action: SwitchAction::WholeWord,
        },
        group: Group::Search,
        help: "Match whole words only",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["r", "regex"],
            action: SwitchAction::Regex,
        },
        group: Group::Search,
        help: "Also match PATTERN as a regular expression, as `regex:` does; spaces \
               and quotes in PATTERN are part of it",
    },
    Switch {
        spelling: Spelling::Family {
            usage: "/aLETTERS, /a-LETTERS",
            parse: parse_attribute_switch,
        },
        group: Group::Search,
        help: "Find results that have every attribute in LETTERS and none of the \
               attributes after a `-`; `/ad` finds folders, `/a-d` files",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["n", "max-results", "count"],
            action: SwitchAction::Limit,
        },
        group: Group::LimitAndSort,
        help: "Show at most N results (default 20); `-n all` shows every result",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["sort"],
            action: SwitchAction::SortParam,
        },
        group: Group::LimitAndSort,
        help: "Sort by KEY: name, path, size, dm or extension; add `-ascending` \
               or `-descending` to KEY to set the order, as in `size-ascending`",
    },
    Switch {
        spelling: Spelling::Family {
            usage: "-sort-KEY",
            parse: parse_sort_switch,
        },
        group: Group::LimitAndSort,
        help: "Same as `-sort KEY`, as in `-sort-size-ascending`",
    },
    Switch {
        spelling: Spelling::Family {
            usage: "/on, /o-n, /os, /o-s, /od, /o-d, /oe, /o-e",
            parse: parse_order_switch,
        },
        group: Group::LimitAndSort,
        help: "Sort as dir does by name, size, date or extension: A to Z, smallest \
               or oldest first; `/o-` reverses the order",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["csv"],
            action: SwitchAction::Format(Format::Csv),
        },
        group: Group::Output,
        help: "Print the results as CSV",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["tsv"],
            action: SwitchAction::Format(Format::Tsv),
        },
        group: Group::Output,
        help: "Print the results as tab-separated values",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["txt"],
            action: SwitchAction::Format(Format::Txt),
        },
        group: Group::Output,
        help: "Print only the full paths, with CRLF line endings",
    },
    Switch {
        spelling: Spelling::Native {
            name: "json",
            action: SwitchAction::Json,
        },
        group: Group::Output,
        help: "Print the results, or the `--status` report, as JSON",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["export-csv"],
            action: SwitchAction::Export(Format::Csv),
        },
        group: Group::Output,
        help: "Write all results to FILE as CSV, or the first N with `-n`",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["export-tsv"],
            action: SwitchAction::Export(Format::Tsv),
        },
        group: Group::Output,
        help: "Write all results to FILE as tab-separated values, or the first \
               N with `-n`",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["export-txt"],
            action: SwitchAction::Export(Format::Txt),
        },
        group: Group::Output,
        help: "Write all full paths to FILE, or the first N with `-n`",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["name"],
            action: SwitchAction::Column(EsColumn::Name),
        },
        group: Group::Output,
        help: "Add a Name column: the name without its folder",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["path-column"],
            action: SwitchAction::Column(EsColumn::Path),
        },
        group: Group::Output,
        help: "Add a Path column: the folder that contains the result",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["full-path-and-name", "filename-column"],
            action: SwitchAction::Column(EsColumn::FullPath),
        },
        group: Group::Output,
        help: "Put the Filename column (the full path) here; without this option, \
               gozler adds it last unless `-name` or `-path-column` is given",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["extension", "ext"],
            action: SwitchAction::Column(EsColumn::Extension),
        },
        group: Group::Output,
        help: "Add an Extension column",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["size"],
            action: SwitchAction::Column(EsColumn::Size),
        },
        group: Group::Output,
        help: "Add a Size column, in bytes; for a folder, the files below it",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["dm", "date-modified"],
            action: SwitchAction::Column(EsColumn::DateModified),
        },
        group: Group::Output,
        help: "Add a Date Modified column",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["dc", "date-created"],
            action: SwitchAction::Column(EsColumn::DateCreated),
        },
        group: Group::Output,
        help: "Add a Date Created column; needs Dates created in the index settings",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["attributes", "attribs", "attrib"],
            action: SwitchAction::Column(EsColumn::Attributes),
        },
        group: Group::Output,
        help: "Add an Attributes column",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["utf8-bom"],
            action: SwitchAction::Utf8Bom,
        },
        group: Group::Output,
        help: "Start an export file with a UTF-8 byte order mark (default)",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-utf8-bom"],
            action: SwitchAction::NoUtf8Bom,
        },
        group: Group::Output,
        help: "Leave out the byte order mark",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-header"],
            action: SwitchAction::NoHeader,
        },
        group: Group::Output,
        help: "Leave out the CSV or TSV header row",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-double-quote"],
            action: SwitchAction::NoDoubleQuote,
        },
        group: Group::Output,
        help: "Quote CSV text only when it needs quotes",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["get-result-count"],
            action: SwitchAction::Count(Count::Results),
        },
        group: Group::Totals,
        help: "Print only the number of matches",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["get-total-size"],
            action: SwitchAction::Count(Count::TotalSize),
        },
        group: Group::Totals,
        help: "Print only the total size of the matching files, in bytes",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["get-folder-size"],
            action: SwitchAction::FolderSize,
        },
        group: Group::Totals,
        help: "Print only the total size of the files below DIR, in bytes",
    },
    Switch {
        spelling: Spelling::Native {
            name: "status",
            action: SwitchAction::Status,
        },
        group: Group::Other,
        help: "Show whether gozlerd and each volume are ready, instead of searching",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["timeout"],
            action: SwitchAction::Timeout,
        },
        group: Group::Other,
        help: "Wait up to MS milliseconds for indexing to finish, then search",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-result-error"],
            action: SwitchAction::NoResultError,
        },
        group: Group::Other,
        help: "Exit 9 when nothing matches",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["h", "?"],
            action: SwitchAction::Help(HelpScreen::Quick),
        },
        group: Group::Other,
        help: "Show the quick help",
    },
    Switch {
        spelling: Spelling::Native {
            name: "h",
            action: SwitchAction::Help(HelpScreen::Quick),
        },
        group: Group::Other,
        help: "Same as `-h`",
    },
    Switch {
        spelling: Spelling::Native {
            name: "help",
            action: SwitchAction::Help(HelpScreen::Full),
        },
        group: Group::Other,
        help: "Show the full reference",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["help"],
            action: SwitchAction::Help(HelpScreen::Full),
        },
        group: Group::Other,
        help: "Same as `--help`",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["v", "version"],
            action: SwitchAction::Version,
        },
        group: Group::Other,
        help: "Show the version",
    },
    Switch {
        spelling: Spelling::Native {
            name: "version",
            action: SwitchAction::Version,
        },
        group: Group::Other,
        help: "Same as `-v`",
    },
    Switch {
        spelling: Spelling::Native {
            name: "insecure-no-server-check",
            action: SwitchAction::InsecureNoServerCheck,
        },
        group: Group::Other,
        help: "Skip the check that the pipe server is gozlerd (testing only)",
    },
];

/// One sort key: its spellings, accepted by `-sort KEY` and `-sort-KEY`, and
/// its line in the help.
pub(super) struct SortKeyName {
    /// The first name is the one the help lists first.
    pub(super) names: &'static [&'static str],
    pub(super) key: SortKey,
    pub(super) help: &'static str,
}

/// Every sort key, in the order the help lists them. A key without a
/// direction suffix takes [`SortSpec::default_for`].
pub(super) const SORT_KEYS: &[SortKeyName] = &[
    SortKeyName {
        names: &["name"],
        key: SortKey::Name,
        help: "Name, A to Z (default)",
    },
    SortKeyName {
        names: &["path"],
        key: SortKey::Path,
        help: "Folder, then name, A to Z",
    },
    SortKeyName {
        names: &["size"],
        key: SortKey::Size,
        help: "Size, largest first",
    },
    SortKeyName {
        names: &["dm", "date-modified"],
        key: SortKey::DateModified,
        help: "Date modified, newest first",
    },
    SortKeyName {
        names: &["dc", "date-created"],
        key: SortKey::DateCreated,
        help: "Date created, newest first; needs Dates created",
    },
    SortKeyName {
        names: &["extension"],
        key: SortKey::Extension,
        help: "Extension, A to Z; folders and names without one come first",
    },
];

/// Parses an argv slice (excluding `argv[0]`) into a [`Command`].
///
/// Pure and total: no I/O, no environment access, no panics on any input.
/// A refused command line returns [`EsFatal`] with its exit code.
///
/// ```
/// use gozler_core::escompat::{Command, parse_argv};
///
/// let args: Vec<String> = ["report", "-sortpath", "-n", "10"]
///     .iter()
///     .map(ToString::to_string)
///     .collect();
/// let Ok(Command::Run(plan)) = parse_argv(&args) else {
///     panic!("a search");
/// };
/// assert_eq!(plan.query, "report");
/// assert_eq!(plan.row_limit(), Some(10));
/// ```
pub fn parse_argv(args: &[String]) -> Result<Command, EsFatal> {
    if args.is_empty() {
        return Ok(Command::Overview);
    }
    let mut plan = EsPlan::default();
    let mut query: Vec<Cow<str>> = Vec::new();
    let mut rest = args.iter();

    while let Some(token) = rest.next() {
        let token = token.as_str();

        // es literal-switch rule: all-dash tokens are search text.
        if !token.is_empty() && token.bytes().all(|b| b == b'-') {
            query.push(token.into());
            continue;
        }

        let action = if let Some(native) = token.strip_prefix("--") {
            match_native(native).ok_or_else(|| EsFatal::unknown_switch(token))?
        } else if let Some(name) = token.strip_prefix('-') {
            match_switch(name).ok_or_else(|| EsFatal::unknown_switch(token))?
        } else if let Some(name) = token.strip_prefix('/') {
            match match_switch(name) {
                Some(action) => action,
                // es: unknown `/` tokens search for the text verbatim.
                None => {
                    query.push(token.into());
                    continue;
                }
            }
        } else {
            query.push(token.into());
            continue;
        };

        match action {
            SwitchAction::Help(screen) => return Ok(Command::Help(screen)),
            SwitchAction::Version => return Ok(Command::Version),
            SwitchAction::Attributes(switch) => query.push(switch.query_text().into()),
            action => apply_switch(action, token, &mut rest, &mut plan)?,
        }
    }

    plan.query = query.join(" ");
    Ok(Command::Run(plan))
}

/// Resolves a name without its `--` against the gozler options.
fn match_native(name: &str) -> Option<SwitchAction> {
    SWITCHES.iter().find_map(|switch| match switch.spelling {
        Spelling::Native {
            name: native,
            action,
        } if native == name => Some(action),
        _ => None,
    })
}

/// Resolves a name without its `-` or `/` against the es switches, then the
/// families. Returns `None` when nothing matches (caller decides fatal vs
/// fallthrough).
fn match_switch(name: &str) -> Option<SwitchAction> {
    let exact = SWITCHES.iter().find_map(|switch| match switch.spelling {
        Spelling::Es { names, action } => names
            .iter()
            .any(|canonical| switch_name_matches(name, canonical))
            .then_some(action),
        _ => None,
    });
    exact.or_else(|| {
        SWITCHES.iter().find_map(|switch| match switch.spelling {
            Spelling::Family { parse, .. } => parse(name),
            _ => None,
        })
    })
}

/// Parses the `-sort-KEY[-ascending|-descending]` family: `sort`, then the
/// dash that switch names make optional, then a `-sort` parameter.
fn parse_sort_switch(name: &str) -> Option<SwitchAction> {
    let rest = name.strip_prefix("sort")?;
    parse_sort_param(rest.strip_prefix('-').unwrap_or(rest)).map(SwitchAction::SortFixed)
}

/// Parses the `/o` family, es's dir-style sorts: `/on`, `/os`, `/od` and
/// `/oe` ascending, and with `-` after the `o` descending.
fn parse_order_switch(name: &str) -> Option<SwitchAction> {
    let rest = name.strip_prefix('o')?;
    let (dir, letter) = match rest.strip_prefix('-') {
        Some(letter) => (SortDir::Desc, letter),
        None => (SortDir::Asc, rest),
    };
    let key = match letter {
        "n" => SortKey::Name,
        "s" => SortKey::Size,
        "d" => SortKey::DateModified,
        "e" => SortKey::Extension,
        _ => {
            return Some(SwitchAction::Refused(
                "is not a sort; use /on, /os, /od or /oe, or /o-n, /o-s, /o-d or /o-e \
                 to reverse",
            ));
        }
    };
    Some(SwitchAction::SortFixed(SortSpec { key, dir }))
}

/// Parses the `/a` family, es's dir-style attributes: each letter, in either
/// case, is an attribute every result has, and with `-` before it one no
/// result has.
fn parse_attribute_switch(name: &str) -> Option<SwitchAction> {
    const NOT_ATTRIBUTES: SwitchAction = SwitchAction::Refused(
        "is not an attribute option: write /a and attribute letters, with - before a \
         letter to leave that attribute out, such as /ah or /a-h; gozler --help \
         lists the letters",
    );
    let mut chars = name.strip_prefix('a')?.chars();
    let (mut required, mut excluded) = (String::new(), String::new());
    while let Some(next) = chars.next() {
        match next {
            '-' => excluded.extend(chars.next()),
            letter => required.push(letter),
        }
    }
    let (Ok(required), Ok(excluded)) = (
        Attributes::from_letters(&required),
        Attributes::from_letters(&excluded),
    ) else {
        return Some(NOT_ATTRIBUTES);
    };
    // Nothing to test, a `-` without its letter, or a letter both wanted
    // and left out.
    if (required.is_empty() && excluded.is_empty())
        || name.ends_with('-')
        || required.without(excluded) != required
    {
        return Some(NOT_ATTRIBUTES);
    }
    Some(SwitchAction::Attributes(AttributeSwitch {
        required,
        excluded,
    }))
}

/// `/a` and its letters: the attributes every result has, and those no
/// result has.
#[derive(Clone, Copy, Debug)]
pub(super) struct AttributeSwitch {
    required: Attributes,
    excluded: Attributes,
}

impl AttributeSwitch {
    /// The query text: D as `folder:` or `file:`, the other letters every
    /// result has as one `attrib:`, and each letter no result has as its own
    /// `!attrib:`, so that `/a-h-s`, as in dir, leaves out hidden files and
    /// system files, not only files that are both.
    fn query_text(self) -> String {
        let folder = Attributes::from_letters("d").expect("D is an attribute letter");
        let mut words = Vec::new();
        if self.required.contains(folder) {
            words.push("folder:".to_string());
        }
        if self.excluded.contains(folder) {
            words.push("file:".to_string());
        }
        let required = self.required.without(folder);
        if !required.is_empty() {
            words.push(format!("attrib:{}", required.letters()));
        }
        let excluded = self.excluded.without(folder).letters();
        words.extend(excluded.chars().map(|letter| format!("!attrib:{letter}")));
        words.join(" ")
    }
}

/// Applies one matched switch, consuming its parameter from `rest` if it
/// takes one. `token` is the switch as the user typed it (for messages).
/// [`parse_argv`] handles [`SwitchAction::Help`], [`SwitchAction::Version`]
/// and [`SwitchAction::Attributes`], which never reach here.
fn apply_switch(
    action: SwitchAction,
    token: &str,
    rest: &mut core::slice::Iter<'_, String>,
    plan: &mut EsPlan,
) -> Result<(), EsFatal> {
    match action {
        SwitchAction::Limit => {
            let value = take_param(rest, token, action)?;
            plan.limit = match value {
                "all" => Limit::All,
                _ => Limit::Rows(
                    value
                        .parse()
                        .map_err(|_| EsFatal::bad_param(token, value, "a number or all"))?,
                ),
            };
        }
        SwitchAction::Timeout => {
            let value = take_param(rest, token, action)?;
            let ms: u32 = value
                .parse()
                .map_err(|_| EsFatal::bad_param(token, value, "milliseconds"))?;
            plan.timeout = Some(Duration::from_millis(ms.into()));
        }
        SwitchAction::NoResultError => plan.no_result_error = true,
        SwitchAction::Scope => {
            plan.scope = Some(ScopeFolder::Dir(
                take_param(rest, token, action)?.to_owned(),
            ));
        }
        SwitchAction::ParentPath => {
            plan.scope = Some(ScopeFolder::ParentOf(
                take_param(rest, token, action)?.to_owned(),
            ));
        }
        SwitchAction::Parent => plan.parent = Some(take_param(rest, token, action)?.to_owned()),
        SwitchAction::Format(format) => plan.format = format,
        SwitchAction::Export(format) => {
            plan.export = Some(take_param(rest, token, action)?.to_owned());
            plan.format = format;
        }
        SwitchAction::Count(count) => plan.count = Some(count),
        SwitchAction::FolderSize => {
            plan.scope = Some(ScopeFolder::Dir(
                take_param(rest, token, action)?.to_owned(),
            ));
            plan.count = Some(Count::TotalSize);
        }
        SwitchAction::Refused(reason) => return Err(EsFatal::refused(token, reason)),
        SwitchAction::SortParam => {
            let value = take_param(rest, token, action)?;
            plan.sort = parse_sort_param(value).ok_or_else(|| {
                EsFatal::bad_param(token, value, "a sort key such as name, size or dm")
            })?;
        }
        SwitchAction::Column(column) => {
            // es `es_add_column`: remove an existing instance, then append,
            // so the last occurrence determines the column's position.
            plan.columns.retain(|existing| *existing != column);
            plan.columns.push(column);
        }
        SwitchAction::SortFixed(spec) => plan.sort = spec,
        SwitchAction::Utf8Bom => plan.bom = true,
        SwitchAction::NoUtf8Bom => plan.bom = false,
        SwitchAction::NoHeader => plan.header = false,
        SwitchAction::NoDoubleQuote => plan.double_quote = false,
        SwitchAction::MatchCase => plan.match_case = true,
        SwitchAction::MatchPath => plan.match_path = true,
        SwitchAction::WholeWord => plan.whole_word = true,
        SwitchAction::Regex => plan
            .regexes
            .push(take_param(rest, token, action)?.to_owned()),
        SwitchAction::Json => plan.json = true,
        SwitchAction::Status => plan.status = true,
        SwitchAction::InsecureNoServerCheck => plan.insecure_no_server_check = true,
        SwitchAction::Help(_) | SwitchAction::Version | SwitchAction::Attributes(_) => {
            unreachable!("parse_argv handles -help, -version and search text")
        }
    }
    Ok(())
}

/// Consumes the next token as the parameter of `action`, or fails with exit
/// 4 naming the switch as the user typed it.
fn take_param<'a>(
    rest: &mut core::slice::Iter<'a, String>,
    token: &str,
    action: SwitchAction,
) -> Result<&'a str, EsFatal> {
    rest.next().map(String::as_str).ok_or_else(|| {
        let param = action
            .param()
            .expect("a switch that takes a parameter names it");
        EsFatal::missing_param(token, param)
    })
}

/// Resolves a `-sort` parameter value (`name`, `dm`, `date-modified`, ...
/// optionally suffixed `-ascending` / `-descending`) into a [`SortSpec`].
/// Uses the same dash-optional matching as switch names.
fn parse_sort_param(value: &str) -> Option<SortSpec> {
    SORT_KEYS.iter().find_map(|row| {
        row.names
            .iter()
            .find_map(|alias| match_sort_form(value, alias, row.key))
    })
}

/// Matches `input` against `base`, `base-ascending`, or `base-descending`
/// (interior dashes optional). Unsuffixed forms take the key's es default
/// direction ([`SortSpec::default_for`]).
fn match_sort_form(input: &str, base: &str, key: SortKey) -> Option<SortSpec> {
    if switch_name_matches(input, base) {
        return Some(SortSpec::default_for(key));
    }
    if switch_name_matches(input, &format!("{base}-ascending")) {
        return Some(SortSpec {
            key,
            dir: SortDir::Asc,
        });
    }
    if switch_name_matches(input, &format!("{base}-descending")) {
        return Some(SortSpec {
            key,
            dir: SortDir::Desc,
        });
    }
    None
}

/// es switch-name comparison (`es_check_param`): case-sensitive, and every
/// dash in the canonical name optionally consumes one dash from the input.
/// Input dashes anywhere else fail the match.
fn switch_name_matches(input: &str, canonical: &str) -> bool {
    let mut input_chars = input.chars().peekable();
    for expected in canonical.chars() {
        if expected == '-' {
            if input_chars.peek() == Some(&'-') {
                input_chars.next();
            }
        } else if input_chars.next() != Some(expected) {
            return false;
        }
    }
    input_chars.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(tokens: &[&str]) -> Result<Command, EsFatal> {
        let args: Vec<String> = tokens.iter().map(ToString::to_string).collect();
        parse_argv(&args)
    }

    fn plan(tokens: &[&str]) -> EsPlan {
        match parse(tokens).expect("expected a successful parse") {
            Command::Run(plan) => plan,
            other => panic!("expected a search, got {other:?}"),
        }
    }

    fn fatal(tokens: &[&str]) -> EsFatal {
        parse(tokens).expect_err("expected a fatal parse error")
    }

    use proptest::prelude::*;

    proptest! {
        /// The argv parser is total: for ANY input it returns Ok or a fatal
        /// `EsFatal`, never panicking (its documented contract).
        #[test]
        fn parse_argv_never_panics_on_arbitrary_input(
            args in proptest::collection::vec(".*", 0..12),
        ) {
            let _ = parse_argv(&args);
        }

        /// Flag-shaped tokens (leading `-`/`/` plus arbitrary text, missing
        /// switch parameters, empty tokens) also never panic.
        #[test]
        fn parse_argv_never_panics_on_flaglike_input(
            args in proptest::collection::vec(
                prop_oneof!["-[a-zA-Z-]{0,16}", "/[a-zA-Z-]{0,16}", "[^ ]{0,16}"],
                0..12,
            ),
        ) {
            let _ = parse_argv(&args);
        }
    }

    /// Bare `gozler` prints the overview instead of listing every file; a
    /// command line of options alone still searches.
    #[test]
    fn empty_argv_is_the_overview() {
        assert_eq!(parse(&[]), Ok(Command::Overview));
        assert_eq!(plan(&["-n", "all"]).query, "");
    }

    #[test]
    fn app_reference_invocation_full_plan() {
        let parsed = plan(&[
            "-path",
            "C:\\some dir",
            "report",
            "-n",
            "200",
            "-size",
            "-dm",
            "-sort-path",
            "-export-csv",
            "C:\\tmp\\out.csv",
        ]);
        assert_eq!(
            parsed,
            EsPlan {
                query: "report".to_string(),
                scope: Some(ScopeFolder::Dir("C:\\some dir".to_string())),
                parent: None,
                limit: Limit::Rows(200),
                columns: vec![EsColumn::Size, EsColumn::DateModified],
                sort: SortSpec {
                    key: SortKey::Path,
                    dir: SortDir::Asc,
                },
                format: Format::Csv,
                export: Some("C:\\tmp\\out.csv".to_string()),
                bom: true,
                header: true,
                double_quote: true,
                count: None,
                match_case: false,
                match_path: false,
                whole_word: false,
                regexes: Vec::new(),
                no_result_error: false,
                timeout: None,
                json: false,
                status: false,
                insecure_no_server_check: false,
            }
        );
    }

    #[test]
    fn interior_dashes_are_optional_everywhere() {
        assert_eq!(
            plan(&["-sortpath"]).sort,
            SortSpec::default_for(SortKey::Path)
        );
        assert_eq!(plan(&["-maxresults", "7"]).limit, Limit::Rows(7));
        assert!(!plan(&["-noutf8bom"]).bom);
        assert!(!plan(&["-nodoublequote"]).double_quote);
        assert_eq!(
            plan(&["-datemodified"]).columns,
            vec![EsColumn::DateModified]
        );
        assert_eq!(
            plan(&["-sortpathascending"]).sort,
            SortSpec {
                key: SortKey::Path,
                dir: SortDir::Asc,
            }
        );
        assert_eq!(
            plan(&["-sortdatemodifieddescending"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
        // Partial omission is fine too: any subset of dashes may be dropped.
        assert!(!plan(&["-no-utf8bom"]).bom);
        assert_eq!(plan(&["-max-results", "3"]).limit, Limit::Rows(3));
    }

    #[test]
    fn switch_matching_is_case_sensitive() {
        assert_eq!(fatal(&["-SORT-PATH"]).exit_code, ExitCode::UnknownSwitch);
        assert_eq!(fatal(&["-N", "5"]).exit_code, ExitCode::UnknownSwitch);
    }

    #[test]
    fn input_dash_where_canonical_has_none_is_unknown() {
        assert_eq!(fatal(&["-d-m"]).exit_code, ExitCode::UnknownSwitch);
        assert_eq!(fatal(&["-si-ze"]).exit_code, ExitCode::UnknownSwitch);
        assert_eq!(fatal(&["-no--header"]).exit_code, ExitCode::UnknownSwitch);
    }

    #[test]
    fn unknown_dash_switch_is_exit_6_with_token() {
        let err = fatal(&["-zzz"]);
        assert_eq!(err.exit_code, ExitCode::UnknownSwitch);
        assert!(err.message.contains("-zzz"), "message was: {}", err.message);
    }

    #[test]
    fn slash_token_without_match_falls_through_to_query() {
        assert_eq!(plan(&["/nomatch"]).query, "/nomatch");
        assert_eq!(plan(&["report", "/downloads"]).query, "report /downloads");
    }

    /// Bug 3: `/ad` and `/a-d` become `folder:` and `file:`, the letter in
    /// either case. The other letters become `attrib:`, each excluded letter
    /// its own `!attrib:`, and an `/a` token that names no attributes exits 4
    /// instead of becoming search text that matches nothing.
    #[test]
    fn attribute_switches_become_attribute_functions_or_exit_4() {
        assert_eq!(plan(&["/a-d", "report"]).query, "file: report");
        assert_eq!(plan(&["report", "/aD"]).query, "report folder:");
        assert_eq!(
            plan(&["/as-r-hd", "x"]).query,
            "folder: attrib:S !attrib:R !attrib:H x"
        );
        for refused in ["/a", "/ad-d", "/a1", "/a-", "/ah-", "/a--h", "/ab"] {
            assert_eq!(
                fatal(&[refused]).exit_code,
                ExitCode::InvalidInput,
                "{refused}"
            );
        }
    }

    /// es's `/o` sorts run ascending, smallest and oldest first, unlike
    /// `-sort size` and `-sort dm`; `-` reverses them, and every other `/o`
    /// token exits 4.
    #[test]
    fn order_switches_map_to_sort_keys() {
        assert_eq!(
            plan(&["/os"]).sort,
            SortSpec {
                key: SortKey::Size,
                dir: SortDir::Asc,
            }
        );
        assert_eq!(
            plan(&["/o-d"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["/o-e"]).sort,
            SortSpec {
                key: SortKey::Extension,
                dir: SortDir::Desc,
            }
        );
        for refused in ["/ox", "/o"] {
            assert_eq!(
                fatal(&[refused]).exit_code,
                ExitCode::InvalidInput,
                "{refused}"
            );
        }
    }

    /// As in es, the last format switch wins and an export switch also names
    /// the file; only an export file starts with the byte order mark.
    #[test]
    fn the_last_format_switch_wins() {
        let tsv_file = plan(&["-export-csv", "out.csv", "-tsv"]);
        assert_eq!(
            (tsv_file.format, tsv_file.export.as_deref()),
            (Format::Tsv, Some("out.csv"))
        );
        assert!(tsv_file.table_options().bom);
        let csv_stdout = plan(&["-txt", "-csv"]);
        assert_eq!(
            (csv_stdout.format, &csv_stdout.export),
            (Format::Csv, &None)
        );
        assert!(!csv_stdout.table_options().bom);
    }

    /// `-get-folder-size DIR` is `-path DIR -get-total-size`, and the last
    /// scope and the last count switch win, as with the other switches.
    #[test]
    fn the_folder_size_is_the_total_size_below_a_folder() {
        let folder = plan(&["-get-folder-size", r"C:\repo"]);
        assert_eq!(
            (folder.scope, folder.count),
            (
                Some(ScopeFolder::Dir(r"C:\repo".to_string())),
                Some(Count::TotalSize)
            )
        );
        let last = plan(&["-get-folder-size", "a", "-path", "b", "-get-result-count"]);
        assert_eq!(
            (last.scope, last.count),
            (
                Some(ScopeFolder::Dir("b".to_string())),
                Some(Count::Results)
            )
        );
    }

    #[test]
    fn slash_token_with_match_is_a_switch() {
        // es's es_check_param treats `-` and `/` prefixes identically,
        // so /n is the -n switch, not search text.
        let parsed = plan(&["/n", "50"]);
        assert_eq!(parsed.limit, Limit::Rows(50));
        assert_eq!(parsed.query, "");
        // A matched `/` switch with a missing parameter is exit 4 like `-`.
        assert_eq!(fatal(&["/path"]).exit_code, ExitCode::InvalidInput);
    }

    #[test]
    fn missing_parameter_is_exit_4_naming_the_switch() {
        for switch in ["-n", "-path", "-export-csv", "-sort", "-maxresults"] {
            let err = fatal(&[switch]);
            assert_eq!(err.exit_code, ExitCode::InvalidInput, "switch: {switch}");
            assert!(
                err.message.contains(switch),
                "message for {switch} was: {}",
                err.message
            );
        }
    }

    #[test]
    fn non_numeric_count_is_exit_4() {
        assert_eq!(fatal(&["-n", "abc"]).exit_code, ExitCode::InvalidInput);
        assert_eq!(fatal(&["-n", "-5"]).exit_code, ExitCode::InvalidInput);
        // Overflowing u32 is a bad parameter, not a silent wrap.
        assert_eq!(
            fatal(&["-n", "4294967296"]).exit_code,
            ExitCode::InvalidInput
        );
    }

    /// Stdout gets 20 rows unless -n says otherwise, and an export file every
    /// match; -n all lifts the limit, and -count is es's alias of -n.
    #[test]
    fn the_limit_defaults_to_20_rows_on_stdout() {
        let row_limit = |tokens: &[&str]| plan(tokens).row_limit();
        assert_eq!(row_limit(&["report"]), Some(20));
        assert_eq!(row_limit(&["-export-csv", "out.csv", "report"]), None);
        assert_eq!(row_limit(&["-n", "all", "report"]), None);
        assert_eq!(row_limit(&["-count", "0", "report"]), Some(0));
        assert_eq!(
            row_limit(&["-export-csv", "out.csv", "-n", "5", "report"]),
            Some(5)
        );
        assert_eq!(fatal(&["-n", "ALL"]).exit_code, ExitCode::InvalidInput);
    }

    #[test]
    fn timeout_takes_milliseconds() {
        assert_eq!(
            plan(&["-timeout", "1500"]).timeout,
            Some(Duration::from_millis(1500))
        );
        assert_eq!(
            fatal(&["-timeout", "soon"]).exit_code,
            ExitCode::InvalidInput
        );
    }

    #[test]
    fn sort_param_defaults_direction_per_key() {
        assert_eq!(
            plan(&["-sort", "dm"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort", "size"]).sort,
            SortSpec {
                key: SortKey::Size,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort", "name"]).sort,
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Asc,
            }
        );
        assert_eq!(
            plan(&["-sort", "path"]).sort,
            SortSpec {
                key: SortKey::Path,
                dir: SortDir::Asc,
            }
        );
    }

    #[test]
    fn sort_param_with_direction_suffix() {
        assert_eq!(
            plan(&["-sort", "name-descending"]).sort,
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort", "date-modified-ascending"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Asc,
            }
        );
        // The parameter uses the same dash-optional matching as switch names.
        assert_eq!(
            plan(&["-sort", "namedescending"]).sort,
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort", "datemodified"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
    }

    #[test]
    fn sort_param_unknown_key_is_exit_4() {
        let err = fatal(&["-sort", "bogus"]);
        assert_eq!(err.exit_code, ExitCode::InvalidInput);
        assert!(
            err.message.contains("bogus"),
            "message was: {}",
            err.message
        );
    }

    #[test]
    fn combined_sort_switches() {
        assert_eq!(
            plan(&["-sort-name"]).sort,
            SortSpec::default_for(SortKey::Name)
        );
        assert_eq!(
            plan(&["-sort-size"]).sort,
            SortSpec {
                key: SortKey::Size,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort-dm"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort-date-modified"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort-dm-descending"]).sort,
            SortSpec {
                key: SortKey::DateModified,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort-size-ascending"]).sort,
            SortSpec {
                key: SortKey::Size,
                dir: SortDir::Asc,
            }
        );
    }

    #[test]
    fn last_sort_switch_wins() {
        assert_eq!(
            plan(&["-sort-path", "-sort", "size"]).sort,
            SortSpec {
                key: SortKey::Size,
                dir: SortDir::Desc,
            }
        );
        assert_eq!(
            plan(&["-sort", "size", "-sortpath"]).sort,
            SortSpec {
                key: SortKey::Path,
                dir: SortDir::Asc,
            }
        );
    }

    #[test]
    fn query_tokens_join_with_single_spaces_around_interleaved_flags() {
        let parsed = plan(&["foo", "-size", "bar", "-n", "5", "baz qux"]);
        assert_eq!(parsed.query, "foo bar baz qux");
        assert_eq!(parsed.limit, Limit::Rows(5));
        assert_eq!(parsed.columns, vec![EsColumn::Size]);
    }

    #[test]
    fn columns_dedupe_and_reorder_like_es() {
        assert_eq!(plan(&["-size", "-size"]).columns, vec![EsColumn::Size]);
        // es removes the existing instance then appends: last occurrence
        // determines position.
        assert_eq!(
            plan(&["-size", "-dm", "-size"]).columns,
            vec![EsColumn::DateModified, EsColumn::Size]
        );
    }

    #[test]
    fn native_namespace_flags() {
        assert!(plan(&["--json"]).json);
        assert!(plan(&["--status"]).status);
        assert!(plan(&["--insecure-no-server-check"]).insecure_no_server_check);
        let all = plan(&["--json", "--status"]);
        assert!(all.json && all.status);
    }

    #[test]
    fn native_namespace_unknown_is_exit_6() {
        let err = fatal(&["--bogus"]);
        assert_eq!(err.exit_code, ExitCode::UnknownSwitch);
        assert!(
            err.message.contains("--bogus"),
            "message was: {}",
            err.message
        );
        // Native names are exact: no dash-optionality in the -- namespace.
        assert_eq!(
            fatal(&["--insecureno-server-check"]).exit_code,
            ExitCode::UnknownSwitch
        );
    }

    /// As in es, `-help` and `-version` end parsing where they appear: a bad
    /// token before them still fails, and one after them is never read.
    #[test]
    fn help_and_version_end_parsing_where_they_appear() {
        let quick = Ok(Command::Help(HelpScreen::Quick));
        let full = Ok(Command::Help(HelpScreen::Full));
        assert_eq!(parse(&["report", "-help", "-bogus"]), full);
        assert_eq!(parse(&["--help"]), full);
        assert_eq!(parse(&["/h", "-n"]), quick);
        assert_eq!(parse(&["/?"]), quick);
        assert_eq!(parse(&["--h"]), quick);
        assert_eq!(parse(&["-version", "-path"]), Ok(Command::Version));
        assert_eq!(fatal(&["-bogus", "-h"]).exit_code, ExitCode::UnknownSwitch);
    }

    #[test]
    fn all_dash_tokens_are_search_text() {
        assert_eq!(plan(&["-", "--", "---"]).query, "- -- ---");
    }

    #[test]
    fn utf8_bom_switches() {
        assert!(plan(&["-utf8-bom"]).bom, "accepted no-op vs gozler default");
        assert!(!plan(&["-no-utf8-bom"]).bom);
        assert!(plan(&["-no-utf8-bom", "-utf8-bom"]).bom, "last flag wins");
    }

    #[test]
    fn header_and_double_quote_suppression() {
        assert!(!plan(&["-no-header"]).header);
        assert!(!plan(&["-no-double-quote"]).double_quote);
    }

    #[test]
    fn match_case_aliases() {
        assert!(plan(&["-i"]).match_case);
        assert!(plan(&["-case"]).match_case);
        assert!(plan(&["-match-case"]).match_case);
        assert!(plan(&["-matchcase"]).match_case);
    }

    /// The folder switches keep their values as typed, for
    /// [`build_search`](crate::escompat::build_search) to make absolute; the
    /// last of `-path` and `-parent-path` is the scope.
    #[test]
    fn folder_switches_are_taken_verbatim() {
        assert_eq!(
            plan(&["-path", "rel\\dir"]).scope,
            Some(ScopeFolder::Dir("rel\\dir".to_string()))
        );
        let parents = plan(&["-path", "a", "-parent-path", "b/c", "-parent", "d"]);
        assert_eq!(
            (parents.scope, parents.parent.as_deref()),
            (Some(ScopeFolder::ParentOf("b/c".to_string())), Some("d"))
        );
    }
}
