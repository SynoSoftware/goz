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
//!   /downloads to search for \downloads").
//! - A token consisting solely of dashes (`-`, `--`, `---`, ...) is search
//!   text, never a switch (es `es_is_literal_switch`).
//! - Other `--`-prefixed tokens are goz's native namespace, matched exactly
//!   (case-sensitive, dashes required). Anything else is fatal (exit 6).
//! - A parameter-taking switch consumes the next token; a missing or
//!   unparseable parameter is fatal (exit 4).
//! - `-h`, `-help` and `-version` end parsing where they appear, as in es, so
//!   a token before them can still fail and a token after them is ignored.
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
use crate::types::{EsColumn, SortDir, SortKey, SortSpec};
use std::time::Duration;

/// What a command line asks gozler to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// `-h` or `-help`: print the help.
    Help,
    /// `-version`: print the version.
    Version,
    /// Search, or with `--status` report the daemon's state.
    Run(EsPlan),
}

/// The fully-resolved plan for one es-compatible invocation.
///
/// Produced by [`parse_argv`]; consumed by the CLI, which resolves
/// process-level concerns (absolutizing `scope`, creating the export file,
/// talking to the daemon).
#[derive(Clone, Debug, PartialEq)]
pub struct EsPlan {
    /// Search text: every non-switch token, joined with single spaces in
    /// command-line order, each token preserved as received.
    pub query: String,
    /// `-path` value, verbatim. The CLI absolutizes it (GetFullPathName
    /// semantics, no existence check); the core stays path-API-free.
    pub scope: Option<String>,
    /// How many results to show, from `-n`; [`EsPlan::row_limit`] applies it.
    pub limit: Limit,
    /// Optional CSV columns from `-size` / `-dm` / `-date-modified`, in flag
    /// order with es dedupe semantics (a repeated flag moves its column to
    /// the end). `Filename` is always emitted last and is not listed here.
    pub columns: Vec<EsColumn>,
    /// Result order. Defaults to name-ascending (es with no `-sort` flag);
    /// unsuffixed sort switches use es's per-key default direction.
    pub sort: SortSpec,
    /// `-export-csv` value, verbatim. `None` means write results to stdout.
    pub export_csv: Option<String>,
    /// Whether the CSV starts with a UTF-8 BOM. Default true: a
    /// documented goz deviation from stock es (which defaults BOM off).
    /// `-no-utf8-bom` clears it; `-utf8-bom` is accepted and sets it.
    pub bom: bool,
    /// Whether the CSV includes the header row. `-no-header` clears it.
    pub header: bool,
    /// Whether string columns are always double-quoted (es default).
    /// `-no-double-quote` downgrades to quote-only-when-needed.
    pub double_quote: bool,
    /// Case-sensitive matching, from `-case` / `-match-case`. Default off.
    pub match_case: bool,
    /// `-no-result-error`: exit 9 when nothing matches.
    pub no_result_error: bool,
    /// `-timeout MS`: how long to wait for the volumes in scope to become
    /// complete before searching. `None` searches at once.
    pub timeout: Option<Duration>,
    /// Native `--json`: emit results as JSON instead of CSV/plain text.
    pub json: bool,
    /// Native `--status`: report daemon/index status instead of searching.
    pub status: bool,
    /// Native `--insecure-no-server-check`: skip the pipe-server owner-SID
    /// verification (dev escape hatch).
    pub insecure_no_server_check: bool,
}

impl Default for EsPlan {
    /// The plan for an empty command line: name-ascending sort and es's
    /// default output shape (header on, always-quote on, BOM on per goz's
    /// deviation).
    fn default() -> Self {
        Self {
            query: String::new(),
            scope: None,
            limit: Limit::Default,
            columns: Vec::new(),
            sort: SortSpec::default(),
            export_csv: None,
            bom: true,
            header: true,
            double_quote: true,
            match_case: false,
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
            Limit::Default if self.export_csv.is_some() => None,
            Limit::Default => Some(DEFAULT_LIMIT),
            Limit::All => None,
            Limit::Rows(rows) => Some(rows),
        }
    }
}

