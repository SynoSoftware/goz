//! Query tokenizer and grammar for gozler's search syntax.
//!
//! Supported: space-separated terms (implicit AND), `"quoted phrases"`,
//! `*`/`?` wildcards (anchored to the whole name), path terms (a term
//! containing `\` or `/`, quoted or not, or a `path:` value; a path term with
//! a wildcard is anchored to the whole full path), the functions in
//! `SEARCH_FUNCTIONS`, `!` before a term to exclude it, and `|` between terms
//! to match either. As in es, `|` binds tighter than the space, so `a|b c` is
//! `(a or b) and c`, and quotes make operators and spaces plain text, also in
//! a function's value: `path:"C:\Program Files"`. In a `regex:` value, `|`
//! and `!` belong to the expression without quotes.
//!
//! The parser writes every path the way the daemon compares it: `/` becomes
//! `\`, and a path term that starts with `.\` or `..\`, and a relative
//! `parent:` folder, are joined to the working directory
//! ([`ParseOptions::working_dir`]). A regular expression is never rewritten.
//!
//! A `dm:` value is resolved to UTC bounds here, from the query start
//! ([`ParseOptions::query_start`]), by the date resolver in `dates`.
//!
//! Refused as invalid input, because each would otherwise search text that
//! no name contains: an unknown function such as `dc:`, an unquoted `<` or
//! `>` outside a function value (es groups terms with them), an operator
//! without its term, and a relative path when there is no working directory.
//!
//! The parser turns the text into a [`ParsedQuery`], which the client sends to
//! the daemon as it is: AND-ed clauses, each an OR of terms. Apart from those
//! paths, terms keep the text as typed; the daemon folds them when it
//! compiles the query ([`super::CompiledQuery`]).

use super::dates::{DateRange, QueryStart, date_range};
use crate::attributes::Attributes;
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
/// entry whose size is unknown or a folder, and a date test an entry whose
/// date is unknown, negated or not, so a negated size test does not report an
/// unread file as small.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Term {
    pub negated: bool,
    pub test: Test,
}

/// What a [`Term`] tests. Text is as typed, apart from paths, which hold `\`
/// separators and no relative form; the daemon folds it unless the query
/// matches case.
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
    /// The entry is directly in this folder: its full path without its last
    /// separator and name equals this absolute path, a trailing separator
    /// ignored. From protocol 3.
    Parent(String),
    /// The name contains this text as a whole word: with no letter or digit
    /// right before or after it. From protocol 4.
    NameWholeWord(String),
    /// The full path contains this text as a whole word. From protocol 4.
    PathWholeWord(String),
    /// This regular expression, in the `regex` crate's syntax, matches
    /// somewhere in the name. From protocol 4.
    NameRegex(String),
    /// This regular expression matches somewhere in the full path. From
    /// protocol 4.
    PathRegex(String),
    /// The extension is one of these, without the dot, compared ignoring case
    /// whatever the query's case.
    Extension(Vec<String>),
    /// The file's size is in this range.
    Size(SizeRange),
    Kind(Kind),
    /// The date modified is in this range, resolved by the client from its
    /// clock and time zone. From protocol 5.
    DateModified(DateRange),
    /// The entry has every one of these attributes. From protocol 6.
    Attributes(Attributes),
}

impl ParsedQuery {
    /// Whether a term tests the size or the date modified, so an answer from
    /// a volume whose sizes and dates could not be read may leave files out.
    pub fn tests_size_or_date(&self) -> bool {
        self.clauses
            .iter()
            .flat_map(|c| &c.terms)
            .any(|t| matches!(t.test, Test::Size(_) | Test::DateModified(_)))
    }

    /// `true` when no term tests text: the query matches everything its
    /// other tests let through.
    #[cfg(test)]
    pub(crate) fn is_text_empty(&self) -> bool {
        !self.clauses.iter().flat_map(|c| &c.terms).any(|t| {
            matches!(
                t.test,
                Test::Name(_)
                    | Test::Path(_)
                    | Test::NameWildcard(_)
                    | Test::PathWildcard(_)
                    | Test::NameWholeWord(_)
                    | Test::PathWholeWord(_)
                    | Test::NameRegex(_)
                    | Test::PathRegex(_)
            )
        })
    }
}

