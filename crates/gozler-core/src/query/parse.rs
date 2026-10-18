//! Query tokenizer and grammar for goz's v1 search syntax.
//!
//! Supported: space-separated terms (implicit AND), `"quoted phrases"`,
//! `*`/`?` wildcards (anchored to the whole name), path terms (a term
//! containing `\` or `/`, or a `path:` value; a path term with a wildcard is
//! anchored to the whole full path), and the functions `ext:`, `size:`,
//! `path:`, `file:`, `folder:`, `case:`. The boolean operators `|` and `!` are
//! rejected loudly rather than silently mis-executed: es's own precedence
//! for them is undocumented, so guessing it would be a compatibility trap.
//! Unknown `func:` prefixes are treated as literal text, matching es.
//!
//! The parser turns the text into a [`ParsedQuery`], which the client sends to
//! the daemon as it is: AND-ed clauses, each an OR of terms. Terms keep the
//! text as typed; the daemon folds them when it compiles the query
//! ([`super::CompiledQuery`]).

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// Scratch buffer of decoded code points, reused across candidates so a
/// wildcard scan doesn't heap-allocate per candidate. NTFS names are <= 255
/// code points, so the inline capacity never spills.
pub type CodePointBuf = SmallVec<[u32; 256]>;

/// A compiled wildcard pattern, matched against a whole name or, for a path
/// wildcard, a whole full path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wildcard {
    tokens: Vec<GlobTok>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GlobTok {
    Lit(u32),
    /// `?`: exactly one code point.
    AnyOne,
    /// `*`: any run of code points (including empty).
    AnyRun,
}

impl Wildcard {
    /// Compiles a pattern whose bytes are already folded iff the query is
    /// case-insensitive.
    pub(crate) fn compile(pattern: &[u8]) -> Self {
        let mut tokens = Vec::new();
        for cp in crate::wtf8::CodePoints::new(pattern) {
            match cp {
                0x2A => {
                    // '*': collapse runs of '*' into one AnyRun.
                    if tokens.last() != Some(&GlobTok::AnyRun) {
                        tokens.push(GlobTok::AnyRun);
                    }
                }
                0x3F => tokens.push(GlobTok::AnyOne),
                other => tokens.push(GlobTok::Lit(other)),
            }
        }
        Wildcard { tokens }
    }

    /// Matches the whole of `name`, decoding its code points into the caller's
    /// reusable `buf` (cleared first) instead of allocating one per candidate.
    /// NTFS names are <= 255 code points, so a name never spills the inline
    /// capacity; a full path can, and the spilled buffer is then reused.
    pub fn matches_into(&self, name: &[u8], buf: &mut CodePointBuf) -> bool {
        buf.clear();
        buf.extend(crate::wtf8::CodePoints::new(name));
        glob_match(&self.tokens, buf.as_slice())
    }

    /// Matches the whole of `name` (WTF-8, folded iff the query is
    /// case-insensitive) against the pattern, with a one-off buffer.
    #[cfg(test)]
    pub(crate) fn matches(&self, name: &[u8]) -> bool {
        let mut buf = CodePointBuf::new();
        self.matches_into(name, &mut buf)
    }

    /// The longest run of literal characters in the pattern, as WTF-8 bytes
    /// (already folded iff the query is case-insensitive). A name matching the
    /// pattern must contain this substring, so it can drive a prefilter scan.
    /// `None` if the pattern is all wildcards (e.g. `*`, `?`).
    pub fn longest_literal(&self) -> Option<Vec<u8>> {
        let mut best: Vec<u8> = Vec::new();
        let mut run: Vec<u8> = Vec::new();
        for tok in &self.tokens {
            match tok {
                GlobTok::Lit(cp) => crate::wtf8::encode_code_point(*cp, &mut run),
                _ => {
                    if run.len() > best.len() {
                        best = std::mem::take(&mut run);
                    }
                    run.clear();
                }
            }
        }
        if run.len() > best.len() {
            best = run;
        }
        (!best.is_empty()).then_some(best)
    }
}

