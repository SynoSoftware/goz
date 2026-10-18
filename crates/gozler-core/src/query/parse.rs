//! Query tokenizer and grammar for gozler's search syntax.
//!
//! Supported: space-separated terms (implicit AND), `"quoted phrases"`,
//! `*`/`?` wildcards (anchored to the whole name), path terms (a term
//! containing `\` or `/`, or a `path:` value; a path term with a wildcard is
//! anchored to the whole full path), the functions in `SEARCH_FUNCTIONS`,
//! `!` before a term to exclude it, and `|` between terms to match either.
//! As in es, `|` binds tighter than the space, so `a|b c` is `(a or b) and c`,
//! and quotes make operators and spaces plain text, also in a function's
//! value: `path:"C:\Program Files"`.
//!
//! Refused as invalid input, because each would otherwise search text that
//! no name contains: an unknown function such as `dm:`, an unquoted `<` or
//! `>` outside a function value (es groups terms with them), and an operator
//! without its term.
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
    /// An unquoted `<` or `>` outside a function value, which es reads as
    /// grouping.
    #[error("'{0}' is not supported: gozler does not group terms with < and >")]
    UnsupportedOperator(char),
    /// A `name:` term whose name is no search function; carries the name.
    #[error("unknown search function: {0}:")]
    UnknownFunction(String),
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
/// that names none of them is refused ([`QueryError::UnknownFunction`]).
pub(crate) const SEARCH_FUNCTIONS: &[SearchFunction] = &[
    SearchFunction {
        names: &["ext"],
        function: Function::Ext,
        example: "cs;txt",
        help: "extension, one of a list; ext: alone finds files without one",
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
/// Each group of terms joined by `|` is a clause, so the groups are AND-ed.
/// A later `ext:`, `size:`, `file:` or `folder:` that stands alone, without
/// `!` or `|`, replaces an earlier one.
pub fn parse_query_with_case(input: &str, match_case: bool) -> Result<ParsedQuery, QueryError> {
    let groups = groups(lex(input)?)?;

    // The last `case:` directive wins.
    let match_case = groups
        .iter()
        .flatten()
        .filter(|item| item.word.quote_at.is_none())
        .filter_map(|item| case_directive(&item.word.text))
        .next_back()
        .unwrap_or(match_case);

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
            match operand(word)? {
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
                let terms = operand(&item.word)?.terms();
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
    /// A name or path test.
    Text(Test),
    /// `file:` or `folder:`, and the text after it.
    Kind(Kind, Option<Test>),
    Size(SizeRange),
    Extensions(Extensions),
    /// A `case:` directive or an empty `path:`, which test nothing.
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
fn operand(word: &Word) -> Result<Operand, QueryError> {
    let text = word.text.as_str();
    if word.quote_at.is_none() && case_directive(text).is_some() {
        return Ok(Operand::Nothing);
    }
    if let Some((name, value)) = function_parts(word) {
        // A function value may hold `<` and `>`, as `size:<1mb` does.
        match function_named(name) {
            Some(Function::Ext) => return Ok(Operand::Extensions(extensions(value))),
            Some(Function::Size) => return Ok(Operand::Size(parse_size(value)?)),
            Some(Function::Path) => {
                return Ok(if has_wildcard(value) {
                    Operand::Text(Test::PathWildcard(value.to_string()))
                } else if value.is_empty() {
                    Operand::Nothing
                } else {
                    Operand::Text(Test::Path(value.to_string()))
                });
            }
            Some(Function::File) => return Ok(Operand::Kind(Kind::File, text_test(value))),
            Some(Function::Folder) => return Ok(Operand::Kind(Kind::Folder, text_test(value))),
            // A `case:` value that is not a directive is searched as text.
            Some(Function::Case) => {}
            None => return Err(QueryError::UnknownFunction(name.to_string())),
        }
    }
    if let Some(angle) = word.angle {
        return Err(QueryError::UnsupportedOperator(angle));
    }
    Ok(Operand::Text(if word.quote_at.is_some() {
        // Quoted phrases are literal filename substrings.
        Test::Name(word.text.clone())
    } else {
        text_test(text).expect("a word without quotes holds a character")
    }))
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

/// Splits the query into words and operators. Unquoted whitespace and `|`
/// end a word, and an unquoted `!` must start one; inside double quotes every
/// character is text.
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
        what: "expression",
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
    let malformed = |detail: &str| QueryError::Malformed {
        what: "size:",
        detail: detail.to_string(),
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
                    Err(QueryError::Malformed {
                        what: "expression",
                        ..
                    })
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
            parse_query("x !DM:today"),
            Err(QueryError::UnknownFunction("DM".to_string()))
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
                    "size:", "ext:", "path:", "file:", "folder:", "case:", "case:0", "dm:", "..",
                    ">=", "<", ">", "=", "*", "?", ";", ".", "\"", " ", "\\", "/", "!", "|", "kb",
                    "gb", "tiny",
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