/// A query that cannot be executed as written.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum QueryError {
    /// An unquoted `<` or `>` outside a function value, which es reads as
    /// grouping.
    #[error("'{0}' is not supported: gozler does not group terms with < and >")]
    UnsupportedOperator(char),
    /// A `name:` term whose name is no search function; carries the name.
    #[error("unknown search function: {0}:")]
    UnknownFunction(String),
    /// A value or part of the query that does not parse; `what` names it,
    /// ending in a colon, and `detail` says what is wrong and how to fix it.
    #[error("{what} {detail}")]
    Malformed { what: &'static str, detail: String },
    /// A relative path, which only a working directory can resolve, in a
    /// query parsed without one; carries the path.
    #[error(
        "{0} is a relative path, and this query has no working directory to resolve it; give the full path"
    )]
    RelativePath(String),
    /// A `dm:` term in a query parsed without a query start to resolve it.
    #[error("dm: needs the time the search starts, and this query was parsed without one")]
    NoQueryStart,
    /// More terms than a query may hold; carries the query's count.
    #[error("the query has {0} terms; a query may have at most {max}", max = super::engine::MAX_QUERY_TERMS)]
    TooManyTerms(usize),
    /// More regular expressions than a query may hold; carries the query's
    /// count.
    #[error(
        "the query has {0} regular expressions; a query may have at most {max}",
        max = super::engine::MAX_QUERY_REGEXES
    )]
    TooManyRegexes(usize),
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Function {
    Ext,
    Size,
    DateModified,
    Path,
    Parent,
    NoPath,
    WholeWord,
    Regex,
    Attributes,
    File,
    Folder,
    /// Handled by [`case_directive`] before the other functions; a `case:`
    /// value it does not accept is searched as text.
    Case,
}