/// Iterative glob match with backtracking on `*` (linear-ish; patterns and
/// names are short).
fn glob_match(pat: &[GlobTok], text: &[u32]) -> bool {
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pattern idx after '*', text idx)
    while ti < text.len() {
        match pat.get(pi) {
            Some(GlobTok::Lit(c)) if *c == text[ti] => {
                pi += 1;
                ti += 1;
            }
            Some(GlobTok::AnyOne) => {
                pi += 1;
                ti += 1;
            }
            Some(GlobTok::AnyRun) => {
                star = Some((pi + 1, ti));
                pi += 1;
            }
            _ => {
                // Mismatch: backtrack to the last '*' and consume one more char.
                if let Some((sp, st)) = star {
                    pi = sp;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                } else {
                    return false;
                }
            }
        }
    }
    while let Some(GlobTok::AnyRun) = pat.get(pi) {
        pi += 1;
    }
    pi == pat.len()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    File,
    Folder,
}

/// An inclusive byte-size range.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SizeRange {
    pub min: u64,
    pub max: u64,
}

impl SizeRange {
    pub fn contains(self, n: u64) -> bool {
        self.min <= n && n <= self.max
    }
}

/// A parsed query: the entries that match every clause.
///
/// The client sends it to the daemon as part of a
/// [`SearchRequest`](crate::proto::SearchRequest), so it is a wire type: it
/// only grows, by new [`Test`] variants and new fields, and each such change
/// raises [`PROTO_VERSION`](crate::proto::PROTO_VERSION), so a daemon that
/// cannot read the new form refuses the client's `Hello` instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParsedQuery {
    pub clauses: Vec<Clause>,
    /// Whether text terms match case-sensitively, after any `case:` directive.
    pub match_case: bool,
}

/// Terms of which at least one must match; a clause with no terms matches
/// nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Clause {
    pub terms: Vec<Term>,
}

impl Clause {
    /// A clause of one term that must match.
    pub(crate) fn single(test: Test) -> Self {
        Clause {
            terms: vec![Term {
                negated: false,
                test,
            }],
        }
    }
}

/// One test, or with `negated` its opposite. A size test never matches an
/// entry whose size is unknown or a folder, negated or not, so a negated
/// size test does not report an unread file as small.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub negated: bool,
    pub test: Test,
}

/// What a [`Term`] tests. Text is as typed; the daemon folds it unless the
/// query matches case.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Test {
    /// The name contains this text.
    Name(String),
    /// The full path contains this text.
    Path(String),
    /// The whole name matches this pattern of `*` and `?`.
    NameWildcard(String),
    /// The whole full path matches this pattern of `*` and `?`.
    PathWildcard(String),
    /// The extension is one of these, without the dot, compared ignoring case
    /// whatever the query's case.
    Extension(Vec<String>),
    /// The file's size is in this range.
    Size(SizeRange),
    Kind(Kind),
}

impl ParsedQuery {
    /// `true` when no term tests text: the query matches everything its
    /// other tests let through.
    #[cfg(test)]
    pub(crate) fn is_text_empty(&self) -> bool {
        !self.clauses.iter().flat_map(|c| &c.terms).any(|t| {
            matches!(
                t.test,
                Test::Name(_) | Test::Path(_) | Test::NameWildcard(_) | Test::PathWildcard(_)
            )
        })
    }
}

/// A query that cannot be executed as written.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    #[error(
        "operator '{0}' is not supported yet; quote it to search for it literally (e.g. \"a{0}b\")"
    )]
    UnsupportedOperator(char),
    #[error("malformed {what}: {detail}")]
    Malformed { what: &'static str, detail: String },
    /// More terms than a query may hold; carries the query's count.
    #[error("the query has {0} terms; a query may have at most {max}", max = super::engine::MAX_QUERY_TERMS)]
    TooManyTerms(usize),
}