/// The rows gozler shows without `-n`.
pub const DEFAULT_LIMIT: u32 = 20;

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
            message: format!("unknown switch: {token}"),
        }
    }

    /// The switch requires a parameter and none followed.
    fn missing_param(token: &str) -> Self {
        Self {
            exit_code: ExitCode::InvalidInput,
            message: format!("expected an additional parameter after {token}"),
        }
    }

    /// The switch got a parameter it cannot interpret.
    fn bad_param(token: &str, value: &str, expected: &str) -> Self {
        Self {
            exit_code: ExitCode::InvalidInput,
            message: format!("expected {expected} after {token}, got: {value}"),
        }
    }
}

/// One switch, or one family of switches: how the command line spells it,
/// and its line in the help.
pub(super) struct Switch {
    pub(super) spelling: Spelling,
    pub(super) help: &'static str,
}

/// How a [`Switch`] is spelled, which also decides its section of the help.
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
    /// `-export-csv`: consume the next token as the output path, verbatim.
    ExportCsv,
    /// `-sort`: consume the next token as a sort-key name with an optional
    /// `-ascending` / `-descending` suffix.
    SortParam,
    /// `-size` / `-dm` / `-date-modified`: append a column (es dedupe).
    Column(EsColumn),
    /// A combined `-sort-<key>[-ascending|-descending]` switch.
    SortFixed(SortSpec),
    /// `-utf8-bom`: set BOM on (a no-op against goz's default).
    Utf8Bom,
    /// `-no-utf8-bom`: set BOM off.
    NoUtf8Bom,
    /// `-no-header`: suppress the CSV header row.
    NoHeader,
    /// `-no-double-quote`: quote string columns only when needed.
    NoDoubleQuote,
    /// `-case` / `-match-case`: case-sensitive matching.
    MatchCase,
    /// `--json`: print results as JSON.
    Json,
    /// `--status`: report the daemon's state instead of searching.
    Status,
    /// `--insecure-no-server-check`: skip the pipe server's trust check.
    InsecureNoServerCheck,
    /// `-h` / `-help`: stop parsing and print the help.
    Help,
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
            SwitchAction::Scope => Some("DIR"),
            SwitchAction::ExportCsv => Some("FILE"),
            SwitchAction::SortParam => Some("KEY"),
            SwitchAction::Column(_)
            | SwitchAction::SortFixed(_)
            | SwitchAction::Utf8Bom
            | SwitchAction::NoUtf8Bom
            | SwitchAction::NoHeader
            | SwitchAction::NoDoubleQuote
            | SwitchAction::MatchCase
            | SwitchAction::NoResultError
            | SwitchAction::Json
            | SwitchAction::Status
            | SwitchAction::InsecureNoServerCheck
            | SwitchAction::Help
            | SwitchAction::Version => None,
        }
    }
}