/// Every search function, in the order the help lists them. A `name:` token
/// that names none of them is refused ([`QueryError::UnknownFunction`]).
pub(crate) const SEARCH_FUNCTIONS: &[SearchFunction] = &[
    SearchFunction {
        names: &["ext"],
        function: Function::Ext,
        example: "cs;txt",
        help: "Extension is one in the list; ext: alone finds files without one",
    },
    SearchFunction {
        names: &["size"],
        function: Function::Size,
        example: ">1mb",
        help: "File size; also <, <=, >=, =, ranges such as 1mb..10mb, and empty, \
               tiny, small, medium, large, huge or gigantic",
    },
    SearchFunction {
        names: &["dm", "datemodified", "date-modified"],
        function: Function::DateModified,
        example: "today",
        help: "Date modified: today, yesterday, YYYY-MM-DD or Ndays; also <, <=, >, \
               >= and ranges such as 2026-01-01..2026-03-31",
    },
    SearchFunction {
        names: &["path"],
        function: Function::Path,
        example: "text",
        help: "Full path contains text; with * or ?, the whole full path must match",
    },
    SearchFunction {
        names: &["parent"],
        function: Function::Parent,
        example: r"C:\repo",
        help: "Only the files and folders directly in this folder; a relative folder \
               starts from the current folder",
    },
    SearchFunction {
        names: &["nopath"],
        function: Function::NoPath,
        example: "text",
        help: "Name contains text, even with -p; with * or ?, the whole name must match",
    },
    SearchFunction {
        names: &["ww"],
        function: Function::WholeWord,
        example: "word",
        help: "Name contains word, with no letter or digit right before or after it; \
               with \\ or / in the value, the full path must contain it",
    },
    SearchFunction {
        names: &["regex"],
        function: Function::Regex,
        example: r"^test\d+\.cs$",
        help: "Name matches a regular expression in Rust syntax; Notes has the details",
    },
    SearchFunction {
        names: &["attrib", "attributes", "attr"],
        function: Function::Attributes,
        example: "hs",
        help: "Has every attribute in the value, here hidden and system; letters in \
               either case; !attrib:h excludes hidden items",
    },
    SearchFunction {
        names: &["file"],
        function: Function::File,
        example: "",
        help: "Files only; file:text also matches text",
    },
    SearchFunction {
        names: &["folder", "dir"],
        function: Function::Folder,
        example: "",
        help: "Folders only; folder:text also matches text",
    },
    SearchFunction {
        names: &["case"],
        function: Function::Case,
        example: "",
        help: "Match case; case:0 turns it off",
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

/// How [`parse_query_with`] reads a query, beyond its text: what the command
/// line's switches and working directory say.
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseOptions<'a> {
    /// es `-case`: whether text terms match case unless a `case:` directive
    /// says otherwise.
    pub match_case: bool,
    /// es `-p`: whether text terms without a separator match the full path,
    /// as `path:` values do, instead of the name. `nopath:` still matches the
    /// name.
    pub match_path: bool,
    /// es `-ww`: whether text terms without a wildcard match whole words, as
    /// `ww:` values do.
    pub whole_word: bool,
    /// The absolute folder that a path term starting with `.\` or `..\`, and
    /// a relative `parent:` folder, are below. `None` refuses them with
    /// [`QueryError::RelativePath`]: the daemon parses a protocol 1 query
    /// without the client's working directory.
    pub working_dir: Option<&'a str>,
    /// What `dm:` values resolve against. `None` refuses them with
    /// [`QueryError::NoQueryStart`].
    pub query_start: Option<&'a QueryStart>,
}

/// Parses a case-insensitive query string, without a working directory or a
/// query start, into a [`ParsedQuery`]; a `case:` directive in the query makes
/// it case-sensitive.
pub fn parse_query(input: &str) -> Result<ParsedQuery, QueryError> {
    parse_query_with(input, ParseOptions::default())
}

/// Parses a query string into a [`ParsedQuery`], read as `options` say. A
/// `case:` directive in the query overrides [`ParseOptions::match_case`], so
/// [`ParsedQuery::match_case`] is the one value that both matching and the
/// `-path` scope lookup follow.
///
/// Each group of terms joined by `|` is a clause, so the groups are AND-ed.
/// A later `ext:`, `size:`, `file:` or `folder:` that stands alone, without
/// `!` or `|`, replaces an earlier one.
pub fn parse_query_with(input: &str, options: ParseOptions) -> Result<ParsedQuery, QueryError> {
    let groups = groups(lex(input)?)?;

    // The last `case:` directive wins.
    let match_case = groups
        .iter()
        .flatten()
        .filter(|item| item.word.quote_at.is_none())
        .filter_map(|item| case_directive(&item.word.text))
        .next_back()
        .unwrap_or(options.match_case);

    let mut clauses = Vec::new();
    let mut kind = None;
    let mut size = None;
    let mut ext = None;
    for group in &groups {
        if let [
            Item {
                negated: false,
                word,
            },
        ] = group.as_slice()
        {
            match operand(word, &options)? {
                Operand::Text(test) => clauses.push(Clause::single(test)),
                Operand::Kind(new_kind, text) => {
                    kind = Some(new_kind);
                    clauses.extend(text.map(Clause::single));
                }
                Operand::Size(range) => size = Some(range),
                Operand::Extensions(extensions) => ext = Some(extensions),
                Operand::Nothing => {}
            }
            continue;
        }
        let alternatives = group
            .iter()
            .map(|item| {
                let terms = operand(&item.word, &options)?.terms();
                if terms.is_empty() {
                    return Err(malformed_expression(&format!(
                        "{} tests nothing, so ! and | cannot apply to it",
                        item.word.text
                    )));
                }
                Ok(if item.negated {
                    // The negated operand passes when any one of its terms
                    // fails.
                    let negated = terms
                        .into_iter()
                        .map(|term| Term {
                            negated: !term.negated,
                            test: term.test,
                        })
                        .collect();
                    vec![Clause { terms: negated }]
                } else {
                    terms
                        .into_iter()
                        .map(|term| Clause { terms: vec![term] })
                        .collect()
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        clauses.extend(any_of(alternatives)?);
    }

    clauses.extend(kind.map(|kind| Clause::single(Test::Kind(kind))));
    clauses.extend(size.map(|range| Clause::single(Test::Size(range))));
    let ext_terms = ext.map(Extensions::terms).unwrap_or_default();
    clauses.extend(
        ext_terms
            .into_iter()
            .map(|term| Clause { terms: vec![term] }),
    );
    Ok(ParsedQuery {
        clauses,
        match_case,
    })
}

/// The clauses that pass when any of `alternatives` passes, where each
/// alternative is clauses that must all pass. By distribution, there is one
/// clause for each way to pick one clause of every alternative, and it holds
/// the picked terms: `a|file:b` becomes `(a or file:) and (a or b)`.
fn any_of(alternatives: Vec<Vec<Clause>>) -> Result<Vec<Clause>, QueryError> {
    // The clauses multiply, so a group past the term cap is refused before it
    // is built, which could otherwise take memory exponential in its length.
    // `CompiledQuery::new` refuses the whole query, which has at least these
    // terms, anyway.
    let count = alternatives
        .iter()
        .fold(1usize, |n, alternative| n.saturating_mul(alternative.len()));
    let terms = alternatives.iter().fold(0usize, |sum, alternative| {
        let picks = count / alternative.len().max(1);
        let terms: usize = alternative.iter().map(|clause| clause.terms.len()).sum();
        sum.saturating_add(terms.saturating_mul(picks))
    });
    if terms > super::engine::MAX_QUERY_TERMS {
        return Err(QueryError::TooManyTerms(terms));
    }

    let mut clauses = vec![Clause { terms: Vec::new() }];
    for alternative in &alternatives {
        clauses = clauses
            .iter()
            .flat_map(|picked| {
                alternative.iter().map(move |clause| Clause {
                    terms: [picked.terms.as_slice(), clause.terms.as_slice()].concat(),
                })
            })
            .collect();
    }
    Ok(clauses)
}

/// What one term of the query stands for.
enum Operand {
    /// A test that is a clause of its own: a name, path, date or attribute
    /// test.
    Text(Test),
    /// `file:` or `folder:`, and the text after it.
    Kind(Kind, Option<Test>),
    Size(SizeRange),
    Extensions(Extensions),
    /// A `case:` directive or an empty `path:`, `nopath:` or `attrib:`, which
    /// test nothing.
    Nothing,
}

impl Operand {
    /// The terms that must all pass for the operand to match.
    fn terms(self) -> Vec<Term> {
        let positive = |test| Term {
            negated: false,
            test,
        };
        match self {
            Operand::Text(test) => vec![positive(test)],
            Operand::Kind(kind, text) => std::iter::once(Test::Kind(kind))
                .chain(text)
                .map(positive)
                .collect(),
            Operand::Size(range) => vec![positive(Test::Size(range))],
            Operand::Extensions(extensions) => extensions.terms(),
            Operand::Nothing => Vec::new(),
        }
    }
}

/// What an `ext:` term asks for.
enum Extensions {
    /// One of these, without the dot.
    OneOf(Vec<String>),
    /// No extension: `ext:` with none listed. As in es, where folders have
    /// no extension, only files match.
    Without,
}

impl Extensions {
    /// The terms that must all pass. `Without` is written with tests the
    /// protocol already carries, so a daemon of the same protocol version
    /// answers it: a file whose name `*.*` does not match has no dot.
    fn terms(self) -> Vec<Term> {
        match self {
            Extensions::OneOf(exts) => vec![Term {
                negated: false,
                test: Test::Extension(exts),
            }],
            Extensions::Without => vec![
                Term {
                    negated: false,
                    test: Test::Kind(Kind::File),
                },
                Term {
                    negated: true,
                    test: Test::NameWildcard("*.*".to_string()),
                },
            ],
        }
    }
}

/// What `word` stands for, or why it cannot be searched.
fn operand(word: &Word, options: &ParseOptions) -> Result<Operand, QueryError> {
    let text = word.text.as_str();
    if word.quote_at.is_none() && case_directive(text).is_some() {
        return Ok(Operand::Nothing);
    }
    if let Some((name, value)) = function_parts(word) {
        // A function value may hold `<` and `>`, as `size:<1mb` does, and its
        // quotes only delimit it, so `*` and `?` in it stay wildcards.
        let text_operand = |test: Option<Test>| test.map_or(Operand::Nothing, Operand::Text);
        match function_named(name) {
            Some(Function::Ext) => return Ok(Operand::Extensions(extensions(value))),
            Some(Function::Size) => return Ok(Operand::Size(parse_size(value)?)),
            Some(Function::DateModified) => {
                let start = options.query_start.ok_or(QueryError::NoQueryStart)?;
                return Ok(Operand::Text(Test::DateModified(date_range(value, start)?)));
            }
            Some(Function::Path) => {
                let path = path_text(value, options.working_dir)?;
                let form = Form::of(has_wildcard(value), options);
                return Ok(text_operand(text_of(path, true, form)));
            }
            Some(Function::NoPath) => {
                let name = value.replace('/', r"\");
                let form = Form::of(has_wildcard(value), options);
                return Ok(text_operand(text_of(name, false, form)));
            }
            Some(Function::WholeWord) => {
                let options = ParseOptions {
                    whole_word: true,
                    ..*options
                };
                return Ok(text_operand(text_test(value, false, &options)?));
            }
            Some(Function::Regex) => {
                return Ok(text_operand(regex_test(value, options.match_path)));
            }
            Some(Function::Parent) => {
                return Ok(Operand::Text(Test::Parent(parent_folder(
                    value,
                    options.working_dir,
                )?)));
            }
            Some(Function::Attributes) => {
                let attributes =
                    Attributes::from_letters(value).map_err(|c| QueryError::Malformed {
                        what: "attrib:",
                        detail: format!(
                            "{c} is not an attribute letter; the letters are {}",
                            Attributes::all_letters()
                        ),
                    })?;
                return Ok(if attributes.is_empty() {
                    Operand::Nothing
                } else {
                    Operand::Text(Test::Attributes(attributes))
                });
            }
            Some(Function::File) => {
                return Ok(Operand::Kind(Kind::File, text_test(value, false, options)?));
            }
            Some(Function::Folder) => {
                return Ok(Operand::Kind(
                    Kind::Folder,
                    text_test(value, false, options)?,
                ));
            }
            // A `case:` value that is not a directive is searched as text.
            Some(Function::Case) => {}
            None => return Err(QueryError::UnknownFunction(name.to_string())),
        }
    }
    if let Some(angle) = word.angle {
        return Err(QueryError::UnsupportedOperator(angle));
    }
    // Only `""` is an empty word; it searches for the empty phrase, which
    // every name contains.
    let test = text_test(text, word.quote_at.is_some(), options)?;
    Ok(Operand::Text(test.unwrap_or(Test::Name(String::new()))))
}

/// The name and the value of a function term. The text before the word's
/// first colon names a function when it is unquoted and could be the name of
/// one; a single letter is a drive, as in `C:\repo`.
fn function_parts(word: &Word) -> Option<(&str, &str)> {
    let colon = word.text.find(':')?;
    if word.quote_at.is_some_and(|at| at <= colon) {
        return None;
    }
    let name = &word.text[..colon];
    let could_be_function = name.len() > 1
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    could_be_function.then(|| (name, &word.text[colon + 1..]))
}

/// The search function `name` names, compared case-insensitively.
fn function_named(name: &str) -> Option<Function> {
    SEARCH_FUNCTIONS
        .iter()
        .find(|row| row.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
        .map(|row| row.function)
}

/// The extensions of an `ext:` value: a list separated by `;`, each without
/// its leading dots.
fn extensions(value: &str) -> Extensions {
    let exts: Vec<String> = value
        .split(';')
        .map(|ext| ext.trim_start_matches('.'))
        .filter(|ext| !ext.is_empty())
        .map(str::to_string)
        .collect();
    if exts.is_empty() {
        Extensions::Without
    } else {
        Extensions::OneOf(exts)
    }
}

/// The test of a text term, `None` for an empty one. A term with a separator
/// matches the full path, any other term the name, or with -p the full path
/// too, and a wildcard anchors either to the whole of what it matches. A
/// `quoted` term without a separator is a phrase, whose `*` and `?` are text;
/// with a separator it is a path term like any other, because a literal `*`
/// can never match a path.
fn text_test(text: &str, quoted: bool, options: &ParseOptions) -> Result<Option<Test>, QueryError> {
    let is_path = text.contains(['\\', '/']);
    let wildcard = has_wildcard(text) && (is_path || !quoted);
    let text = if is_path {
        path_text(text, options.working_dir)?
    } else {
        text.to_string()
    };
    Ok(text_of(
        text,
        is_path || options.match_path,
        Form::of(wildcard, options),
    ))
}

/// How a text term matches what it tests.
#[derive(Clone, Copy)]
enum Form {
    Substring,
    WholeWord,
    /// A pattern of `*` and `?` that matches the whole of it.
    Wildcard,
}

impl Form {
    /// A wildcard already matches a whole name or path, so whole-word
    /// matching leaves it unchanged.
    fn of(wildcard: bool, options: &ParseOptions) -> Self {
        match (wildcard, options.whole_word) {
            (true, _) => Form::Wildcard,
            (false, true) => Form::WholeWord,
            (false, false) => Form::Substring,
        }
    }
}

/// A test of the full path, or else of the name, for `text`, matched as
/// `form` says; `None` when `text` is empty.
fn text_of(text: String, full_path: bool, form: Form) -> Option<Test> {
    if text.is_empty() {
        return None;
    }
    Some(match (form, full_path) {
        (Form::Substring, true) => Test::Path(text),
        (Form::Substring, false) => Test::Name(text),
        (Form::WholeWord, true) => Test::PathWholeWord(text),
        (Form::WholeWord, false) => Test::NameWholeWord(text),
        (Form::Wildcard, true) => Test::PathWildcard(text),
        (Form::Wildcard, false) => Test::NameWildcard(text),
    })
}

/// The test of a regular expression, from `regex:` or `-r`; `None` for an
/// empty one. As in es, it matches the name, and the full path with -p
/// (`match_path`) or when it holds an escaped backslash, `\\`, which can
/// only match a separator. The pattern is never rewritten: its `/` stays a
/// `/`, which no full path holds.
pub(crate) fn regex_test(pattern: &str, match_path: bool) -> Option<Test> {
    if pattern.is_empty() {
        return None;
    }
    let pattern = pattern.to_string();
    Some(if match_path || has_escaped_backslash(&pattern) {
        Test::PathRegex(pattern)
    } else {
        Test::NameRegex(pattern)
    })
}

/// Whether `pattern` holds `\\`. Each `\` escapes the character after it, so
/// the pair is read together and `\\d` is a backslash and a `d`.
fn has_escaped_backslash(pattern: &str) -> bool {
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.next() == Some('\\') {
            return true;
        }
    }
    false
}

/// A path term's text as the daemon compares it: every `/` a `\`, and one
/// that starts with `.\` or `..\` joined to `working_dir`, keeping a trailing
/// separator, which limits a path term to what is below the folder.
fn path_text(text: &str, working_dir: Option<&str>) -> Result<String, QueryError> {
    let text = text.replace('/', r"\");
    if !(text.starts_with(r".\") || text.starts_with(r"..\")) {
        return Ok(text);
    }
    let working_dir = working_dir.ok_or_else(|| QueryError::RelativePath(text.clone()))?;
    let mut path = crate::winpath::absolute(&text, working_dir);
    if text.ends_with('\\') && !path.ends_with('\\') {
        path.push('\\');
    }
    Ok(path)
}

/// A `parent:` folder as the daemon compares it: absolute, joined to
/// `working_dir` when it is not, and every `/` a `\`.
fn parent_folder(value: &str, working_dir: Option<&str>) -> Result<String, QueryError> {
    let value = value.replace('/', r"\");
    if value.is_empty() {
        return Err(QueryError::Malformed {
            what: "parent:",
            detail: "needs a folder".to_string(),
        });
    }
    match working_dir {
        Some(working_dir) => Ok(crate::winpath::absolute(&value, working_dir)),
        None if crate::winpath::is_absolute(&value) => Ok(value),
        None => Err(QueryError::RelativePath(value)),
    }
}

fn has_wildcard(text: &str) -> bool {
    text.contains('*') || text.contains('?')
}

/// One term of the query as typed, without its quotes.
#[derive(Default)]
struct Word {
    text: String,
    /// Where in `text` the first quoted part starts; `None` when nothing is
    /// quoted. A function's name and colon lie before it.
    quote_at: Option<usize>,
    /// The first unquoted `<` or `>`.
    angle: Option<char>,
}

enum Lexeme {
    Word(Word),
    /// `!`
    Not,
    /// `|`
    Or,
}

impl Word {
    /// Whether the word so far is a `regex:` term, whose `|` and `!` belong
    /// to the expression, as in es, where `|` is the regex alternation.
    fn is_regex(&self) -> bool {
        function_parts(self).is_some_and(|(name, _)| function_named(name) == Some(Function::Regex))
    }
}

/// Splits the query into words and operators. Unquoted whitespace and `|`
/// end a word, and an unquoted `!` must start one, except in a `regex:`
/// value; inside double quotes every character is text.
fn lex(input: &str) -> Result<Vec<Lexeme>, QueryError> {
    let mut lexemes = Vec::new();
    let mut word: Option<Word> = None;
    let mut in_quotes = false;
    for ch in input.chars() {
        if in_quotes {
            if ch == '"' {
                in_quotes = false;
            } else {
                word.get_or_insert_default().text.push(ch);
            }
            continue;
        }
        if matches!(ch, '|' | '!')
            && let Some(regex) = word.as_mut().filter(|word| word.is_regex())
        {
            regex.text.push(ch);
            continue;
        }
        match ch {
            '"' => {
                let word = word.get_or_insert_default();
                if word.quote_at.is_none() {
                    word.quote_at = Some(word.text.len());
                }
                in_quotes = true;
            }
            '|' => {
                lexemes.extend(word.take().map(Lexeme::Word));
                lexemes.push(Lexeme::Or);
            }
            '!' if word.is_none() => lexemes.push(Lexeme::Not),
            '!' => {
                return Err(malformed_expression(
                    "! must start a term; quote it to search for it",
                ));
            }
            c if c.is_whitespace() => lexemes.extend(word.take().map(Lexeme::Word)),
            c => {
                let word = word.get_or_insert_default();
                if matches!(c, '<' | '>') && word.angle.is_none() {
                    word.angle = Some(c);
                }
                word.text.push(c);
            }
        }
    }
    lexemes.extend(word.map(Lexeme::Word));
    Ok(lexemes)
}

/// A word after its `!` operators; an odd number of them negates it.
struct Item {
    negated: bool,
    word: Word,
}

/// The lexemes as groups of items joined by `|`; the query AND-s the groups.
fn groups(lexemes: Vec<Lexeme>) -> Result<Vec<Vec<Item>>, QueryError> {
    let or_needs_terms = || malformed_expression("| needs a term on each side");
    let mut groups: Vec<Vec<Item>> = Vec::new();
    let mut nots = 0usize;
    // After a `|`, the next word joins the last group.
    let mut joined = false;
    for lexeme in lexemes {
        match lexeme {
            Lexeme::Not => nots += 1,
            Lexeme::Or => {
                if groups.is_empty() || joined || nots > 0 {
                    return Err(or_needs_terms());
                }
                joined = true;
            }
            Lexeme::Word(word) => {
                let item = Item {
                    negated: nots % 2 == 1,
                    word,
                };
                match groups.last_mut() {
                    Some(group) if joined => group.push(item),
                    _ => groups.push(vec![item]),
                }
                nots = 0;
                joined = false;
            }
        }
    }
    if joined {
        return Err(or_needs_terms());
    }
    if nots > 0 {
        return Err(malformed_expression("! needs a term after it"));
    }
    Ok(groups)
}

fn malformed_expression(detail: &str) -> QueryError {
    QueryError::Malformed {
        what: "query:",
        detail: detail.to_string(),
    }
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
    let malformed = |bad: &str| QueryError::Malformed {
        what: "size:",
        detail: format!("{bad} is not a size; write a number with kb, mb, gb or tb, such as 10mb"),
    };

    // Named constants, as Everything defines them: each above the previous
    // one's maximum, so a size on a boundary matches one constant.
    let above = |lower: u64, max: u64| {
        Ok(SizeRange {
            min: lower + 1,
            max,
        })
    };
    match v.to_ascii_lowercase().as_str() {
        "empty" => return Ok(SizeRange { min: 0, max: 0 }),
        "tiny" => return above(0, 10 * KB),
        "small" => return above(10 * KB, 100 * KB),
        "medium" => return above(100 * KB, MB),
        "large" => return above(MB, 16 * MB),
        "huge" => return above(16 * MB, 128 * MB),
        "gigantic" => return above(128 * MB, u64::MAX),
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

    /// The tests of a query whose clauses each hold one positive term.
    fn tests(s: &str) -> Vec<Test> {
        tests_with(s, ParseOptions::default())
    }

    /// [`tests`] of a query read as `options` say.
    fn tests_with(s: &str, options: ParseOptions) -> Vec<Test> {
        parse_query_with(s, options)
            .unwrap_or_else(|e| panic!("{s}: {e}"))
            .clauses
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
        let case = |text, match_case| {
            let options = ParseOptions {
                match_case,
                ..ParseOptions::default()
            };
            parse_query_with(text, options).unwrap().match_case
        };
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
        assert_eq!(size("size:1024"), range(1024, 1024));
        assert_eq!(size("size:..500"), range(0, 500));
    }

    /// As in Everything, a size constant starts above the previous one's
    /// maximum, so a file of exactly 10 KB is tiny and not small, and an
    /// empty file is not tiny (#20).
    #[test]
    fn a_size_on_a_boundary_matches_one_constant() {
        let range = |min, max| SizeRange { min, max };
        assert_eq!(size("size:tiny"), range(1, 10 * KB));
        assert_eq!(size("size:small"), range(10 * KB + 1, 100 * KB));
    }

    fn clauses(s: &str) -> Vec<Vec<Term>> {
        q(s).clauses
            .into_iter()
            .map(|clause| clause.terms)
            .collect()
    }

    fn term(negated: bool, test: Test) -> Term {
        Term { negated, test }
    }

    /// `|` binds tighter than the space, as in es, and `!` negates the term
    /// after it, a function included. `!file:report` excludes the files whose
    /// names contain report, so either test failing passes, and a negated
    /// filter leaves the filter that stands alone in place.
    #[test]
    fn operators_build_clauses_of_possibly_negated_terms() {
        assert_eq!(
            clauses("a|b !c"),
            [
                vec![term(false, name("a")), term(false, name("b"))],
                vec![term(true, name("c"))],
            ]
        );
        assert_eq!(clauses("a | b"), clauses("a|b"));
        assert_eq!(clauses("!!a"), clauses("a"));
        assert_eq!(
            clauses("!file:report"),
            [vec![
                term(true, Test::Kind(Kind::File)),
                term(true, name("report"))
            ]]
        );
        let x = || term(false, name("x"));
        assert_eq!(
            clauses("x|file:report"),
            [
                vec![x(), term(false, Test::Kind(Kind::File))],
                vec![x(), term(false, name("report"))],
            ]
        );
        let ext = |e: &str| Test::Extension(vec![e.to_string()]);
        assert_eq!(
            clauses("ext:cs !ext:txt"),
            [vec![term(true, ext("txt"))], vec![term(false, ext("cs"))]]
        );
        // Quoted, an operator is text.
        assert_eq!(tests("\"a|b\" \"!c\""), [name("a|b"), name("!c")]);
    }

    /// An operator without its term, or a `!` inside a term, has no meaning
    /// that a search could answer without guessing.
    #[test]
    fn an_operator_without_its_term_is_refused() {
        for query in [
            "a|", "|a", "a||b", "!", "a !", "!|a", "a!b", "!case:", "a|path:",
        ] {
            assert!(
                matches!(
                    parse_query(query),
                    Err(QueryError::Malformed { what: "query:", .. })
                ),
                "{query}"
            );
        }
    }

    /// es groups terms with `<` and `>`, which gozler does not, and no name
    /// contains them; in a function value they compare.
    #[test]
    fn angle_brackets_are_refused_outside_a_function_value() {
        assert_eq!(
            parse_query("a<b"),
            Err(QueryError::UnsupportedOperator('<'))
        );
        assert_eq!(
            parse_query("<a b>|c"),
            Err(QueryError::UnsupportedOperator('<'))
        );
        assert_eq!(
            size("size:<1kb"),
            SizeRange {
                min: 0,
                max: KB - 1
            }
        );
        assert_eq!(tests("\"a<b\""), [name("a<b")]);
    }

    /// An unknown function was searched as text, which no name contains, so
    /// it answered nothing that looked valid (bug 2). A drive letter is not a
    /// function.
    #[test]
    fn an_unknown_function_is_refused() {
        assert_eq!(
            parse_query("x !DC:today"),
            Err(QueryError::UnknownFunction("DC".to_string()))
        );
        assert_eq!(tests(r"C:\repo"), [Test::Path(r"C:\repo".to_string())]);
    }

    /// Quotes delimit a function's value, as in es (bug 10); quoted before
    /// its colon, a term is a phrase.
    #[test]
    fn quotes_delimit_a_function_value() {
        assert_eq!(
            tests(r#"path:"C:\Program Files""#),
            [Test::Path(r"C:\Program Files".to_string())]
        );
        assert_eq!(tests(r#""path:x""#), [name("path:x")]);
    }

    fn path(s: &str) -> Test {
        Test::Path(s.to_string())
    }

    fn path_wildcard(s: &str) -> Test {
        Test::PathWildcard(s.to_string())
    }

    fn parent(s: &str) -> Test {
        Test::Parent(s.to_string())
    }

    /// Full paths hold `\`, so a `/` in a path term, a path wildcard or a
    /// `path:` or `parent:` value would never match (bug 9).
    #[test]
    fn a_slash_in_a_path_is_a_backslash() {
        assert_eq!(
            tests("docs/agents C:/repo/*.cs path:a/b parent:C:/repo"),
            [
                path(r"docs\agents"),
                path_wildcard(r"C:\repo\*.cs"),
                path(r"a\b"),
                parent(r"C:\repo"),
            ]
        );
    }

    /// A path term that starts with `.\` or `..\`, and a `parent:` folder
    /// that is not absolute, are below the working directory; a path term
    /// keeps its trailing separator, which limits it to what is below the
    /// folder. Without a working directory, as on the daemon's text path,
    /// each is refused, never searched as text that matches nothing.
    #[test]
    fn a_relative_path_is_below_the_working_directory() {
        let in_src = ParseOptions {
            working_dir: Some(r"C:\repo\src"),
            ..ParseOptions::default()
        };
        assert_eq!(
            tests_with(r"./ ..\docs\*.md docs\.\x parent:.. parent:sub", in_src),
            [
                path(r"C:\repo\src\"),
                path_wildcard(r"C:\repo\docs\*.md"),
                path(r"docs\.\x"),
                parent(r"C:\repo"),
                parent(r"C:\repo\src\sub"),
            ]
        );
        assert_eq!(
            parse_query(r"x .\src"),
            Err(QueryError::RelativePath(r".\src".to_string()))
        );
        assert_eq!(
            parse_query("parent:src"),
            Err(QueryError::RelativePath("src".to_string()))
        );
        assert!(matches!(
            parse_query("parent:"),
            Err(QueryError::Malformed {
                what: "parent:",
                ..
            })
        ));
    }

    /// A quoted term with a separator is a path term, wildcards included,
    /// because a literal `*` can never match a path; without one it is a
    /// phrase of the name, whose `*` is text (bug 10).
    #[test]
    fn a_quoted_term_with_a_separator_is_a_path_term() {
        assert_eq!(
            tests(r#""C:\Program Files (x86)\Microsoft" "C:\Program Files\*.cs" "a*b""#),
            [
                path(r"C:\Program Files (x86)\Microsoft"),
                path_wildcard(r"C:\Program Files\*.cs"),
                name("a*b"),
            ]
        );
    }

    /// `-p` makes every text term a path term, as es's Match Path does, and
    /// `nopath:` still matches the name.
    #[test]
    fn match_path_makes_text_terms_path_terms_except_nopath() {
        let match_path = ParseOptions {
            match_path: true,
            ..ParseOptions::default()
        };
        assert_eq!(
            tests_with(
                r#"report *.cs "two words" nopath:readme nopath:*.md"#,
                match_path
            ),
            [
                path("report"),
                path_wildcard("*.cs"),
                path("two words"),
                name("readme"),
                Test::NameWildcard("*.md".to_string()),
            ]
        );
    }

    /// `ww:` matches one term as whole words and `-ww` every text term; a
    /// wildcard, which already matches a whole name or path, stays a
    /// wildcard.
    #[test]
    fn whole_word_terms_keep_their_target() {
        let whole = |s: &str| Test::NameWholeWord(s.to_string());
        let whole_path = |s: &str| Test::PathWholeWord(s.to_string());
        assert_eq!(
            tests(r"ww:report ww:docs/agents ww:*.cs report"),
            [
                whole("report"),
                whole_path(r"docs\agents"),
                Test::NameWildcard("*.cs".to_string()),
                name("report"),
            ]
        );
        let whole_word = ParseOptions {
            whole_word: true,
            ..ParseOptions::default()
        };
        assert_eq!(
            tests_with(r#"report "two words" path:docs nopath:x *.cs"#, whole_word),
            [
                whole("report"),
                whole("two words"),
                whole_path("docs"),
                whole("x"),
                Test::NameWildcard("*.cs".to_string()),
            ]
        );
    }

    /// A regular expression keeps its text: its `|` and `!` belong to it, as
    /// es's parser gives them to it, and its `/` is not a separator. It
    /// matches the name, or the full path with -p or when it holds `\\`.
    #[test]
    fn a_regex_keeps_its_text_and_matches_the_path_only_with_a_backslash() {
        let regex = |s: &str| Test::NameRegex(s.to_string());
        let path_regex = |s: &str| Test::PathRegex(s.to_string());
        assert_eq!(
            tests(r#"regex:^a|b!$ regex:a/b regex:\d+ regex:C:\\repo\\ regex:[\\/]x REGEX:"a b""#),
            [
                regex("^a|b!$"),
                regex("a/b"),
                regex(r"\d+"),
                path_regex(r"C:\\repo\\"),
                path_regex(r"[\\/]x"),
                regex("a b"),
            ]
        );
        assert_eq!(
            clauses("x|regex:a|b !regex:c|d"),
            [
                vec![term(false, name("x")), term(false, regex("a|b"))],
                vec![term(true, regex("c|d"))],
            ]
        );
        let match_path = ParseOptions {
            match_path: true,
            ..ParseOptions::default()
        };
        assert_eq!(tests_with(r"regex:^C:", match_path), [path_regex("^C:")]);
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
                    "size:", "ext:", "path:", "parent:", "nopath:", "ww:", "regex:", "file:",
                    "folder:", "case:", "case:0", "dm:", "..", ">=", "<", ">", "=", "*", "?", ";",
                    ".", "\"", " ", "\\", "/", "!", "|", "kb", "gb", "tiny", "today", "days", "-",
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
            /// of reporting it. The query start is the earliest and the
            /// latest one jiff holds, so date arithmetic leaves its range.
            #[test]
            fn arbitrary_text_errors_instead_of_panicking(
                input in arb_query(),
                match_case in any::<bool>(),
                latest in any::<bool>(),
            ) {
                let start = QueryStart {
                    now: if latest { jiff::Timestamp::MAX } else { jiff::Timestamp::MIN },
                    time_zone: jiff::tz::TimeZone::UTC,
                };
                let options = ParseOptions {
                    match_case,
                    query_start: Some(&start),
                    ..ParseOptions::default()
                };
                let _ = parse_query_with(&input, options);
            }
        }
    }
}