/// One search function: its names, what it does, and its line in the help.
pub(crate) struct SearchFunction {
    /// Names without the colon, matched case-insensitively. The first is the
    /// one the help lists first.
    pub(crate) names: &'static [&'static str],
    function: Function,
    /// The value the help shows after each name, such as `>1mb` for `size:`.
    pub(crate) example: &'static str,
    pub(crate) help: &'static str,
}

/// What a [`SearchFunction`] does to the query.
#[derive(Clone, Copy)]
enum Function {
    Ext,
    Size,
    Path,
    File,
    Folder,
    /// Handled by [`case_directive`] before the other functions; a `case:`
    /// value it does not accept is searched as text.
    Case,
}

/// Every search function, in the order the help lists them. A `name:` token
/// that names none of them is searched as text, as in es.
pub(crate) const SEARCH_FUNCTIONS: &[SearchFunction] = &[
    SearchFunction {
        names: &["ext"],
        function: Function::Ext,
        example: "cs;txt",
        help: "extension, one of a list",
    },
    SearchFunction {
        names: &["size"],
        function: Function::Size,
        example: ">1mb",
        help: "file size; also <, >=, <=, =, 1mb..10mb, empty, tiny, small, medium, large, huge, gigantic",
    },
    SearchFunction {
        names: &["path"],
        function: Function::Path,
        example: "text",
        help: "the full path contains text; with * or ? the whole full path matches",
    },
    SearchFunction {
        names: &["file"],
        function: Function::File,
        example: "",
        help: "files only; file:text also matches text",
    },
    SearchFunction {
        names: &["folder", "dir"],
        function: Function::Folder,
        example: "",
        help: "folders only; folder:text also matches text",
    },
    SearchFunction {
        names: &["case"],
        function: Function::Case,
        example: "",
        help: "match case; case:0 turns it off",
    },
];

/// Recognizes the `case:` directive: `case:`/`case:1` enable case-sensitivity,
/// `case:0` disables it. Returns `None` for anything else (so it is not
/// mistaken for a search term). Case-insensitive on the function name itself.
fn case_directive(text: &str) -> Option<bool> {
    match text.to_ascii_lowercase().as_str() {
        "case:" | "case:1" => Some(true),
        "case:0" => Some(false),
        _ => None,
    }
}

/// Parses a case-insensitive query string into a [`ParsedQuery`]; a `case:`
/// directive in the query makes it case-sensitive.
pub fn parse_query(input: &str) -> Result<ParsedQuery, QueryError> {
    parse_query_with_case(input, false)
}

/// Parses a query string into a [`ParsedQuery`] whose case sensitivity
/// defaults to `match_case` (es `-case`). A `case:` directive in the query
/// overrides the default, so [`ParsedQuery::match_case`] is the one value that
/// both matching and the `-path` scope lookup follow.
///
/// Each text term is a clause of its own, so the terms are AND-ed. A later
/// `ext:`, `size:`, `file:` or `folder:` replaces an earlier one.
pub fn parse_query_with_case(input: &str, match_case: bool) -> Result<ParsedQuery, QueryError> {
    let tokens = tokenize(input)?;

    // The last `case:` directive wins.
    let match_case = tokens
        .iter()
        .filter(|t| !t.quoted)
        .filter_map(|t| case_directive(&t.text))
        .next_back()
        .unwrap_or(match_case);

    let mut clauses = Vec::new();
    let mut ext = None;
    let mut size = None;
    let mut kind = None;
    for tok in &tokens {
        if tok.quoted {
            // Quoted phrases are literal filename substrings.
            clauses.push(Clause::single(Test::Name(tok.text.clone())));
            continue;
        }
        let text = &tok.text;
        if case_directive(text).is_some() {
            continue; // case:/case:0/case:1 are consumed, never search terms
        }

        // Function tokens: prefix before the first ':'.
        let mut text_term = text.as_str();
        if let Some(colon) = text.find(':') {
            let value = &text[colon + 1..];
            match function_named(&text[..colon]) {
                Some(Function::Ext) => {
                    let exts: Vec<String> = value
                        .split(';')
                        .filter(|s| !s.is_empty())
                        .map(|s| s.trim_start_matches('.').to_string())
                        .collect();
                    if !exts.is_empty() {
                        ext = Some(exts);
                    }
                    continue;
                }
                Some(Function::Size) => {
                    size = Some(parse_size(value)?);
                    continue;
                }
                Some(Function::Path) => {
                    if has_wildcard(value) {
                        clauses.push(Clause::single(Test::PathWildcard(value.to_string())));
                    } else if !value.is_empty() {
                        clauses.push(Clause::single(Test::Path(value.to_string())));
                    }
                    continue;
                }
                Some(Function::File) => {
                    kind = Some(Kind::File);
                    text_term = value;
                }
                Some(Function::Folder) => {
                    kind = Some(Kind::Folder);
                    text_term = value;
                }
                // A `case:` value that is not a directive, and an unknown
                // function, are literal text (es behavior).
                Some(Function::Case) | None => {}
            }
        }
        clauses.extend(text_test(text_term).map(Clause::single));
    }

    let filters = [
        kind.map(Test::Kind),
        size.map(Test::Size),
        ext.map(Test::Extension),
    ];
    clauses.extend(filters.into_iter().flatten().map(Clause::single));
    Ok(ParsedQuery {
        clauses,
        match_case,
    })
}