/// Every switch gozler accepts, in the order the help lists them.
pub(super) const SWITCHES: &[Switch] = &[
    Switch {
        spelling: Spelling::Es {
            names: &["n", "max-results", "count"],
            action: SwitchAction::Limit,
        },
        help: "show at most N results, or every result with -n all. Without -n, \
               gozler shows 20, and -export-csv writes every result. Exit 10 when \
               the limit leaves out matches",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["path"],
            action: SwitchAction::Scope,
        },
        help: "search below a folder",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["sort"],
            action: SwitchAction::SortParam,
        },
        help: "order by a sort key; KEY-ascending or KEY-descending sets the direction",
    },
    Switch {
        spelling: Spelling::Family {
            usage: "-sort-KEY",
            parse: parse_sort_switch,
        },
        help: "the same as -sort KEY, also with -ascending or -descending",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["case", "match-case"],
            action: SwitchAction::MatchCase,
        },
        help: "match case",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["export-csv"],
            action: SwitchAction::ExportCsv,
        },
        help: "write the results to FILE as CSV instead of to stdout",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["size"],
            action: SwitchAction::Column(EsColumn::Size),
        },
        help: "add a Size column to the CSV",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["dm", "date-modified"],
            action: SwitchAction::Column(EsColumn::DateModified),
        },
        help: "add a Date Modified column to the CSV",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["utf8-bom"],
            action: SwitchAction::Utf8Bom,
        },
        help: "start the CSV with a UTF-8 byte order mark (the default)",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-utf8-bom"],
            action: SwitchAction::NoUtf8Bom,
        },
        help: "leave out the byte order mark",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-header"],
            action: SwitchAction::NoHeader,
        },
        help: "leave out the CSV header row",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-double-quote"],
            action: SwitchAction::NoDoubleQuote,
        },
        help: "quote a CSV filename only when it needs quotes",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["no-result-error"],
            action: SwitchAction::NoResultError,
        },
        help: "exit 9 when nothing matches",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["timeout"],
            action: SwitchAction::Timeout,
        },
        help: "wait up to MS milliseconds for the volumes in scope to finish \
               indexing, then search; a volume still indexing makes the answer exit 2",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["h", "help"],
            action: SwitchAction::Help,
        },
        help: "show this help",
    },
    Switch {
        spelling: Spelling::Es {
            names: &["version"],
            action: SwitchAction::Version,
        },
        help: "print the version",
    },
    Switch {
        spelling: Spelling::Native {
            name: "json",
            action: SwitchAction::Json,
        },
        help: "print the results, or with --status the state, as JSON",
    },
    Switch {
        spelling: Spelling::Native {
            name: "status",
            action: SwitchAction::Status,
        },
        help: "show the state of gozlerd and each volume instead of searching; exit 2 \
               when a volume is not live or its sizes and dates could not be read",
    },
    Switch {
        spelling: Spelling::Native {
            name: "insecure-no-server-check",
            action: SwitchAction::InsecureNoServerCheck,
        },
        help: "skip the check that the pipe server is gozlerd (testing only)",
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
        help: "name, A to Z (the default)",
    },
    SortKeyName {
        names: &["path"],
        key: SortKey::Path,
        help: "folder, then name, A to Z",
    },
    SortKeyName {
        names: &["size"],
        key: SortKey::Size,
        help: "size, largest first",
    },
    SortKeyName {
        names: &["dm", "date-modified"],
        key: SortKey::DateModified,
        help: "date modified, newest first",
    },
];