/// The search function `name` names, compared case-insensitively.
fn function_named(name: &str) -> Option<Function> {
    SEARCH_FUNCTIONS
        .iter()
        .find(|row| row.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
        .map(|row| row.function)
}

/// The test of an unquoted text token, `None` for an empty one: a term with a
/// separator matches the full path, any other term the name, and a wildcard
/// in either anchors it to the whole of what it matches.
fn text_test(text: &str) -> Option<Test> {
    if text.is_empty() {
        return None;
    }
    let text = text.to_string();
    let is_path = text.contains('\\') || text.contains('/');
    Some(match (has_wildcard(&text), is_path) {
        (true, true) => Test::PathWildcard(text),
        (true, false) => Test::NameWildcard(text),
        (false, true) => Test::Path(text),
        (false, false) => Test::Name(text),
    })
}

fn has_wildcard(text: &str) -> bool {
    text.contains('*') || text.contains('?')
}

struct Token {
    text: String,
    quoted: bool,
}

/// Splits on unquoted whitespace, honoring `"double quotes"`, and rejects the
/// unsupported boolean operators when they appear unquoted.
fn tokenize(input: &str) -> Result<Vec<Token>, QueryError> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut token_quoted = false;
    let mut has_content = false;

    for ch in input.chars() {
        if in_quotes {
            if ch == '"' {
                in_quotes = false;
            } else {
                cur.push(ch);
            }
            continue;
        }
        match ch {
            '"' => {
                in_quotes = true;
                token_quoted = true;
                has_content = true;
            }
            // Reject the boolean OR / NOT operators (whose es precedence is
            // undocumented). `<`/`>` are NOT rejected: they are comparison
            // operators inside the `size:` function. es grouping via `<…>`
            // is unsupported and would be treated as literal text.
            '|' | '!' => return Err(QueryError::UnsupportedOperator(ch)),
            c if c.is_whitespace() => {
                if has_content {
                    tokens.push(Token {
                        text: std::mem::take(&mut cur),
                        quoted: token_quoted,
                    });
                    token_quoted = false;
                    has_content = false;
                }
            }
            c => {
                cur.push(c);
                has_content = true;
            }
        }
    }
    if has_content {
        tokens.push(Token {
            text: cur,
            quoted: token_quoted,
        });
    }
    Ok(tokens)
}

const KB: u64 = 1024;
const MB: u64 = 1024 * KB;
const GB: u64 = 1024 * MB;
const TB: u64 = 1024 * GB;