/// Parses an argv slice (excluding `argv[0]`) into a [`Command`].
///
/// Pure and total: no I/O, no environment access, no panics on any input.
/// A refused command line returns [`EsFatal`] with its exit code.
///
/// ```
/// use goz_core::escompat::{Command, parse_argv};
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
    let mut plan = EsPlan::default();
    let mut query: Vec<&str> = Vec::new();
    let mut rest = args.iter();

    while let Some(token) = rest.next() {
        let token = token.as_str();

        // es literal-switch rule: all-dash tokens are search text.
        if !token.is_empty() && token.bytes().all(|b| b == b'-') {
            query.push(token);
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
                    query.push(token);
                    continue;
                }
            }
        } else {
            query.push(token);
            continue;
        };

        match action {
            SwitchAction::Help => return Ok(Command::Help),
            SwitchAction::Version => return Ok(Command::Version),
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

/// Parses the `-sort-KEY[-ascending|-descending]` family.
fn parse_sort_switch(name: &str) -> Option<SwitchAction> {
    SORT_KEYS.iter().find_map(|row| {
        row.names.iter().find_map(|alias| {
            match_sort_form(name, &format!("sort-{alias}"), row.key).map(SwitchAction::SortFixed)
        })
    })
}

/// Applies one matched switch, consuming its parameter from `rest` if it
/// takes one. `token` is the switch as the user typed it (for messages).
/// [`SwitchAction::Help`] and [`SwitchAction::Version`] end parsing in
/// [`parse_argv`] and never reach here.
fn apply_switch(
    action: SwitchAction,
    token: &str,
    rest: &mut core::slice::Iter<'_, String>,
    plan: &mut EsPlan,
) -> Result<(), EsFatal> {
    match action {
        SwitchAction::Limit => {
            let value = take_param(rest, token)?;
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
            let value = take_param(rest, token)?;
            let ms: u32 = value
                .parse()
                .map_err(|_| EsFatal::bad_param(token, value, "milliseconds"))?;
            plan.timeout = Some(Duration::from_millis(ms.into()));
        }
        SwitchAction::NoResultError => plan.no_result_error = true,
        SwitchAction::Scope => plan.scope = Some(take_param(rest, token)?.to_owned()),
        SwitchAction::ExportCsv => plan.export_csv = Some(take_param(rest, token)?.to_owned()),
        SwitchAction::SortParam => {
            let value = take_param(rest, token)?;
            plan.sort = parse_sort_param(value)
                .ok_or_else(|| EsFatal::bad_param(token, value, "a sort key"))?;
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
        SwitchAction::Json => plan.json = true,
        SwitchAction::Status => plan.status = true,
        SwitchAction::InsecureNoServerCheck => plan.insecure_no_server_check = true,
        SwitchAction::Help | SwitchAction::Version => {
            unreachable!("parse_argv returns at -help and -version")
        }
    }
    Ok(())
}

/// Consumes the next token as a switch parameter, or fails with exit 4
/// naming the switch as the user typed it.
fn take_param<'a>(
    rest: &mut core::slice::Iter<'a, String>,
    token: &str,
) -> Result<&'a str, EsFatal> {
    rest.next()
        .map(String::as_str)
        .ok_or_else(|| EsFatal::missing_param(token))
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

    #[test]
    fn empty_argv_yields_default_plan() {
        let parsed = plan(&[]);
        assert_eq!(parsed, EsPlan::default());
        assert_eq!(parsed.query, "");
        assert_eq!(parsed.sort, SortSpec::default());
        assert!(parsed.bom);
        assert!(parsed.header);
        assert!(parsed.double_quote);
        assert!(!parsed.match_case);
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
                scope: Some("C:\\some dir".to_string()),
                limit: Limit::Rows(200),
                columns: vec![EsColumn::Size, EsColumn::DateModified],
                sort: SortSpec {
                    key: SortKey::Path,
                    dir: SortDir::Asc,
                },
                export_csv: Some("C:\\tmp\\out.csv".to_string()),
                bom: true,
                header: true,
                double_quote: true,
                match_case: false,
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

    #[test]
    fn slash_token_with_match_is_a_switch() {
        // Research: es_check_param treats `-` and `/` prefixes identically,
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

    /// The owner's default limit: stdout gets 20 rows unless -n says
    /// otherwise, and an export file every match; -n all lifts the limit, and
    /// -count is es's alias of -n.
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
        assert_eq!(parse(&["report", "-help", "-bogus"]), Ok(Command::Help));
        assert_eq!(parse(&["/h", "-n"]), Ok(Command::Help));
        assert_eq!(parse(&["-version", "-path"]), Ok(Command::Version));
        assert_eq!(fatal(&["-bogus", "-h"]).exit_code, ExitCode::UnknownSwitch);
    }

    #[test]
    fn all_dash_tokens_are_search_text() {
        assert_eq!(plan(&["-", "--", "---"]).query, "- -- ---");
    }

    #[test]
    fn utf8_bom_switches() {
        assert!(plan(&["-utf8-bom"]).bom, "accepted no-op vs goz default");
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
        assert!(plan(&["-case"]).match_case);
        assert!(plan(&["-match-case"]).match_case);
        assert!(plan(&["-matchcase"]).match_case);
    }

    #[test]
    fn scope_is_taken_verbatim() {
        // Absolutization is the CLI's job; the core keeps the raw value.
        assert_eq!(
            plan(&["-path", "rel\\dir"]).scope,
            Some("rel\\dir".to_string())
        );
    }

    #[test]
    fn fatal_display_is_the_message() {
        let err = fatal(&["-zzz"]);
        assert_eq!(format!("{err}"), err.message);
    }
}