/// Parses an es-style `size:` expression: a bare number, a comparison
/// (`>`, `>=`, `<`, `<=`, `=`), a range `a..b`, or a named constant
/// (`empty`, `tiny`, …, `gigantic`). Numbers accept `kb`/`mb`/`gb`/`tb`
/// (1024-based) suffixes.
fn parse_size(value: &str) -> Result<SizeRange, QueryError> {
    let v = value.trim();
    let malformed = |detail: &str| QueryError::Malformed {
        what: "size:",
        detail: detail.to_string(),
    };

    // Named constants.
    match v.to_ascii_lowercase().as_str() {
        "empty" => return Ok(SizeRange { min: 0, max: 0 }),
        "tiny" => {
            return Ok(SizeRange {
                min: 0,
                max: 10 * KB,
            });
        }
        "small" => {
            return Ok(SizeRange {
                min: 10 * KB,
                max: 100 * KB,
            });
        }
        "medium" => {
            return Ok(SizeRange {
                min: 100 * KB,
                max: MB,
            });
        }
        "large" => {
            return Ok(SizeRange {
                min: MB,
                max: 16 * MB,
            });
        }
        "huge" => {
            return Ok(SizeRange {
                min: 16 * MB,
                max: 128 * MB,
            });
        }
        "gigantic" => {
            return Ok(SizeRange {
                min: 128 * MB,
                max: u64::MAX,
            });
        }
        _ => {}
    }

    // Range a..b (either side optional).
    if let Some((lo, hi)) = v.split_once("..") {
        let min = if lo.trim().is_empty() {
            0
        } else {
            parse_bytes(lo).ok_or_else(|| malformed(lo))?
        };
        let max = if hi.trim().is_empty() {
            u64::MAX
        } else {
            parse_bytes(hi).ok_or_else(|| malformed(hi))?
        };
        return Ok(SizeRange { min, max });
    }

    // Comparison operators. Order matters: two-char ops before their one-char
    // prefixes so `>=` is not read as `>` plus a stray `=`.
    for op in [">=", "<=", ">", "<"] {
        if let Some(rest) = v.strip_prefix(op) {
            let n = parse_bytes(rest).ok_or_else(|| malformed(rest))?;
            return Ok(match op {
                ">=" => SizeRange {
                    min: n,
                    max: u64::MAX,
                },
                ">" => SizeRange {
                    min: n.saturating_add(1),
                    max: u64::MAX,
                },
                "<=" => SizeRange { min: 0, max: n },
                // The only remaining op.
                _ => SizeRange {
                    min: 0,
                    max: n.saturating_sub(1),
                },
            });
        }
    }

    // Bare number or `=N` → exact.
    let rest = v.strip_prefix('=').unwrap_or(v);
    let n = parse_bytes(rest).ok_or_else(|| malformed(v))?;
    Ok(SizeRange { min: n, max: n })
}

/// Parses `"10mb"` / `"1024"` / `"2gb"` into a byte count (1024-based units).
fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim();
    let lower = s.to_ascii_lowercase();
    let (num, mult) = if let Some(n) = lower.strip_suffix("tb") {
        (n, TB)
    } else if let Some(n) = lower.strip_suffix("gb") {
        (n, GB)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, MB)
    } else if let Some(n) = lower.strip_suffix("kb") {
        (n, KB)
    } else if let Some(n) = lower.strip_suffix('b') {
        (n, 1)
    } else {
        (lower.as_str(), 1)
    };
    let num = num.trim();
    let value: u64 = num.parse().ok()?;
    value.checked_mul(mult)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold::fold;

    fn q(s: &str) -> ParsedQuery {
        parse_query(s).unwrap()
    }

    /// The tests of a query whose clauses each hold one positive term, as
    /// every query does until the parser reads `!` and `|`.
    fn tests(s: &str) -> Vec<Test> {
        q(s).clauses
            .into_iter()
            .map(|clause| match <[Term; 1]>::try_from(clause.terms) {
                Ok(
                    [
                        Term {
                            negated: false,
                            test,
                        },
                    ],
                ) => test,
                Ok([term]) => panic!("{s}: a negated {term:?}"),
                Err(terms) => panic!("{s}: a clause of {terms:?}"),
            })
            .collect()
    }

    fn name(s: &str) -> Test {
        Test::Name(s.to_string())
    }

    #[test]
    fn space_separated_terms_are_anded_as_typed() {
        assert_eq!(tests("Report Invoice"), [name("Report"), name("Invoice")]);
        assert!(!q("Report Invoice").match_case);
    }

    #[test]
    fn quoted_phrase_keeps_spaces() {
        assert_eq!(tests("\"quarterly report\""), [name("quarterly report")]);
    }

    #[test]
    fn case_directive_variants_are_consumed_not_searched() {
        // Regression: `case:0` must be recognized (and disable case
        // sensitivity), never leak as a literal "case:0" search term.
        assert!(q("Report case:").match_case);
        assert!(!q("Report case:0").match_case);
        assert_eq!(tests("Report case:0"), [name("Report")]);
        assert!(q("Report case:1").match_case);
        // Last directive wins.
        assert!(!q("case:1 foo case:0").match_case);
        assert!(q("case:0 foo case:1").match_case);
    }

    #[test]
    fn path_term_detected_by_separator() {
        assert_eq!(
            tests("projects\\src"),
            [Test::Path("projects\\src".to_string())]
        );
    }

    /// A wildcard with a separator, and a `path:` value with a wildcard, match
    /// the full path; neither is a name wildcard, which no name with a
    /// separator could ever match.
    #[test]
    fn a_wildcard_with_a_separator_is_a_path_wildcard() {
        for query in [r"*\docs\*.md", "*/docs/*.md", "path:*.md"] {
            assert!(
                matches!(tests(query)[..], [Test::PathWildcard(_)]),
                "{query}"
            );
        }
        assert_eq!(
            tests(r"file:*\docs\*"),
            [
                Test::PathWildcard(r"*\docs\*".to_string()),
                Test::Kind(Kind::File)
            ]
        );
        assert_eq!(tests("path:docs"), [Test::Path("docs".to_string())]);
        assert_eq!(tests("*.rs"), [Test::NameWildcard("*.rs".to_string())]);
    }

    /// `-case` sets the default; a `case:` directive overrides it either way.
    #[test]
    fn the_case_switch_is_the_default_a_directive_overrides() {
        let case = |text, default| parse_query_with_case(text, default).unwrap().match_case;
        assert!(case("Report", true));
        assert!(!case("Report case:0", true));
        assert!(case("Report case:", false));
        assert!(!case("Report", false));
    }

    fn wildcard(pattern: &str) -> Wildcard {
        Wildcard::compile(&fold(pattern.as_bytes()))
    }

    #[test]
    fn wildcards_match_the_whole_name() {
        let rs = wildcard("*.rs");
        assert!(rs.matches(&fold(b"main.rs")));
        assert!(!rs.matches(&fold(b"main.rs.bak")));
        let star = wildcard("re*rt");
        assert!(star.matches(&fold(b"report")));
        assert!(star.matches(&fold(b"rert"))); // "re" + empty + "rt"
        assert!(!star.matches(&fold(b"rt"))); // missing the "re" prefix
        assert!(!star.matches(&fold(b"reprtx")));
    }

    #[test]
    fn question_mark_matches_one_code_point() {
        let p = wildcard("f?o");
        assert!(p.matches(&fold(b"foo")));
        assert!(p.matches(&fold(b"fao")));
        assert!(!p.matches(&fold(b"fo")));
        assert!(!p.matches(&fold(b"fooo")));
    }

    #[test]
    fn ext_function_splits_and_drops_the_dot() {
        assert_eq!(
            tests("ext:JPG;png;.Gif"),
            [Test::Extension(vec![
                "JPG".to_string(),
                "png".to_string(),
                "Gif".to_string()
            ])]
        );
    }

    #[test]
    fn file_and_folder_kinds() {
        assert_eq!(tests("file:"), [Test::Kind(Kind::File)]);
        assert_eq!(tests("folder:"), [Test::Kind(Kind::Folder)]);
        assert_eq!(
            tests("file:report"),
            [name("report"), Test::Kind(Kind::File)]
        );
    }

    /// A later `ext:`, `size:` or kind replaces an earlier one, as before the
    /// query had clauses; AND-ing them would make `ext:cs ext:txt` match
    /// nothing.
    #[test]
    fn a_later_filter_replaces_an_earlier_one() {
        assert_eq!(
            tests("ext:cs size:1 file: a ext:txt size:2 folder:"),
            [
                name("a"),
                Test::Kind(Kind::Folder),
                Test::Size(SizeRange { min: 2, max: 2 }),
                Test::Extension(vec!["txt".to_string()]),
            ]
        );
        // An empty list replaces nothing.
        assert_eq!(
            tests("ext:cs ext:;"),
            [Test::Extension(vec!["cs".to_string()])]
        );
    }

    fn size(query: &str) -> SizeRange {
        match tests(query)[..] {
            [Test::Size(range)] => range,
            ref other => panic!("{query}: {other:?}"),
        }
    }

    #[test]
    fn size_operators_ranges_and_constants() {
        let range = |min, max| SizeRange { min, max };
        assert_eq!(size("size:>1mb"), range(MB + 1, u64::MAX));
        assert_eq!(size("size:>=1mb"), range(MB, u64::MAX));
        assert_eq!(size("size:1mb..10mb"), range(MB, 10 * MB));
        assert_eq!(size("size:tiny"), range(0, 10 * KB));
        assert_eq!(size("size:1024"), range(1024, 1024));
        assert_eq!(size("size:..500"), range(0, 500));
    }

    #[test]
    fn unsupported_operators_rejected_unquoted_but_literal_when_quoted() {
        assert_eq!(
            parse_query("a|b"),
            Err(QueryError::UnsupportedOperator('|'))
        );
        assert_eq!(
            parse_query("!foo"),
            Err(QueryError::UnsupportedOperator('!'))
        );
        // `<`/`>` are allowed (comparison operators); "a<b" is literal text.
        assert_eq!(tests("a<b"), [name("a<b")]);
        // Quoted → literal, no rejection.
        assert_eq!(tests("\"a|b\""), [name("a|b")]);
    }

    #[test]
    fn unknown_function_is_literal_text() {
        // `parent:` is not supported in v1 → treated as literal name text.
        assert_eq!(tests("parent:foo"), [name("parent:foo")]);
    }

    #[test]
    fn malformed_size_is_reported() {
        assert!(matches!(
            parse_query("size:abc"),
            Err(QueryError::Malformed { what: "size:", .. })
        ));
    }

    #[test]
    fn empty_query_has_no_text_constraints() {
        assert!(q("").is_text_empty());
        assert!(q("   ").is_text_empty());
    }

    mod arbitrary {
        use super::*;
        use proptest::prelude::*;

        /// Query text built from the parser's own syntax and arbitrary
        /// characters, so that functions, ranges and wildcards are reached.
        fn arb_query() -> impl Strategy<Value = String> {
            let fragment = prop_oneof![
                prop::sample::select(vec![
                    "size:", "ext:", "path:", "file:", "folder:", "case:", "case:0", "..", ">=",
                    "<", "=", "*", "?", ";", ".", "\"", " ", "\\", "/", "kb", "gb", "tiny",
                ])
                .prop_map(str::to_string),
                "[0-9]{1,24}",
                "\\PC{0,4}",
            ];
            prop::collection::vec(fragment, 0..12).prop_map(|parts| parts.concat())
        }

        proptest! {
            /// The daemon parses the query text of a protocol 1 request, so
            /// any text can arrive, and a panic would fail the query instead
            /// of reporting it.
            #[test]
            fn arbitrary_text_errors_instead_of_panicking(
                input in arb_query(),
                match_case in any::<bool>(),
            ) {
                let _ = parse_query_with_case(&input, match_case);
            }
        }
    }
}
