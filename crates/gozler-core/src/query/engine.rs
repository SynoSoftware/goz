//! The query executor: compile a [`ParsedQuery`] into a [`CompiledQuery`],
//! scan the index, apply scope and sort, and return the matches' rows or the
//! totals of every match.
//!
//! The scan matches each candidate against the index's precomputed folded name
//! (case-insensitive queries) and tests the cheapest clauses first: kind and
//! attributes, size and date, then name substrings and wildcards, then ext,
//! then regular expressions of the name, then (only for survivors) scope
//! ancestry, the path terms that one component of the path decides, and the
//! clauses that test the full path. The `paths` module answers both from a
//! cache of each folder built once per query: a component term reads its
//! folder's bits and its own name, with no path built, and any other path
//! term costs one copy of the full path per candidate. A match
//! holds no full path: it becomes a row, its entry and its file ID, which
//! [`super::ResultRows`] reads again when a page is built.

use super::dates::DateRange;
use super::order::{
    KeyValue, OrderFields, SortRecord, compare, folder_ranks, packed_folder_rank, primary_key,
};
use super::parse::{Kind, Malformed, ParsedQuery, QueryError, SizeRange, TermMatch, Test};
use super::paths::{ComponentBits, DirPaths, FolderCopy, NameForm, ResultNames, root_name};
use super::rows::VolumeRows;
use super::through::{LinkPaths, Target as LinkTarget, Through};
use super::wildcard::{CodePointBuf, OneName, Wildcard};
use crate::attributes::PackedAttributes;
use crate::fold::fold;
use crate::index::store::NameId;
use crate::index::{EntryView, SeenBy, VolumeIndex};
use crate::links::Links;
use crate::types::{EntryIdx, Frn, SortKey, SortSpec};
use memchr::memmem;
use regex::bytes::{Regex, RegexBuilder};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;
use std::ops::Range;

/// How many matches are files and how many are folders, and the space each
/// kind takes. The files' sum adds the sizes that are known and counts the
/// files whose size is not, so a search on a volume whose size pass stopped
/// still shows the space it found. A sum stops at `u64::MAX`, which only a
/// corrupt size makes it reach.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchSums {
    pub files: u64,
    /// The summed size in bytes of the matching files whose size is known,
    /// each name of a hard-linked file counted.
    pub file_bytes: u64,
    /// The matching files whose size is unknown, which `file_bytes` leaves
    /// out.
    pub files_without_size: u64,
    pub folders: u64,
    /// The summed size in bytes of the matching folders, without a folder
    /// that another matching folder holds, so it is the space they take.
    pub folder_bytes: u64,
    /// `folder_bytes` is only a lower bound: the size pass stopped on a
    /// volume searched, so a folder's size counts only the files it read
    /// ([`VolumeIndex::folder_sizes_at_least`]), or a matching folder's size
    /// is not known to the reader, as for a filtered user.
    pub folder_bytes_at_least: bool,
}

impl MatchSums {
    /// Adds `other`, the sums of another volume's matches.
    pub fn add(&mut self, other: MatchSums) {
        self.files += other.files;
        self.file_bytes = self.file_bytes.saturating_add(other.file_bytes);
        self.files_without_size += other.files_without_size;
        self.folders += other.folders;
        self.folder_bytes = self.folder_bytes.saturating_add(other.folder_bytes);
        self.folder_bytes_at_least |= other.folder_bytes_at_least;
    }
}

/// The [`MatchSums`] of `records`, as `seen_by` reads them. A folder inside
/// another matching folder adds to the count only, because its size is in
/// that folder's.
fn match_sums(index: &VolumeIndex, records: &[SortRecord], seen_by: &SeenBy) -> MatchSums {
    let folders: FxHashSet<EntryIdx> = records
        .iter()
        .map(|r| r.idx())
        .filter(|&idx| index.entry(idx).is_dir())
        .collect();
    // A loop in the parent links, which a move can make until the next
    // journal record breaks it, never reaches the root; the walk stops after
    // more folders than a Windows path can hold.
    const MAX_DEPTH: usize = 1 << 15;
    let held = |folder: EntryIdx| {
        let mut parent = index.entry(folder).parent();
        for _ in 0..MAX_DEPTH {
            if parent == crate::types::NIL || parent == index.root() {
                return false;
            }
            if folders.contains(&parent) {
                return true;
            }
            parent = index.entry(parent).parent();
        }
        false
    };
    let mut sums = MatchSums {
        folder_bytes_at_least: index.folder_sizes_at_least(),
        ..MatchSums::default()
    };
    for r in records {
        let entry = index.entry(r.idx());
        if entry.is_dir() {
            sums.folders += 1;
            if !held(r.idx()) {
                // A filtered user reads no folder's size. Otherwise only an
                // index whose size pass has not ended lacks one, and the
                // daemon searches none that holds a folder.
                match entry.size(seen_by) {
                    Some(size) => sums.folder_bytes = sums.folder_bytes.saturating_add(size),
                    None => sums.folder_bytes_at_least = true,
                }
            }
        } else {
            sums.files += 1;
            match entry.size(seen_by) {
                Some(size) => sums.file_bytes = sums.file_bytes.saturating_add(size),
                None => sums.files_without_size += 1,
            }
        }
    }
    sums
}

/// The number of matches and the summed size of the matching files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Totals {
    pub count: u64,
    /// Bytes. A file whose size is unknown adds nothing, and a folder adds
    /// nothing, because its size is the sum of its files.
    pub size: u64,
}

impl Totals {
    /// Adds `other`, failing rather than wrapping when the size passes
    /// `u64::MAX`, which only a corrupt size in the index can make it do.
    pub fn add(&mut self, other: Totals) -> Result<(), SizeOverflow> {
        self.size = self.size.checked_add(other.size).ok_or(SizeOverflow)?;
        self.count += other.count;
        Ok(())
    }
}

/// The summed size of the matching files does not fit in a `u64`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the total size of the matches does not fit in 64 bits")]
pub struct SizeOverflow;

/// The most terms one query may hold, counting each `ext:` value, and a
/// clause without terms as one. The engine tests each candidate against every
/// term under the volume's index read lock, so the cost of a query grows with
/// its term count; without a cap, one query from any local user holds that
/// lock, and the journal tail waiting behind it, for minutes. Real queries use
/// a few terms, and 256 still fits a long `ext:` list.
pub(crate) const MAX_QUERY_TERMS: usize = 256;

/// The most regular expressions one query may hold. Each compiled regex keeps
/// up to [`REGEX_SIZE_LIMIT`] for each of its forward and reverse automata, up
/// to [`REGEX_CACHE_LIMIT`] of lazy DFA cache for each, and a one-pass DFA of
/// up to 1 MiB that the `regex` crate does not let a caller lower, so the cap
/// keeps a query from any local user to tens of megabytes. One expression can
/// hold any number of alternatives.
pub(crate) const MAX_QUERY_REGEXES: usize = 8;

/// The largest compiled automaton of one regular expression, a tenth of the
/// `regex` crate's default: it still fits `\w{20}`, and a pattern past it is
/// refused as malformed.
const REGEX_SIZE_LIMIT: usize = 1 << 20;

/// The lazy DFA cache of one regular expression; past it, the crate clears
/// the cache or falls back to a slower engine, never fails.
const REGEX_CACHE_LIMIT: usize = 1 << 20;

/// A [`ParsedQuery`] ready to test entries: its text folded unless it matches
/// case, its wildcards and regular expressions compiled and its needles built
/// into finders, once per query instead of once per candidate.
///
/// The daemon compiles the query, not the client, because the index's folded
/// names come from the daemon's fold, and a client from another release can
/// fold some text differently. Compiling also refuses a query from any local
/// user that holds too many terms ([`QueryError::TooManyTerms`]) or regular
/// expressions ([`QueryError::TooManyRegexes`]), or a regular expression that
/// is malformed or compiles past its size limit.
pub struct CompiledQuery {
    match_case: bool,
    /// Every clause, cheapest first. The clauses from `component_clauses` on
    /// read the entry's [`ComponentBits`] and those from `path_clauses` on
    /// the full path, so both run after the scope check, and the path
    /// clauses last.
    clauses: Vec<CompiledClause>,
    component_clauses: usize,
    path_clauses: usize,
    /// An ordered filename proves every component and path clause; its
    /// parent still has to have a valid path.
    names_decide_path: bool,
    /// The matcher of each [`Check::Component`], by its bit.
    components: Vec<Matcher>,
    /// The bit and the needle of the [`Check::Component`] with the longest
    /// needle that is a clause on its own and not negated: every match has a
    /// path component that contains it. Drives the component walk of
    /// [`scan`] when no `prefilter` drives the arena scan.
    component_literal: Option<(u8, Vec<u8>)>,
    /// The longest literal that every matching name contains, from a clause
    /// of one positive name term, whole-word, word-start or name-start name
    /// term, name wildcard (`.dll` of `*.dll`) or single `ext:` value
    /// (`.ext`). Drives the folded-arena
    /// prefilter scan; `None` when no clause offers one (then the query
    /// iterates every entry).
    ///
    /// No other clause supplies it: a clause of several terms or a negated
    /// term lets through names without the literal, the text of a path term
    /// can lie in any folder of the path, the literal of `*\docs\*` is
    /// `\docs\`, which no name contains, and a regular expression is not a
    /// literal.
    prefilter: Option<Vec<u8>>,
    /// The scope the query was compiled for ([`Self::with_links`]) and the
    /// folder a search of it reads, or `None` to read every volume.
    scope: Option<(String, Option<String>)>,
    link_targets: Vec<LinkTarget>,
}

/// Terms of which at least one must pass.
struct CompiledClause {
    terms: Vec<CompiledTerm>,
}

struct CompiledTerm {
    negated: bool,
    check: Check,
    through: Option<Through>,
}

/// A [`Test`] with its text folded unless the query matches case, its
/// wildcards compiled and its substrings built into finders.
// A query holds a handful of checks, and each tests every candidate, so
// boxing the large `Text` would buy no memory and cost a load per test.
#[allow(clippy::large_enum_variant)]
pub(super) enum Check {
    Kind(Kind),
    /// The entry has every packed attribute, and is a folder when `folder`
    /// is set: the test of D, which the folder flag answers.
    Attributes {
        packed: PackedAttributes,
        folder: bool,
    },
    Size(SizeRange),
    DateModified(DateRange),
    DateCreated(DateRange),
    /// Folded whatever the query's case, as es compares extensions.
    Extension(Vec<Vec<u8>>),
    /// The folder's full path, without a trailing separator, which the full
    /// path of the folder that holds the entry must equal.
    Parent(Vec<u8>),
    Text(Target, Matcher),
    /// A path text test that one component of the full path decides, read
    /// as this bit of the entry's [`ComponentBits`]; its matcher is in
    /// [`CompiledQuery::components`]. See [`Matcher::component_needle`] and
    /// [`Matcher::is_first_component_start`].
    Component(u8),
}

/// The text a [`Check::Text`] reads, in the form the query compares: folded
/// unless it matches case.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Target {
    Name,
    Path,
}

/// How a [`Check::Text`] compares its text.
pub(super) enum Matcher {
    Contains(memmem::Finder<'static>),
    WholeWord(memmem::Finder<'static>),
    WordStart(memmem::Finder<'static>),
    Start(Vec<u8>),
    /// Matches the whole text.
    Wildcard(Wildcard),
    /// Compiled case-insensitive unless the query matches case, when it
    /// tests the folded text like every other matcher. The pattern itself is
    /// never folded, because folding would turn `\D` into `\d`.
    Regex(Regex),
    Ordered {
        terms: Vec<memmem::Finder<'static>>,
        term_match: TermMatch,
    },
}

impl CompiledQuery {
    /// Compiles `parsed`, or refuses it with [`QueryError::TooManyTerms`].
    pub fn new(parsed: &ParsedQuery) -> Result<Self, QueryError> {
        let terms: usize = parsed
            .clauses
            .iter()
            .map(|clause| {
                let terms: usize = clause
                    .terms
                    .iter()
                    .map(|term| match &term.test {
                        Test::Extension(exts) => exts.len().max(1),
                        Test::Ordered { terms, .. } => terms.len().max(1),
                        _ => 1,
                    })
                    .sum();
                terms.max(1)
            })
            .sum();
        if terms > MAX_QUERY_TERMS {
            return Err(QueryError::TooManyTerms(terms));
        }
        let regexes = parsed
            .clauses
            .iter()
            .flat_map(|clause| &clause.terms)
            .filter(|term| matches!(term.test, Test::NameRegex(_) | Test::PathRegex(_)))
            .count();
        if regexes > MAX_QUERY_REGEXES {
            return Err(QueryError::TooManyRegexes(regexes));
        }

        let match_case = parsed.match_case;
        let mut clauses = parsed
            .clauses
            .iter()
            .map(|clause| {
                let terms = clause
                    .terms
                    .iter()
                    .map(|term| {
                        Ok(CompiledTerm {
                            negated: term.negated,
                            check: Check::new(&term.test, match_case)?,
                            through: None,
                        })
                    })
                    .collect::<Result<_, QueryError>>()?;
                Ok(CompiledClause { terms })
            })
            .collect::<Result<Vec<_>, QueryError>>()?;

        // A path wildcard that one name decides needs no full path: `*.dll`
        // tests the name, which the prefilter below then scans for, and
        // `*report*` is a path term that the components below decide.
        for term in clauses.iter_mut().flat_map(|clause| &mut clause.terms) {
            let Check::Text(target @ Target::Path, Matcher::Wildcard(wildcard)) = &mut term.check
            else {
                continue;
            };
            match wildcard.one_name() {
                Some(OneName::Last) => *target = Target::Name,
                Some(OneName::Inside(text)) => {
                    let finder = memmem::Finder::new(&text).into_owned();
                    term.check = Check::Text(Target::Path, Matcher::Contains(finder));
                }
                None => {}
            }
        }

        // Every match contains the prefilter, so scanning for it yields a
        // superset of the matches that the clauses then verify. The first of
        // equally long literals wins.
        let mut prefilter: Option<Vec<u8>> = None;
        for literal in clauses.iter().filter_map(CompiledClause::name_literal) {
            if literal.len() > prefilter.as_ref().map_or(0, Vec::len) {
                prefilter = Some(literal);
            }
        }

        // A clause whose every term one component decides reads its bits
        // instead of the full path. A clause that mixes in another path test
        // builds the path anyway, so its terms keep their path tests.
        let mut components = Vec::new();
        for clause in &mut clauses {
            let decided = clause.terms.iter().all(|term| {
                matches!(&term.check, Check::Text(Target::Path, matcher)
                    if matcher.component_needle().is_some() || matcher.is_first_component_start())
            });
            if !decided || components.len() + clause.terms.len() > MAX_COMPONENT_TERMS {
                continue;
            }
            for term in &mut clause.terms {
                let bit = u8::try_from(components.len())
                    .expect("at most MAX_COMPONENT_TERMS component terms");
                let Check::Text(_, matcher) =
                    std::mem::replace(&mut term.check, Check::Component(bit))
                else {
                    unreachable!("every term of the clause is a path text term");
                };
                components.push(matcher);
            }
        }
        let mut component_literal: Option<(u8, &[u8])> = None;
        for clause in &clauses {
            if let [term] = clause.terms.as_slice()
                && !term.negated
                && let Check::Component(bit) = term.check
                && let Some(needle) = components[usize::from(bit)].component_needle()
                && needle.len() > component_literal.map_or(0, |(_, longest)| longest.len())
            {
                component_literal = Some((bit, needle));
            }
        }
        let component_literal = component_literal.map(|(bit, needle)| (bit, needle.to_vec()));

        // A stable sort keeps the query's order among clauses of one cost.
        clauses.sort_by_key(CompiledClause::cost);
        let reading = |reads| clauses.partition_point(|clause| clause.cost().0 < reads);
        let component_clauses = reading(Reads::Components);
        let path_clauses = reading(Reads::Path);
        let names_decide_path = !components.is_empty()
            && match &clauses[path_clauses..] {
                [ordered] => ordered.name_decides_components(
                    &clauses[component_clauses..path_clauses],
                    &components,
                ),
                _ => false,
            };
        Ok(Self {
            match_case,
            clauses,
            component_clauses,
            path_clauses,
            names_decide_path,
            components,
            component_literal,
            prefilter,
            scope: None,
            link_targets: Vec::new(),
        })
    }

    /// [`Self::new`] for a search below `scope` that goes through `links`,
    /// as gozlerd runs one: the scope and each `parent:` folder are their
    /// real folders, and literal and wildcard path tests with a separator
    /// also test the paths that reach an entry through the links. A scope that
    /// holds a link to a folder outside it becomes a path test of that kind,
    /// and the search reads every volume.
    pub fn with_links(
        parsed: &ParsedQuery,
        links: &Links,
        scope: Option<&str>,
    ) -> Result<Self, QueryError> {
        let mut parsed = parsed.clone();
        for term in parsed.clauses.iter_mut().flat_map(|c| &mut c.terms) {
            if let Test::Parent(folder) = &mut term.test {
                *folder = links.real_path(folder);
            }
        }
        let mut query = Self::new(&parsed)?;
        let reads = links.search_scope(scope);
        if reads.is_none()
            && let Some(scope) = scope
        {
            let real = links.real_path(scope);
            let below = format!("{}\\", real.trim_end_matches('\\'));
            // Internal scope enforcement does not use a user term's slot.
            query.clauses.push(CompiledClause {
                terms: vec![CompiledTerm {
                    negated: false,
                    check: Check::new(&Test::PathStart(below), query.match_case)?,
                    through: None,
                }],
            });
            query.names_decide_path = false;
        }
        query.scope = scope.map(|scope| (scope.to_owned(), reads));
        if links.is_empty() || !query.clauses.iter().flat_map(|c| &c.terms)
            .any(|term| matches!(&term.check, Check::Text(Target::Path, matcher) if matcher.reads_links())) {
            return Ok(query);
        }
        let path_clauses = query.path_clauses;
        let paths = LinkPaths::new(links, query.match_case);
        for term in query.clauses[path_clauses..]
            .iter_mut()
            .flat_map(|c| &mut c.terms)
        {
            if let Check::Text(Target::Path, matcher) = &term.check
                && matcher.reads_links()
            {
                term.through = Some(paths.compile(matcher));
            }
        }
        query.link_targets = paths.targets;
        Ok(query)
    }

    /// The folder a search below `scope` reads, or `None` for every volume:
    /// for the scope the query was compiled for, the folder
    /// [`Self::with_links`] chose, and any other scope as it is.
    pub(super) fn search_scope(&self, scope: Option<&str>) -> Option<String> {
        match (&self.scope, scope) {
            (Some((compiled, reads)), Some(scope)) if compiled == scope => reads.clone(),
            (_, scope) => scope.map(str::to_owned),
        }
    }

    /// Whether the query matches case, which its scope folder follows too.
    pub(super) fn match_case(&self) -> bool {
        self.match_case
    }

    /// The bits of the [`Check::Component`] terms that `text`, one path
    /// component in the form the query compares, matches; a start only when
    /// the component is the path's `first`.
    fn component_bits(&self, text: &[u8], first: bool, code_points: &mut CodePointBuf) -> u64 {
        let mut bits = 0;
        for (bit, matcher) in self.components.iter().enumerate() {
            let decides = first || !matches!(matcher, Matcher::Start(_));
            if decides && matcher.matches(text, code_points) {
                bits |= 1 << bit;
            }
        }
        bits
    }

    /// Whether `root`, the name of a volume's root in the form the query
    /// compares, fails a clause of one positive start that it alone decides,
    /// so that nothing on the volume matches. A root mounted nowhere has no
    /// name, and decides nothing.
    fn root_fails(&self, root: &[u8]) -> bool {
        !root.is_empty()
            && self
                .clauses
                .iter()
                .any(|clause| match clause.terms.as_slice() {
                    [
                        CompiledTerm {
                            negated: false,
                            check: Check::Component(bit),
                            ..
                        },
                    ] => matches!(&self.components[usize::from(*bit)],
                    Matcher::Start(start) if !root.starts_with(start)),
                    _ => false,
                })
    }
}

/// The most terms of one query that [`ComponentBits`] decide, one bit each;
/// the terms past it test the full path.
const MAX_COMPONENT_TERMS: usize = u64::BITS as usize;

/// What a clause reads of an entry, in the order the scan has it: the
/// entry's own columns, then its [`ComponentBits`], then its full path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Reads {
    Entry,
    Components,
    Path,
}

impl CompiledClause {
    fn name_decides_components(&self, clauses: &[CompiledClause], components: &[Matcher]) -> bool {
        let [
            CompiledTerm {
                negated: false,
                check: Check::Text(Target::Path, Matcher::Ordered { terms, term_match }),
                ..
            },
        ] = self.terms.as_slice()
        else {
            return false;
        };
        if *term_match == TermMatch::NameStart {
            return false;
        }
        clauses.iter().all(|clause| {
            let [
                CompiledTerm {
                    negated: false,
                    check: Check::Component(bit),
                    ..
                },
            ] = clause.terms.as_slice()
            else {
                return false;
            };
            let finder = match (&components[usize::from(*bit)], term_match) {
                (Matcher::Contains(finder), _) => finder,
                (Matcher::WordStart(finder), TermMatch::WordStart | TermMatch::WholeWords) => {
                    finder
                }
                (Matcher::WholeWord(finder), TermMatch::WholeWords) => finder,
                _ => return false,
            };
            terms.iter().any(|term| term.needle() == finder.needle())
        })
    }

    fn passes_in_name(&self, name: &[u8], code_points: &mut CodePointBuf) -> bool {
        self.terms.iter().any(|term| {
            !term.negated
                && matches!(&term.check,
                Check::Text(Target::Path, matcher @ Matcher::Ordered { term_match, .. })
                if *term_match != TermMatch::NameStart && matcher.matches(name, code_points))
        })
    }

    /// The literal every name that passes this clause contains, when the
    /// clause is one positive name term, whole-word, word-start or name-start
    /// name term, name wildcard or single extension.
    fn name_literal(&self) -> Option<Vec<u8>> {
        let [term] = self.terms.as_slice() else {
            return None;
        };
        if term.negated {
            return None;
        }
        match &term.check {
            Check::Text(Target::Name, matcher) => match matcher {
                Matcher::Contains(finder)
                | Matcher::WholeWord(finder)
                | Matcher::WordStart(finder) => Some(finder.needle().to_vec()),
                Matcher::Start(start) => Some(start.clone()),
                Matcher::Wildcard(wildcard) => wildcard.longest_literal(),
                Matcher::Ordered { terms, .. } => terms
                    .iter()
                    .max_by_key(|finder| finder.needle().len())
                    .map(|finder| finder.needle().to_vec()),
                Matcher::Regex(_) => None,
            },
            Check::Extension(exts) => match exts.as_slice() {
                [ext] if !ext.is_empty() => Some([b".", ext.as_slice()].concat()),
                _ => None,
            },
            _ => None,
        }
    }

    /// What the clause reads, then the cost of its dearest term: the order
    /// the scan tests clauses in.
    fn cost(&self) -> (Reads, u8) {
        let reads = self.terms.iter().map(|term| match term.check {
            Check::Text(Target::Path, _) | Check::Parent(_) => Reads::Path,
            Check::Component(_) => Reads::Components,
            _ => Reads::Entry,
        });
        let cost = self.terms.iter().map(|term| term.check.cost()).max();
        (reads.max().unwrap_or(Reads::Entry), cost.unwrap_or(0))
    }

    fn passes(
        &self,
        subject: &Subject,
        target: Option<(usize, usize)>,
        scratch: &mut Scratch,
    ) -> bool {
        self.terms.iter().any(|term| {
            let real = term.check.test(subject, scratch);
            let pass = match (&term.through, &term.check, target) {
                (Some(through), Check::Text(_, matcher), Some((target, len))) => {
                    let rest = &subject.path.expect("path clauses have a path")[len..];
                    Some(
                        real == Some(true)
                            || through.passes(target, matcher, rest, &mut scratch.code_points),
                    )
                }
                _ => real,
            };
            pass.is_some_and(|pass| pass != term.negated)
        })
    }
}

/// What the checks read of one entry.
struct Subject<'a> {
    /// The entry, whose columns a check loads only when it reads them.
    entry: EntryView<'a>,
    /// The name as the query compares it: folded unless it matches case.
    name: &'a [u8],
    /// The entry's [`ComponentBits`]; set before the component clauses run.
    components: Option<u64>,
    /// The full path in the same form as `name`; set before the path
    /// clauses run.
    path: Option<&'a [u8]>,
    /// [`VolumeIndex::folder_sizes_at_least`]: a folder's size is only a
    /// lower bound.
    folder_sizes_at_least: bool,
    /// What the request sees, which decides whether a folder has a size.
    seen_by: &'a SeenBy,
}

/// Buffers the checks reuse from one entry to the next.
#[derive(Default)]
struct Scratch {
    code_points: CodePointBuf,
    extension: Vec<u8>,
}

impl Check {
    /// Compiles `test`, its text folded unless `match_case`, or refuses a
    /// malformed or oversized regular expression.
    pub(super) fn new(test: &Test, match_case: bool) -> Result<Self, QueryError> {
        let text = |s: &str| {
            if match_case {
                s.as_bytes().to_vec()
            } else {
                fold(s.as_bytes())
            }
        };
        let finder = |s: &str| memmem::Finder::new(&text(s)).into_owned();
        // A term that starts with a character that is not a letter or digit,
        // such as `.dll`, starts its own word, so it matches wherever it is.
        let word_start = |s: &str| {
            if is_word(s.chars().next().map(u32::from)) {
                Matcher::WordStart(finder(s))
            } else {
                Matcher::Contains(finder(s))
            }
        };
        let wildcard = |s: &str| Wildcard::compile(&text(s));
        let regex = |pattern: &str| {
            RegexBuilder::new(pattern)
                .case_insensitive(!match_case)
                .size_limit(REGEX_SIZE_LIMIT)
                .dfa_size_limit(REGEX_CACHE_LIMIT)
                .build()
                .map_err(|e| QueryError::Malformed(Malformed::Regex(e.to_string())))
        };
        Ok(match test {
            Test::Name(s) => Check::Text(Target::Name, Matcher::Contains(finder(s))),
            Test::Path(s) => Check::Text(Target::Path, Matcher::Contains(finder(s))),
            Test::NameWholeWord(s) => Check::Text(Target::Name, Matcher::WholeWord(finder(s))),
            Test::PathWholeWord(s) => Check::Text(Target::Path, Matcher::WholeWord(finder(s))),
            Test::NameWordStart(s) => Check::Text(Target::Name, word_start(s)),
            Test::PathWordStart(s) => Check::Text(Target::Path, word_start(s)),
            Test::NameStart(s) => Check::Text(Target::Name, Matcher::Start(text(s))),
            Test::PathStart(s) => Check::Text(Target::Path, Matcher::Start(text(s))),
            Test::NameWildcard(s) => Check::Text(Target::Name, Matcher::Wildcard(wildcard(s))),
            Test::PathWildcard(s) => Check::Text(Target::Path, Matcher::Wildcard(wildcard(s))),
            Test::NameRegex(s) => Check::Text(Target::Name, Matcher::Regex(regex(s)?)),
            Test::PathRegex(s) => Check::Text(Target::Path, Matcher::Regex(regex(s)?)),
            Test::Ordered {
                terms,
                term_match,
                match_path,
            } => Check::Text(
                if *match_path {
                    Target::Path
                } else {
                    Target::Name
                },
                Matcher::Ordered {
                    terms: terms.iter().map(|term| finder(term)).collect(),
                    term_match: *term_match,
                },
            ),
            Test::Extension(exts) => {
                Check::Extension(exts.iter().map(|ext| fold(ext.as_bytes())).collect())
            }
            Test::Parent(s) => Check::Parent(text(s.trim_end_matches(['\\', '/']))),
            Test::Size(range) => Check::Size(*range),
            Test::DateModified(range) => Check::DateModified(*range),
            Test::DateCreated(range) => Check::DateCreated(*range),
            Test::Kind(kind) => Check::Kind(*kind),
            Test::Attributes(attributes) => {
                let (packed, folder) = attributes.pack();
                Check::Attributes { packed, folder }
            }
        })
    }

    /// Relative cost of testing one entry, cheapest first: the allocation-free
    /// checks reject most entries before the extension check folds anything.
    fn cost(&self) -> u8 {
        match self {
            Check::Kind(_) | Check::Attributes { .. } => 0,
            Check::Size(_) | Check::DateModified(_) | Check::DateCreated(_) => 1,
            Check::Parent(_) => 2,
            Check::Text(_, matcher) => match matcher {
                Matcher::Contains(_)
                | Matcher::WholeWord(_)
                | Matcher::WordStart(_)
                | Matcher::Start(_) => 2,
                Matcher::Wildcard(_) => 3,
                Matcher::Regex(_) => 5,
                Matcher::Ordered { .. } => 4,
            },
            Check::Extension(_) => 4,
            Check::Component(_) => 0,
        }
    }

    /// Whether `subject` passes, or `None` when the value the check reads is
    /// unknown, which fails the term whether or not it is negated.
    fn test(&self, subject: &Subject, scratch: &mut Scratch) -> Option<bool> {
        let path = || {
            subject
                .path
                .expect("path clauses run after the path is set")
        };
        let entry = &subject.entry;
        Some(match self {
            Check::Kind(Kind::File) => !entry.is_dir(),
            Check::Kind(Kind::Folder) => entry.is_dir(),
            Check::Attributes { packed, folder } => {
                entry.packed_attributes().contains(*packed) && (entry.is_dir() || !*folder)
            }
            // A folder's size is the sum of its files, and unknown until the
            // size and date pass ends.
            Check::Size(range) if entry.is_dir() && subject.folder_sizes_at_least => {
                return holds_from(*range, entry.size(subject.seen_by)?);
            }
            Check::Size(range) => range.contains(entry.size(subject.seen_by)?),
            Check::DateModified(range) => range.contains(entry.mtime()?),
            Check::DateCreated(range) => range.contains(entry.created()?),
            Check::Extension(exts) => match extension(entry.name(), entry.is_dir()) {
                Some(ext) => {
                    scratch.extension.clear();
                    crate::fold::fold_into(ext, &mut scratch.extension);
                    exts.contains(&scratch.extension)
                }
                None => false,
            },
            Check::Parent(folder) => {
                // The path ends with the name in the same form, after the
                // folder's path and, unless that is empty, a separator.
                let parent = path()
                    .strip_suffix(subject.name)
                    .expect("a full path ends with its name");
                parent.strip_suffix(b"\\").unwrap_or(parent) == folder.as_slice()
            }
            Check::Text(target, matcher) => {
                let text = match target {
                    Target::Name => subject.name,
                    Target::Path => path(),
                };
                matcher.matches(text, &mut scratch.code_points)
            }
            Check::Component(bit) => {
                let bits = subject
                    .components
                    .expect("component clauses run after the bits are set");
                bits & (1 << bit) != 0
            }
        })
    }
}

/// Whether `range` holds a size that is at least `floor`, as a folder's after
/// a size pass that stopped: `Some(true)` when it holds every size from
/// `floor` up, `Some(false)` when it holds none, and unknown otherwise. So a
/// folder whose unread files could take it out of the range never matches,
/// negated or not, because a wrong row is worse than a missing one.
fn holds_from(range: SizeRange, floor: u64) -> Option<bool> {
    if range.max == u64::MAX && range.min <= floor {
        Some(true)
    } else if range.max < floor {
        Some(false)
    } else {
        None
    }
}

impl Matcher {
    pub(super) fn matches(&self, text: &[u8], code_points: &mut CodePointBuf) -> bool {
        match self {
            Matcher::Contains(finder) => finder.find(text).is_some(),
            Matcher::WholeWord(finder) => contains_whole_word(finder, text),
            Matcher::WordStart(finder) => contains_at_word_start(finder, text),
            Matcher::Start(start) => text.starts_with(start),
            Matcher::Wildcard(wildcard) => wildcard.matches_into(text, code_points),
            Matcher::Regex(regex) => regex.is_match(text),
            Matcher::Ordered { terms, term_match } => {
                ordered_parts(terms, *term_match, text, |_| {})
            }
        }
    }

    /// The needle of a matcher that one component of a full path decides:
    /// a text without a separator, matched anywhere, as a whole word or at
    /// a word start. Such a text cannot span a `\`, and a `\`, like the edge
    /// of a text, is no letter or digit, so the full path matches it exactly
    /// when one of its components does. `None` for every other matcher: a
    /// start or a wildcard matches where the whole path starts or ends, and
    /// a regular expression can span a separator.
    fn component_needle(&self) -> Option<&[u8]> {
        match self {
            Matcher::Contains(finder) | Matcher::WholeWord(finder) | Matcher::WordStart(finder) => {
                Some(finder.needle()).filter(|needle| !needle.contains(&b'\\'))
            }
            Matcher::Start(_)
            | Matcher::Wildcard(_)
            | Matcher::Regex(_)
            | Matcher::Ordered { .. } => None,
        }
    }

    /// Whether this is a start without a separator, which the first
    /// component of a full path decides: the path starts with the text
    /// exactly when that component does, because the `\` after it is in no
    /// such text. So with Match path, each word of Start of name, a
    /// `startwith:` of the full path, needs no path.
    fn is_first_component_start(&self) -> bool {
        matches!(self, Matcher::Start(start) if !start.contains(&b'\\'))
    }

    /// Whether this path test also reads the paths that reach an entry
    /// through folder links: literal and wildcard terms with a separator.
    /// A word names no folder to open, and through Windows'
    /// compatibility junctions, such as `Local Settings`, a word that names
    /// a link would find every entry below its target.
    fn reads_links(&self) -> bool {
        match self {
            Matcher::Regex(_) => false,
            Matcher::Wildcard(wildcard) => wildcard.has_separator(),
            _ => self.literal().is_some_and(|text| text.contains(&b'\\')),
        }
    }

    fn last_component_literal(&self) -> Option<Vec<u8>> {
        match self {
            Matcher::Wildcard(wildcard) => wildcard.last_component_literal(0),
            _ => self
                .literal()?
                .rsplit(|&b| b == b'\\')
                .find(|part| !part.is_empty())
                .map(<[u8]>::to_vec),
        }
    }

    /// The text of a matcher that compares a text, which
    /// [`super::through`] splits; `None` for a wildcard and a regular
    /// expression.
    pub(super) fn literal(&self) -> Option<&[u8]> {
        match self {
            Matcher::Contains(finder) | Matcher::WholeWord(finder) | Matcher::WordStart(finder) => {
                Some(finder.needle())
            }
            Matcher::Start(start) => Some(start),
            Matcher::Wildcard(_) | Matcher::Regex(_) | Matcher::Ordered { .. } => None,
        }
    }
    /// Appends to `parts` each part of `text` that this matcher matches, as
    /// a byte range, with the occurrence rules of [`Check::test`], so the
    /// part a window shows bold is always one that made the entry match.
    /// A wildcard gives the parts its literal characters matched, and a
    /// regular expression each of its matches.
    pub(super) fn parts(
        &self,
        text: &[u8],
        code_points: &mut CodePointBuf,
        parts: &mut Vec<Range<usize>>,
    ) {
        let found = |finder: &memmem::Finder, accept: fn(&Occurrence) -> bool| {
            let len = finder.needle().len();
            occurrences(finder, text)
                .filter(accept)
                .map(move |o| o.start..o.start + len)
                .collect::<Vec<_>>()
        };
        match self {
            Matcher::Contains(finder) => parts.extend(found(finder, |_| true)),
            Matcher::WholeWord(finder) => parts.extend(found(finder, Occurrence::is_whole_word)),
            Matcher::WordStart(finder) => parts.extend(found(finder, Occurrence::is_word_start)),
            Matcher::Start(start) => {
                if text.starts_with(start) {
                    parts.push(0..start.len());
                }
            }
            Matcher::Wildcard(wildcard) => wildcard.literal_parts(text, code_points, parts),
            Matcher::Regex(regex) => parts.extend(regex.find_iter(text).map(|m| m.range())),
            Matcher::Ordered { terms, term_match } => {
                let before = parts.len();
                if !ordered_parts(terms, *term_match, text, |range| parts.push(range)) {
                    parts.truncate(before);
                }
            }
        }
    }
}

fn ordered_parts(
    terms: &[memmem::Finder<'static>],
    term_match: TermMatch,
    text: &[u8],
    mut matched: impl FnMut(Range<usize>),
) -> bool {
    let mut from = 0;
    for finder in terms {
        let occurrence = occurrences(finder, text).find(|occurrence| {
            occurrence.start >= from
                && match term_match {
                    TermMatch::Anywhere => true,
                    TermMatch::WholeWords => occurrence.is_whole_word(),
                    TermMatch::WordStart => {
                        !is_word(crate::wtf8::CodePoints::new(finder.needle()).next())
                            || occurrence.is_word_start()
                    }
                    TermMatch::NameStart => occurrence.start == 0,
                }
        });
        let Some(occurrence) = occurrence else {
            return false;
        };
        let end = occurrence.start + finder.needle().len();
        matched(occurrence.start..end);
        from = end;
    }
    true
}

/// Whether `text` contains the finder's needle with no letter or digit right
/// before or after it. As in Everything, every other character, `_`
/// included, ends a word; letters and digits outside ASCII are word
/// characters too.
fn contains_whole_word(finder: &memmem::Finder, text: &[u8]) -> bool {
    occurrences(finder, text).any(|o| o.is_whole_word())
}

/// Whether `text` contains the finder's needle where a word starts: at the
/// start of `text`, or after a character that is not a letter or digit, as
/// [`contains_whole_word`] reads a word.
fn contains_at_word_start(finder: &memmem::Finder, text: &[u8]) -> bool {
    occurrences(finder, text).any(|o| o.is_word_start())
}

/// Whether a code point is a letter or a digit; `None`, the edge of the
/// text, is not.
pub(super) fn is_word(cp: Option<u32>) -> bool {
    cp.and_then(char::from_u32)
        .is_some_and(char::is_alphanumeric)
}

/// Where a needle occurs in a text, and the code points right before and
/// after it; `None` at an edge of the text.
struct Occurrence {
    start: usize,
    before: Option<u32>,
    after: Option<u32>,
}

impl Occurrence {
    fn is_whole_word(&self) -> bool {
        !is_word(self.before) && !is_word(self.after)
    }

    fn is_word_start(&self) -> bool {
        !is_word(self.before)
    }
}

/// The last code point of `bytes`; `None` when they are empty.
fn last_code_point(bytes: &[u8]) -> Option<u32> {
    // A code point is at most 4 bytes, and only its first is not 0b10xxxxxx.
    let tail = &bytes[bytes.len().saturating_sub(4)..];
    let lead = tail.iter().rposition(|b| b & 0xC0 != 0x80)?;
    crate::wtf8::CodePoints::new(&tail[lead..]).next()
}

/// Each occurrence of the finder's needle in `text`, overlapping ones
/// included.
fn occurrences<'a>(
    finder: &'a memmem::Finder,
    text: &'a [u8],
) -> impl Iterator<Item = Occurrence> + 'a {
    use crate::wtf8::CodePoints;
    let mut from = 0;
    std::iter::from_fn(move || {
        let start = from + finder.find(text.get(from..)?)?;
        let end = start + finder.needle().len();
        // The next occurrence may overlap this one: `a-a` in `xa-a-a`.
        from = start + 1;
        Some(Occurrence {
            start,
            before: last_code_point(&text[..start]),
            after: CodePoints::new(&text[end..]).next(),
        })
    })
}

/// The parts of an entry's full path that a query's text terms match, which
/// the window shows bold, so a person sees why a row matched.
///
/// It compiles each positive text term as [`CompiledQuery`] does and finds
/// its parts with the matcher that decides whether the entry matches, so the
/// bold parts and the matches cannot disagree. A negated term matched by
/// being absent, and a test of the size, the date, the kind, the extension,
/// the attributes or the parent folder matches no text, so neither gives a
/// part. Build it once per search: it compiles every term.
pub struct MatchedParts {
    match_case: bool,
    terms: Vec<(Target, Matcher, Option<Through>)>,
    link_targets: FxHashMap<Vec<u8>, usize>,
}

impl MatchedParts {
    /// The text terms of `query`, compiled.
    ///
    /// # Errors
    ///
    /// A regular expression that [`CompiledQuery::new`] refuses, as
    /// [`QueryError::Malformed`].
    pub fn new(query: &ParsedQuery) -> Result<Self, QueryError> {
        let mut terms = Vec::new();
        for term in query.clauses.iter().flat_map(|c| &c.terms) {
            if term.negated {
                continue;
            }
            if let Check::Text(target, matcher) = Check::new(&term.test, query.match_case)? {
                terms.push((target, matcher, None));
            }
        }
        Ok(Self {
            match_case: query.match_case,
            terms,
            link_targets: FxHashMap::default(),
        })
    }

    /// [`Self::new`] for a search that went through `links`, as gozlerd
    /// ran it ([`CompiledQuery::with_links`]): a path term also gives the
    /// parts it matched through a link.
    ///
    /// # Errors
    ///
    /// As [`Self::new`].
    pub fn with_links(query: &ParsedQuery, links: &Links) -> Result<Self, QueryError> {
        let mut parts = Self::new(query)?;
        if !links.is_empty()
            && parts
                .terms
                .iter()
                .any(|(target, matcher, _)| *target == Target::Path && matcher.reads_links())
        {
            let paths = LinkPaths::new(links, query.match_case);
            for (target, matcher, through) in &mut parts.terms {
                if *target == Target::Path && matcher.reads_links() {
                    *through = Some(paths.compile(matcher));
                }
            }
            parts.link_targets = paths
                .targets
                .iter()
                .enumerate()
                .map(|(number, target)| (fold(target.raw.as_bytes()), number))
                .collect();
        }
        Ok(parts)
    }

    /// The byte ranges of `full_path` that the terms match, in order and
    /// without overlaps. A name term's parts lie in the name, after the last
    /// `\`; a path term's anywhere in the full path, and through a link in
    /// the part of the path below the link's target.
    pub fn ranges(&self, full_path: &str) -> Vec<Range<usize>> {
        let name_at = full_path.rfind('\\').map_or(0, |i| i + 1);
        let mut ranges = Vec::new();
        let mut code_points = CodePointBuf::new();
        let mut found = Vec::new();
        for (target, raw, offset) in [
            (Target::Name, &full_path[name_at..], name_at),
            (Target::Path, full_path, 0),
        ] {
            let matchers = self.terms.iter().filter(|(t, _, _)| *t == target);
            if matchers.clone().next().is_none() {
                continue;
            }
            let compared = Compared::new(raw, self.match_case);
            found.clear();
            for (_, matcher, _) in matchers {
                matcher.parts(&compared.text, &mut code_points, &mut found);
            }
            ranges.extend(found.iter().filter(|r| !r.is_empty()).map(|r| {
                let r = compared.raw_range(r.clone());
                r.start + offset..r.end + offset
            }));
        }
        if !self.link_targets.is_empty()
            && let Some((number, at)) =
                super::through::nearest(&self.link_targets, full_path.as_bytes(), true)
        {
            let rest = &full_path[at..];
            let compared = Compared::new(rest, self.match_case);
            for (_, matcher, through) in &self.terms {
                let Some(through) = through else {
                    continue;
                };
                found.clear();
                through.parts(
                    number,
                    matcher,
                    &compared.text,
                    &mut code_points,
                    &mut found,
                );
                ranges.extend(found.iter().filter(|r| !r.is_empty()).map(|r| {
                    let raw = compared.raw_range(r.clone());
                    raw.start + at..raw.end + at
                }));
            }
        }
        ranges.sort_by_key(|r| r.start);
        let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    }
}

/// A text in the form a query compares it, folded unless the query matches
/// case, with the way back from a range of that form to the text as stored.
struct Compared<'a> {
    raw: &'a str,
    text: std::borrow::Cow<'a, [u8]>,
    /// Each character boundary of the raw text, its end included; `None`
    /// when every byte keeps its place: a query that matches case, or ASCII
    /// text, which folds byte for byte.
    boundaries: Option<Vec<Boundary>>,
}

/// A character boundary of the raw text, placed by the folded length of the
/// whole text before it, because folding changes lengths and composes a
/// letter with the marks after it. That length can shrink as the text
/// grows, when a mark reorders and composes, so a boundary also holds the
/// running maximum up to it and the minimum from it on, which never shrink
/// and let a binary search find what a scan of every boundary finds.
struct Boundary {
    raw: usize,
    most_up_to_here: usize,
    least_from_here: usize,
}

impl<'a> Compared<'a> {
    fn new(raw: &'a str, match_case: bool) -> Self {
        if match_case {
            return Self {
                raw,
                text: raw.as_bytes().into(),
                boundaries: None,
            };
        }
        let boundaries = (!raw.is_ascii()).then(|| {
            let lengths = crate::fold::folded_lengths(raw);
            let mut least = usize::MAX;
            let mut least_from: Vec<usize> = (lengths.iter().rev())
                .map(|&(_, length)| {
                    least = least.min(length);
                    least
                })
                .collect();
            least_from.reverse();
            let mut most = 0;
            lengths
                .iter()
                .zip(least_from)
                .map(|(&(raw, length), least_from_here)| {
                    most = most.max(length);
                    Boundary {
                        raw,
                        most_up_to_here: most,
                        least_from_here,
                    }
                })
                .collect()
        });
        Self {
            raw,
            text: fold(raw.as_bytes()).into(),
            boundaries,
        }
    }

    /// The range of the raw text that `range` of the compared text came
    /// from, widened to whole characters: from the last boundary whose
    /// folded length is at most `range.start` to the first whose folded
    /// length is at least `range.end`.
    fn raw_range(&self, range: Range<usize>) -> Range<usize> {
        let Some(boundaries) = &self.boundaries else {
            return range;
        };
        // Past the last boundary at most `range.start`, every one is above it.
        let start = boundaries.partition_point(|b| b.least_from_here <= range.start);
        // Before the first boundary at least `range.end`, every one is below.
        let end = boundaries.partition_point(|b| b.most_up_to_here < range.end);
        let start = start.checked_sub(1).map_or(0, |i| boundaries[i].raw);
        let end = boundaries.get(end).map_or(self.raw.len(), |b| b.raw);
        start..end.max(start)
    }
}

/// Finds the matches of a query over `index`, the volume mounted at
/// `mount_prefix` (`C:\`), with the keys they sort by. The caller holds the
/// index's read lock for this call, releases it for [`Matches::order`], and
/// holds it again for [`OrderedMatches::read_rows`], so the folder ranks of a
/// path sort and the sort of millions of matches never hold the journal back.
///
/// `scope` (if any) restricts results to descendants of that directory entry;
/// [`super::ResultRows::search`] resolves the `-path` folder via
/// [`resolve_scope`] and runs these steps. The matches are ordered per
/// `sort`, then the first `limit` become the rows; the rows' total counts
/// every match. There is no offset: the search applies a request's offset
/// after it merges the volumes, so an offset here would page each volume on
/// its own. With `sums`, the rows also carry every match's [`MatchSums`],
/// from the same scan. Only the entries `seen_by` sees match, and their keys
/// hold what it may read.
///
/// The common case, a case-insensitive query with a name literal, scans the
/// index's contiguous folded-name arena with a single prebuilt `memmem`
/// finder (SIMD over the whole buffer) and maps each hit back to its entry,
/// which keeps the hot path off per-query folding and off per-match path
/// building. The matches sort by numbers, from the index's name ranks, so the
/// sort compares no names but within the runs of matches whose keys are
/// equal.
#[allow(clippy::too_many_arguments)]
pub(crate) fn find_matches(
    index: &VolumeIndex,
    mount_prefix: &str,
    query: &CompiledQuery,
    scope: Option<EntryIdx>,
    sort: SortSpec,
    limit: Option<u32>,
    sums: bool,
    seen_by: &SeenBy,
) -> Matches {
    let Scan {
        records,
        folders,
        folder_names,
        scope,
        ..
    } = scan(index, mount_prefix, query, scope, seen_by);
    let origin = Origin {
        instance: index.instance(),
        reuse_mark: index.reuse_mark(),
        scope: scope.map(|s| (s, index.entry(s).frn())),
        sums: sums.then(|| match_sums(index, &records, seen_by)),
        mount_prefix: mount_prefix.to_string(),
    };
    let folders = (folders, folder_names);
    key_matches(index, records, folders, sort, limit, origin, seen_by)
}

/// Keys `records`, matches an earlier search found on `index` below the
/// scope folder `scope`, under `sort`, as [`find_matches`] keys its own but
/// with no scan: how a result snapshot is sorted again. `sums` are the
/// search's, and the keys hold what `seen_by` may read.
pub(crate) fn key_found(
    index: &VolumeIndex,
    mount_prefix: &str,
    records: Vec<SortRecord>,
    scope: Option<(EntryIdx, Frn)>,
    sums: Option<MatchSums>,
    sort: SortSpec,
    seen_by: &SeenBy,
) -> Matches {
    let mut folder_names = ResultNames::default();
    let folders = FolderCopy::new(index, mount_prefix, &mut folder_names);
    let origin = Origin {
        instance: index.instance(),
        reuse_mark: index.reuse_mark(),
        scope,
        sums,
        mount_prefix: mount_prefix.to_string(),
    };
    let folders = (folders, folder_names);
    key_matches(index, records, folders, sort, None, origin, seen_by)
}

/// Keys `records` under `sort` for [`Matches::order`], whose first `limit`
/// become the rows; a path sort copies the records' folders into `folders`.
fn key_matches(
    index: &VolumeIndex,
    mut records: Vec<SortRecord>,
    folders: (FolderCopy, ResultNames),
    sort: SortSpec,
    limit: Option<u32>,
    origin: Origin,
    seen_by: &SeenBy,
) -> Matches {
    let end = match limit {
        Some(n) => (n as usize).min(records.len()),
        None => records.len(),
    };
    let path_folders = (end > 0)
        .then(|| key_records(index, &mut records, folders, sort, seen_by))
        .flatten();
    Matches {
        records,
        path_folders,
        sort,
        end,
        origin,
    }
}

/// The keyed matches of one search on one volume, after the first hold of
/// the volume's index read lock (see [`find_matches`]).
pub(crate) struct Matches {
    records: Vec<SortRecord>,
    /// For a path sort: the folders of the records, while each record's
    /// primary key holds its folder's place in this table.
    path_folders: Option<ResultNames>,
    sort: SortSpec,
    /// How many records the rows take.
    end: usize,
    origin: Origin,
}

/// The matches of [`Matches::order`], for the second hold of the volume's
/// index read lock.
pub(crate) struct OrderedMatches {
    /// Sorted by their keys up to `end`. From `end` to `ties_end` lie the
    /// other records with the keys of the last row, unsorted, which
    /// [`Self::read_rows`] orders with the rows; the rest are in no order.
    records: Vec<SortRecord>,
    sort: SortSpec,
    end: usize,
    ties_end: usize,
    origin: Origin,
}

/// Where a search found its matches, read at the first hold of the lock:
/// what the second hold checks the index against, and what the rows keep.
struct Origin {
    /// [`VolumeIndex::instance`] of the index the records name.
    instance: u64,
    /// [`VolumeIndex::reuse_mark`] at the scan: a record whose slot a new
    /// entry took over after it names an entry that died.
    reuse_mark: u64,
    /// The scope folder and its file ID; `None` without a scope.
    scope: Option<(EntryIdx, Frn)>,
    sums: Option<MatchSums>,
    mount_prefix: String,
}

/// The index changed between [`find_matches`] and
/// [`OrderedMatches::read_rows`]: a rebuild replaced it, its entries got new
/// indexes, or more of its slots got new entries than it keeps a note of, so
/// the records no longer name its entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the volume's index changed during the search")]
pub(crate) struct IndexChanged;

impl Matches {
    /// Puts the records of the rows in the order of their keys, without the
    /// index, after a path sort's folder ranks complete their keys. When the
    /// rows are a part of the matches, quickselect finds them in O(M) and only
    /// they are sorted; the records with the keys of the last row can then lie
    /// on both sides of the cut, so they are all gathered at the cut, to be
    /// ordered with the rows.
    pub(crate) fn order(self) -> OrderedMatches {
        use rayon::slice::ParallelSliceMut;
        // A broad query with no limit sorts millions of records, which is
        // done in parallel across cores; a small sort is not worth the rayon
        // fan-out. A limit of all but a handful of matches selects nearly
        // everything, so the selected part is not always small.
        const PARALLEL_SORT_MIN: usize = 100_000;
        let sort_by_keys = |records: &mut [SortRecord]| {
            if records.len() >= PARALLEL_SORT_MIN {
                records.par_sort_unstable();
            } else {
                records.sort_unstable();
            }
        };
        let Self {
            mut records,
            path_folders,
            sort,
            end,
            origin,
        } = self;
        if let Some(folders) = path_folders {
            let ranks = folder_ranks(&folders);
            for record in &mut records {
                let value = KeyValue::Path {
                    folder_rank: ranks[record.primary() as usize],
                    name_key: record.name_key(),
                };
                let primary = primary_key(sort.dir, value);
                *record = SortRecord::keyed(primary, record.name_key(), record.idx());
            }
        }
        let mut ties_end = end;
        if end > 0 && end < records.len() {
            records.select_nth_unstable(end - 1);
            let last = records[end - 1].keys();
            for at in end..records.len() {
                if records[at].keys() == last {
                    records.swap(at, ties_end);
                    ties_end += 1;
                }
            }
            sort_by_keys(&mut records[..end]);
        } else if end > 0 {
            sort_by_keys(&mut records);
        }
        OrderedMatches {
            records,
            sort,
            end,
            ties_end,
            origin,
        }
    }
}

impl OrderedMatches {
    /// Orders the runs of equal keys and makes the rows, reading each one's
    /// file ID from `index`, the index of [`find_matches`], held under its
    /// read lock again, with the `seen_by` the matches were keyed with. A
    /// match whose entry was removed since, and one whose slot another entry
    /// took over since, is left out and counted out of the total, so the
    /// rows can be fewer than the limit.
    ///
    /// # Errors
    ///
    /// [`IndexChanged`] when `index` is not the index the matches were found
    /// in, or its entries got new indexes since, or more of its slots got new
    /// entries since than it keeps a note of.
    pub(crate) fn read_rows(
        mut self,
        index: &VolumeIndex,
        seen_by: &SeenBy,
    ) -> Result<VolumeRows, IndexChanged> {
        let origin = self.origin;
        if index.instance() != origin.instance {
            return Err(IndexChanged);
        }
        let reused: FxHashSet<EntryIdx> = index
            .reused_since(origin.reuse_mark)
            .ok_or(IndexChanged)?
            .collect();
        // A removed entry's slot names no name, which the tie runs read, and
        // a reused one names another file.
        let kept = |r: &SortRecord| index.is_live(r.idx()) && !reused.contains(&r.idx());
        let matches = self.records.len();
        let removed = self.records.iter().filter(|r| !kept(r)).count();
        self.records.truncate(self.ties_end);
        self.records.retain(kept);
        order_ties(index, seen_by, self.sort, &mut self.records);
        self.records.truncate(self.end);
        // Otherwise a limited search's few rows would keep the room of every
        // match. Shrinking copies the rows, so it runs only where it frees
        // more room than it copies; an export's long list keeps its room
        // rather than be copied under the lock.
        if self.records.len() < self.records.capacity() / 2 {
            self.records.shrink_to_fit();
        }
        let past_cut = (matches - removed - self.records.len()) as u64;
        Ok(VolumeRows::new(
            self.records,
            index,
            origin.scope,
            past_cut,
            self.sort,
            origin.sums,
            origin.mount_prefix,
        ))
    }
}

/// The [`Totals`] of every match of `query` over `index` that `seen_by`
/// sees, with `scope` as in [`find_matches`]. No path is built and nothing
/// is ordered.
pub(super) fn run_totals(
    index: &VolumeIndex,
    mount_prefix: &str,
    query: &CompiledQuery,
    scope: Option<EntryIdx>,
    seen_by: &SeenBy,
) -> Result<Totals, SizeOverflow> {
    let records = scan(index, mount_prefix, query, scope, seen_by).records;
    let mut totals = Totals {
        count: records.len() as u64,
        size: 0,
    };
    for size in records
        .iter()
        .map(|r| index.entry(r.idx()))
        .filter(|e| !e.is_dir())
        .filter_map(|e| e.size(seen_by))
    {
        totals.add(Totals { count: 0, size })?;
    }
    Ok(totals)
}

/// A folder-scoped query walks the scope's subtree instead of scanning the
/// whole volume, up to this many visited entries. Past it (a near-volume
/// scope), the walk aborts and the ordinary scan+ancestry-filter path runs:
/// pointer-chasing millions of entries is slower than the SIMD arena scan.
/// The cutoff makes a Downloads-sized scope independent of disk size: the
/// scan path makes every candidate on the volume pay an ancestry check,
/// while the walk touches only the folder.
const SUBTREE_WALK_MAX: usize = 200_000;

/// Finds every match of `query` over `index` that `seen_by` sees, as
/// unordered records whose keys are not set yet.
fn scan<'q, 'i>(
    index: &'i VolumeIndex,
    mount_prefix: &str,
    query: &'q CompiledQuery,
    scope: Option<EntryIdx>,
    seen_by: &'i SeenBy,
) -> Scan<'q, 'i> {
    let mut scan = Scan::new(index, mount_prefix, query, seen_by);
    // A start that the root's name fails, as each word of Start of name
    // with Match path does on every volume, matches nothing here.
    let root = root_name(mount_prefix);
    let root = if query.match_case {
        root.to_vec()
    } else {
        fold(root)
    };
    if query.root_fails(&root) {
        return scan;
    }
    // The volume root (`-path C:\`) holds the whole volume, so it is no scope;
    // as one it would walk the capped subtree and then pay an ancestry walk
    // per match for nothing.
    let scope = scope.filter(|&root| root != index.root());
    scan.scope = scope;

    // Folder scope: walk the scope subtree via the child chains, so cost
    // tracks the folder, not the volume. Membership is by construction, so
    // the candidates are tested without the scope (its ancestry walk is the
    // expensive part this path exists to avoid); every other predicate is
    // identical to the scan paths below. Oversized scopes abort to those paths.
    if let Some(root) = scope {
        let mut walk = index.subtree_entries(root);
        for idx in walk.by_ref().take(SUBTREE_WALK_MAX) {
            if !index.is_synthetic(idx) {
                scan.visit(idx, None);
            }
        }
        if walk.next().is_none() {
            return scan;
        }
        scan.records.clear();
    }

    // Fast path: case-insensitive query with a usable literal → scan the
    // folded arena for the prefilter literal and verify the rest per hit.
    if !query.match_case
        && let Some(needle) = query.prefilter.as_deref()
    {
        for_each_name_containing(index, needle, |id| {
            for idx in index.name_chain(id) {
                if !index.is_synthetic(idx) {
                    scan.visit(idx, scope);
                }
            }
            true
        });
        return scan;
    }

    // Component walk: a case-insensitive path term that one component
    // decides, such as each term of a -p search, matches only in a name that
    // contains its needle or below a folder whose name does, unless the
    // root's name holds the needle, which every full path starts with.
    if !query.match_case
        && let Some((bit, needle)) = &query.component_literal
        && memmem::find(&fold(root_name(mount_prefix)), needle).is_none()
    {
        if scan.visit_components(*bit, needle, scope) {
            return scan;
        }
        scan.records.clear();
    }

    if !query.match_case
        && let Some((term, needle)) = query
            .clauses
            .iter()
            .filter_map(|clause| {
                let [term] = clause.terms.as_slice() else {
                    return None;
                };
                let Check::Text(Target::Path, matcher) = &term.check else {
                    return None;
                };
                (!term.negated)
                    .then(|| matcher.last_component_literal())
                    .flatten()
                    .map(|needle| (term, needle))
            })
            .max_by_key(|(_, needle)| needle.len())
        && memmem::find(&root, &needle).is_none()
    {
        if scan.visit_path_candidates(term, &needle, scope) {
            return scan;
        }
        scan.records.clear();
    }

    // Slow path: case-sensitive, or no name literal (only filters, path
    // terms or all-wildcard patterns). Iterate every live entry. The empty
    // search with no scope, as a full export runs, matches every entry, so
    // its records are reserved once: a list that grows holds its old and new
    // buffers together. Other scans grow the list, so a selective one does
    // not commit a record for every entry.
    if scope.is_none() && query.clauses.is_empty() {
        scan.records.reserve_exact(index.len());
    }
    for idx in index.live_entries() {
        if index.is_synthetic(idx) {
            continue;
        }
        scan.visit(idx, scope);
    }
    scan
}

/// Calls `found` with each name of the folded arena that contains `needle`,
/// once per name, in arena order, until it returns `false`.
///
/// The arena holds each distinct name once (names are interned), so the
/// scan's cost follows the unique-name bytes, and a name fans out to every
/// entry bearing it through its same-name chain. The chains are exact, so no
/// per-candidate staleness guard is needed.
fn for_each_name_containing(
    index: &VolumeIndex,
    needle: &[u8],
    mut found: impl FnMut(NameId) -> bool,
) {
    let pairs = index.name_pairs();
    if pairs.is_empty() {
        return;
    }
    // Merge-walk: both the `memmem` hits and `pairs[id].folded.off` are
    // ascending, so a forward cursor maps each hit to its name id in
    // amortized O(1) with no per-hit binary search (which cache-thrashes at
    // scale). The first hit seeds the cursor with one binary search so a
    // rare term whose matches start deep in the arena doesn't pay a
    // full-length forward scan.
    let mut cursor = 0usize;
    let mut seeded = false;
    // A name is a contiguous NUL-separated region, so repeated hits in one
    // name are adjacent: a single "same as last" check dedups them.
    let mut last_id = NameId::MAX;
    for pos in memmem::Finder::new(needle).find_iter(index.folded_bytes()) {
        let pos = pos as u32;
        if !seeded {
            cursor = pairs
                .partition_point(|p| p.folded.off <= pos)
                .saturating_sub(1);
            seeded = true;
        } else {
            while cursor + 1 < pairs.len() && pairs[cursor + 1].folded.off <= pos {
                cursor += 1;
            }
        }
        let id = cursor as NameId;
        if id == last_id {
            continue; // the needle appears twice in one name
        }
        last_id = id;
        if !found(id) {
            return;
        }
    }
}

/// The component walk of [`scan`] gives up past this share of a volume's
/// entries, as one in this many, and the scan visits every entry instead.
/// So a walk that gives up adds at most a quarter of a full pass, while one
/// that finishes visits at most a quarter of the entries.
const COMPONENT_WALK_SHARE: usize = 4;

/// The state one scan shares across its candidates: memos and scratch
/// buffers, built once per query instead of once per candidate, and the
/// matches found so far.
struct Scan<'q, 'i> {
    index: &'i VolumeIndex,
    query: &'q CompiledQuery,
    /// What the request sees: an entry it does not see never matches.
    seen_by: &'i SeenBy,
    /// The bits of the query's [`Check::Component`] terms, in the form the
    /// query compares; `None` when it has none.
    component_bits: Option<ComponentBits>,
    /// Full paths in the form the query compares, folded unless it matches
    /// case; `None` when no clause tests the path.
    query_paths: Option<DirPaths>,
    path: Vec<u8>,
    /// The matches' folders. ONE memo, shared by the presentability check and
    /// the path sort: both need exactly these folders.
    folders: FolderCopy,
    /// The table `folders` copies into.
    folder_names: ResultNames,
    scope_memo: FxHashMap<EntryIdx, bool>,
    scratch: Scratch,
    records: Vec<SortRecord>,
    /// The folder the matches are in, set by [`scan`]; `None` for the whole
    /// volume.
    scope: Option<EntryIdx>,
}

impl<'q, 'i> Scan<'q, 'i> {
    fn new(
        index: &'i VolumeIndex,
        mount_prefix: &str,
        query: &'q CompiledQuery,
        seen_by: &'i SeenBy,
    ) -> Self {
        let mut folder_names = ResultNames::default();
        let form = if query.match_case {
            NameForm::Raw
        } else {
            NameForm::Folded
        };
        let mut scratch = Scratch::default();
        let mut query_paths = (query.path_clauses < query.clauses.len())
            .then(|| DirPaths::new(index, form, mount_prefix));
        if let Some(paths) = &mut query_paths {
            let root = mount_prefix.trim_end_matches('\\');
            let targets = query
                .link_targets
                .iter()
                .enumerate()
                .filter_map(|(number, target)| {
                    let below = crate::links::below(&target.raw, root)?;
                    resolve_scope(index, below.as_bytes(), false).map(|folder| (folder, number))
                })
                .collect();
            paths.set_targets(targets);
        }
        Self {
            index,
            query,
            seen_by,
            component_bits: (!query.components.is_empty()).then(|| {
                ComponentBits::new(index, form, mount_prefix, |text, first| {
                    query.component_bits(text, first, &mut scratch.code_points)
                })
            }),
            query_paths,
            path: Vec::new(),
            folders: FolderCopy::new(index, mount_prefix, &mut folder_names),
            folder_names,
            scope_memo: FxHashMap::default(),
            scratch,
            records: Vec::new(),
            scope: None,
        }
    }

    /// Keeps entry `idx` as a candidate if it passes every clause.
    fn visit(&mut self, idx: EntryIdx, scope: Option<EntryIdx>) {
        if let Some(record) = self.evaluate(idx, scope) {
            self.records.push(record);
        }
    }

    /// Visits, each once, the candidates of the [`Check::Component`] at
    /// `bit`: the entries whose name contains its `needle`, and every entry
    /// below one whose name passes it. Returns `false` when they pass a
    /// [`COMPONENT_WALK_SHARE`] of the volume, with some visited.
    fn visit_components(&mut self, bit: u8, needle: &[u8], scope: Option<EntryIdx>) -> bool {
        let index = self.index;
        let query = self.query;
        let most = index.len() / COMPONENT_WALK_SHARE;
        let mut named = Vec::new();
        for_each_name_containing(index, needle, |id| {
            named.extend(index.name_chain(id).filter(|&idx| !index.is_synthetic(idx)));
            named.len() <= most
        });
        if named.len() > most {
            return false;
        }
        let term = &query.components[usize::from(bit)];
        let mut visited = named.len();
        for &idx in &named {
            let bits = self
                .component_bits
                .as_mut()
                .expect("a component literal is a component term's");
            let code_points = &mut self.scratch.code_points;
            let above = bits.parent_bits(index, idx, |text, first| {
                query.component_bits(text, first, code_points)
            });
            // Without a path nothing matches, and the folder above that
            // passes the term is walked, which visits this entry. So each
            // entry is visited once, and no loop in the parent links, which
            // has no path, is walked.
            if above.is_none_or(|above| above & (1 << bit) != 0) {
                continue;
            }
            self.visit(idx, scope);
            // An entry that the parent links make a parent is walked
            // through as a folder is, so this does not ask which it is.
            if term.matches(index.folded_name(idx), &mut self.scratch.code_points) {
                for below in index.subtree_entries(idx) {
                    visited += 1;
                    if visited > most {
                        return false;
                    }
                    if !index.is_synthetic(below) {
                        self.visit(below, scope);
                    }
                }
            }
        }
        true
    }

    fn visit_path_candidates(
        &mut self,
        term: &CompiledTerm,
        needle: &[u8],
        scope: Option<EntryIdx>,
    ) -> bool {
        let index = self.index;
        let most = index.len() / COMPONENT_WALK_SHARE;
        let mut roots = Vec::new();
        for_each_name_containing(index, needle, |id| {
            roots.extend(index.name_chain(id).filter(|&idx| !index.is_synthetic(idx)));
            roots.len() <= most
        });
        if roots.len() > most {
            return false;
        }
        if let (Some(through), Check::Text(_, matcher), Some(paths)) =
            (&term.through, &term.check, &self.query_paths)
        {
            roots.extend(paths.targets().filter_map(|(folder, target)| {
                through
                    .needs_target(target, matcher, needle)
                    .then_some(folder)
            }));
        }
        // Nested name matches and link targets can reach the same entry.
        // Deduplicate before evaluating it, and bound the speculative walk
        // as the component walk does before falling back to the full scan.
        let mut visited = FxHashSet::default();
        let mut walked = 0;
        for root in roots {
            if visited.contains(&root) {
                continue;
            }
            if root != index.root()
                && self
                    .query_paths
                    .as_mut()
                    .expect("a path candidate reads paths")
                    .full_path_into(index, root, &mut self.path)
                    .is_none()
            {
                continue;
            }
            for idx in std::iter::once(root).chain(index.subtree_entries(root)) {
                walked += 1;
                if walked > most {
                    return false;
                }
                if visited.insert(idx) && !index.is_synthetic(idx) {
                    self.visit(idx, scope);
                }
            }
        }
        true
    }

    fn evaluate(&mut self, idx: EntryIdx, scope: Option<EntryIdx>) -> Option<SortRecord> {
        let index = self.index;
        let query = self.query;
        let (entry_clauses, later) = query.clauses.split_at(query.component_clauses);
        let (component_clauses, path_clauses) =
            later.split_at(query.path_clauses - query.component_clauses);
        let entry = index.entry(idx);
        let mut subject = Subject {
            // The precomputed folded name serves a case-insensitive query.
            name: if query.match_case {
                entry.name()
            } else {
                index.folded_name(idx)
            },
            entry,
            components: None,
            path: None,
            folder_sizes_at_least: index.folder_sizes_at_least(),
            seen_by: self.seen_by,
        };
        if !entry_clauses
            .iter()
            .all(|clause| clause.passes(&subject, None, &mut self.scratch))
        {
            return None;
        }

        if let Some(root) = scope
            && !in_scope(index, idx, root, &mut self.scope_memo)
        {
            return None;
        }

        // After the name tests, which reject most candidates of a full scan
        // first, so most never cost the read of their folder's descriptor;
        // before the path tests, so no hidden entry pays for its path.
        if !index.is_seen(idx, self.seen_by) {
            return None;
        }

        // An entry whose parent has no path (ancestry ends in lost+found) is
        // left out by `Reply`. Reject it here so it never takes one of the
        // limit's rows: a left-out row in a volume whose limit cut off later
        // matches ends the reply short, so `gozler -n 10 report` could print
        // nothing while the same query without `-n` prints the real matches.
        //
        // Applies the same `FolderCopy::entry_folder` rule that `Reply`
        // applies, so the two cannot drift. Gated on `has_orphans` because an
        // index with nothing under lost+found cannot contain an unpresentable
        // entry, and the walk it guards copies every distinct parent folder,
        // which a broad limited query would mostly never need. A query that
        // reads the path skips it too: `ComponentBits::entry_bits` and
        // `DirPaths::full_path_into` below refuse the same entries by the
        // same rule, without copying folders.
        if index.has_orphans()
            && self.component_bits.is_none()
            && self.query_paths.is_none()
            && self
                .folders
                .entry_folder(index, idx, &mut self.folder_names)
                .is_none()
        {
            return None;
        }

        let in_name = query.names_decide_path
            && path_clauses[0].passes_in_name(subject.name, &mut self.scratch.code_points);
        if let Some(bits) = &mut self.component_bits {
            let code_points = &mut self.scratch.code_points;
            // An entry without a path matches no path test.
            if in_name {
                bits.parent_bits(index, idx, |text, first| {
                    query.component_bits(text, first, code_points)
                })?;
            } else {
                let found = bits.entry_bits(index, idx, |text, first| {
                    query.component_bits(text, first, code_points)
                })?;
                subject.components = Some(found);
                if !component_clauses
                    .iter()
                    .all(|clause| clause.passes(&subject, None, &mut self.scratch))
                {
                    return None;
                }
            }
        }

        // An ordered sequence already in the filename also occurs in its
        // full path. Component checks above have validated its ancestry, so
        // that case needs no path construction.
        let in_name = in_name
            || !query.names_decide_path
                && subject.components.is_some()
                && path_clauses.iter().all(|clause| {
                    clause.passes_in_name(subject.name, &mut self.scratch.code_points)
                });
        if let Some(paths) = self.query_paths.as_mut().filter(|_| !in_name) {
            let path = paths.full_path_into(index, idx, &mut self.path)?;
            subject.path = Some(&self.path);
            if !path_clauses
                .iter()
                .all(|clause| clause.passes(&subject, path.target, &mut self.scratch))
            {
                return None;
            }
        }

        Some(SortRecord::new(idx))
    }
}

/// The extension of an entry: what follows the last `.` of a file's name, or
/// `None` for a name without a dot and for every folder, as in Everything,
/// so `ext:d` does not find a folder named `conf.d`. `ext:` matches it, and
/// the Extension column prints it.
pub fn extension(name: &[u8], is_dir: bool) -> Option<&[u8]> {
    if is_dir {
        return None;
    }
    let dot = name.iter().rposition(|&b| b == b'.')?;
    Some(&name[dot + 1..])
}

/// Is `node` a descendant of `root`? Memoized parent walk.
pub(super) fn in_scope(
    index: &VolumeIndex,
    node: EntryIdx,
    root: EntryIdx,
    memo: &mut FxHashMap<EntryIdx, bool>,
) -> bool {
    // A scope excludes the folder itself (es: descendants only).
    if node == root {
        return false;
    }
    // Mirrors `VolumeIndex::path_of`: directory depths under 32 stay on the
    // stack, so the per-candidate ancestry walk doesn't heap-allocate.
    let mut chain: SmallVec<[EntryIdx; 32]> = SmallVec::new();
    // The walk starts at the parent, so only folders are memoized: each entry
    // is asked about once, and memoizing it too would grow the memo by every
    // match.
    let mut cur = index.entry(node).parent();
    let verdict = loop {
        if cur == root {
            break true;
        }
        if cur == crate::types::NIL {
            break false;
        }
        if let Some(&v) = memo.get(&cur) {
            break v;
        }
        chain.push(cur);
        cur = index.entry(cur).parent();
        if chain.len() > 4096 {
            break false; // cycle guard
        }
    };
    for e in chain {
        memo.insert(e, verdict);
    }
    verdict
}

/// Sets the keys of every record under `sort` ([`primary_key`]), and returns
/// the folders of a path sort's records, whose ranks [`Matches::order`]
/// computes without the index: until then, each record's primary key holds
/// its folder's place in that table. A path sort copies the folder of every
/// record, because [`folder_ranks`] ranks every copied folder at once.
fn key_records(
    index: &VolumeIndex,
    records: &mut [SortRecord],
    (mut folders, mut names): (FolderCopy, ResultNames),
    sort: SortSpec,
    seen_by: &SeenBy,
) -> Option<ResultNames> {
    let extensions = match sort.key {
        SortKey::Extension => extension_ranks(index, records),
        _ => FxHashMap::default(),
    };
    for record in records.iter_mut() {
        let idx = record.idx();
        let entry = index.entry(idx);
        let name_key = index.name_key(idx);
        let value = match sort.key {
            SortKey::Name => KeyValue::Name(name_key),
            SortKey::Path => {
                // A record without a path is left out of the page, so any
                // rank serves it: the first folder's.
                let folder = folders.entry_folder(index, idx, &mut names);
                let place = folder.map_or(0, |f| f.index() as u64);
                *record = SortRecord::keyed(place, name_key, idx);
                continue;
            }
            SortKey::Size => KeyValue::Size(entry.size(seen_by)),
            SortKey::DateModified => KeyValue::DateModified(entry.mtime()),
            SortKey::DateCreated => KeyValue::DateCreated(entry.created()),
            SortKey::Extension => KeyValue::Extension(extensions[&folded_extension(index, idx)]),
        };
        *record = SortRecord::keyed(primary_key(sort.dir, value), name_key, idx);
    }
    matches!(sort.key, SortKey::Path).then_some(names)
}

/// The place of each distinct folded extension of `records`' entries in the
/// order [`compare`] gives them, `None` first.
fn extension_ranks<'i>(
    index: &'i VolumeIndex,
    records: &[SortRecord],
) -> FxHashMap<Option<&'i [u8]>, u32> {
    let mut ranks: FxHashMap<Option<&[u8]>, u32> = FxHashMap::default();
    for r in records {
        ranks.entry(folded_extension(index, r.idx())).or_insert(0);
    }
    let mut distinct: Vec<Option<&[u8]>> = ranks.keys().copied().collect();
    distinct.sort_unstable();
    for (rank, extension) in distinct.into_iter().enumerate() {
        ranks.insert(extension, rank as u32);
    }
    ranks
}

/// Folding keeps every `.` where it is, so the extension of the folded name
/// is the folded extension.
fn folded_extension(index: &VolumeIndex, idx: EntryIdx) -> Option<&[u8]> {
    extension(index.folded_name(idx), index.entry(idx).is_dir())
}

/// Orders each run of records with equal keys in `records`, which are sorted
/// by their keys, with [`compare`], which reads the names, the file IDs and
/// the parents from the index, and the sizes `seen_by` may read.
fn order_ties(index: &VolumeIndex, seen_by: &SeenBy, sort: SortSpec, records: &mut [SortRecord]) {
    use rayon::iter::ParallelIterator;
    use rayon::slice::ParallelSliceMut;
    let ranked = |record| Ranked {
        index,
        seen_by,
        sort,
        record,
    };
    records
        .par_chunk_by_mut(|a, b| a.keys() == b.keys())
        .for_each(|run| {
            if run.len() > 1 {
                run.sort_unstable_by(|&a, &b| compare(sort, &ranked(a), &ranked(b)));
            }
        });
}

/// A record as the result order reads it: straight from the index, so a
/// tie-break key is loaded only when a comparison reaches it.
struct Ranked<'a> {
    index: &'a VolumeIndex,
    seen_by: &'a SeenBy,
    sort: SortSpec,
    record: SortRecord,
}

impl Ranked<'_> {
    fn entry(&self) -> EntryView<'_> {
        self.index.entry(self.record.idx())
    }
}

impl OrderFields for Ranked<'_> {
    fn is_dir(&self) -> bool {
        self.entry().is_dir()
    }
    fn folder_rank(&self) -> u32 {
        packed_folder_rank(self.sort.dir, self.record.primary())
    }
    fn folded_name(&self) -> &[u8] {
        self.index.folded_name(self.record.idx())
    }
    fn size(&self) -> Option<u64> {
        self.entry().size(self.seen_by)
    }
    fn mtime(&self) -> Option<i64> {
        self.entry().mtime()
    }
    fn created(&self) -> Option<i64> {
        self.entry().created()
    }
    fn volume(&self) -> &[u8] {
        b"" // one volume per query run
    }
    fn frn(&self) -> Frn {
        self.entry().frn()
    }
    fn parent_frn(&self) -> Frn {
        self.index.entry(self.entry().parent()).frn()
    }
    fn name(&self) -> &[u8] {
        self.index.entry(self.record.idx()).name()
    }
}

/// Resolves a scope folder path (WTF-8, no volume prefix) to its entry,
/// walking components from the volume root: case-insensitively via the folded
/// directory index, or case-sensitively against raw names when `match_case`. Returns `None`
/// if any component is missing.
pub fn resolve_scope(index: &VolumeIndex, path: &[u8], match_case: bool) -> Option<EntryIdx> {
    let components: Vec<&[u8]> = path
        .split(|&b| b == b'\\' || b == b'/')
        .filter(|c| !c.is_empty())
        .collect();
    if components.is_empty() {
        return Some(index.root());
    }

    // Common case: walk the persistent directory index, O(components). It is
    // keyed by folded name, so it serves case-insensitive scopes directly with
    // no per-query scan of the volume.
    if !match_case {
        let mut cur = index.root();
        for comp in components {
            cur = index.child_dir(cur, crate::fold::fold(comp))?;
        }
        return Some(cur);
    }

    // Case-sensitive scopes are rare (the persistent index is folded), so we do
    // not build a whole-volume raw-name map (one key Vec per directory) per
    // query. Walk each component with a linear scan of the live entries,
    // comparing raw names directly (no allocation), stopping at the first
    // matching sibling. NOTE: do not reuse the folded `child_dir` walk with a
    // post-hoc raw-name check: `dir_children` keeps a single entry per
    // (parent, folded name), so it cannot represent case-variant siblings and
    // would miss a legitimately case-matching sibling that folding collapsed.
    let mut cur = index.root();
    'components: for comp in components {
        for idx in index.live_entries() {
            if index.is_synthetic(idx) {
                continue;
            }
            let e = index.entry(idx);
            if e.is_dir() && e.parent() == cur && e.name() == comp {
                cur = idx;
                continue 'components;
            }
        }
        return None;
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attributes::Attributes;
    use crate::index::{NTFS_ROOT_FRN, VolumeIndex};
    use crate::query::parse::{
        Clause, ParseOptions, Term, TermMatch, parse_query, parse_query_with,
    };
    use crate::query::rows::{QueryOutcome, Reply, VolumeRows, run_query};
    use crate::types::{Frn, SortDir};
    use crate::usn::record::{
        FILE_ATTRIBUTE_DIRECTORY, ParsedUsnRecord, USN_REASON_FILE_CREATE, USN_REASON_FILE_DELETE,
        USN_REASON_RENAME_NEW_NAME,
    };

    fn compiled(query: &str) -> CompiledQuery {
        CompiledQuery::new(&parse_query(query).unwrap()).unwrap()
    }

    fn frn(rec: u64) -> Frn {
        Frn::ntfs(rec, 1)
    }

    fn enum_rec(f: Frn, parent: Frn, name: &str, is_dir: bool) -> ParsedUsnRecord {
        ParsedUsnRecord {
            frn: f,
            parent_frn: parent,
            reason: USN_REASON_FILE_CREATE,
            attributes: if is_dir { FILE_ATTRIBUTE_DIRECTORY } else { 0 },
            name: name.as_bytes().to_vec(),
        }
    }

    /// Builds: docs/ (100), docs/sub/ (101), docs/sub/Report.PDF (102, 4KB),
    /// docs/notes.txt (103, 200B), photo.JPG (104, 2MB) at root.
    fn sample_index() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        let root = NTFS_ROOT_FRN;
        for r in [
            enum_rec(frn(100), root, "docs", true),
            enum_rec(frn(101), frn(100), "sub", true),
            enum_rec(frn(102), frn(101), "Report.PDF", false),
            enum_rec(frn(103), frn(100), "notes.txt", false),
            enum_rec(frn(104), root, "photo.JPG", false),
        ] {
            idx.insert_enum(&r);
        }
        // Enrich sizes via the layout path.
        set_size(&mut idx, frn(102), 4096);
        set_size(&mut idx, frn(103), 200);
        set_size(&mut idx, frn(104), 2 * 1024 * 1024);
        idx
    }

    /// An orphan: a real file whose parent was never seen, so the parent is a
    /// placeholder under lost+found and the file has no presentable path.
    /// Reachable whenever a genuine gap exists (journal wrap, missed parent
    /// record, FRN sequence mismatch), and densest during bootstrap and rescan,
    /// which are exactly the windows the server still queries.
    fn index_with_orphans() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        let root = NTFS_ROOT_FRN;
        // Two presentable hits.
        idx.insert_enum(&enum_rec(frn(100), root, "real1.log", false));
        idx.insert_enum(&enum_rec(frn(101), root, "real2.log", false));
        // Three orphans: parent 999 is never inserted, so it is synthesized as a
        // placeholder under lost+found and path_of on it is not Ok.
        for (f, n) in [
            (200, "ghost1.log"),
            (201, "ghost2.log"),
            (202, "ghost3.log"),
        ] {
            idx.insert_enum(&enum_rec(frn(f), frn(999), n, false));
        }
        idx
    }

    /// `total` and the page must count the same things.
    ///
    /// A reply leaves out a row whose folder has no full path. Orphans that
    /// the scan kept would take the limit's rows, and the volume would then
    /// run out of rows while its limit cut off the real matches, which ends
    /// the reply short: a limited query would return NOTHING for files that
    /// exist, while the unlimited one returns the 2 real matches.
    #[test]
    fn orphans_are_excluded_from_total_and_never_eat_the_page() {
        let idx = index_with_orphans();
        let parsed = compiled("log");

        let all = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec::default_for(SortKey::Name),
            None,
        );
        assert_eq!(all.rows.len(), 2, "only the presentable files are rows");
        assert_eq!(
            all.total, 2,
            "total must count what the page can actually return, not orphans"
        );

        // The failing case: a limit small enough that orphans would have filled it.
        for key in [SortKey::Name, SortKey::Path] {
            let page = run_query(&idx, "", &parsed, None, SortSpec::default_for(key), Some(3));
            assert_eq!(
                page.rows.len(),
                2,
                "{key:?}: a limited query must still return the real hits"
            );
            assert_eq!(
                page.total, 2,
                "{key:?}: total must not count dropped orphans"
            );
        }
    }

    fn set_size(idx: &mut VolumeIndex, f: Frn, size: u64) {
        use crate::layout::{LayoutFile, LayoutName};
        // Reuse the existing name/parent by reading them back.
        let head = idx.head_of(f).unwrap();
        let e = idx.entry(head);
        let name = e.name().to_vec();
        let parent_frn = idx.entry(e.parent()).frn();
        idx.enrich(&LayoutFile {
            frn: f,
            attributes: 0x20,
            size: Some(size),
            mtime_ft: Some(1000),
            created_ft: None,
            security: None,
            names: vec![LayoutName {
                parent_frn,
                name,
                dos_only: false,
            }],
        });
    }

    fn run(idx: &VolumeIndex, query: &str) -> Vec<String> {
        let parsed = compiled(query);
        let out = run_query(idx, "", &parsed, None, SortSpec::default(), None);
        texts(&out)
    }

    #[test]
    fn case_insensitive_substring_over_names() {
        let idx = sample_index();
        assert_eq!(run(&idx, "report"), vec!["docs\\sub\\Report.PDF"]);
        assert_eq!(run(&idx, "REPORT"), vec!["docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn multi_term_and() {
        let idx = sample_index();
        // Both terms must be substrings of the same name.
        assert!(run(&idx, "notes zzz").is_empty()); // "zzz" is in no name
        assert_eq!(run(&idx, "notes txt"), vec!["docs\\notes.txt"]);
    }

    #[test]
    fn ext_filter() {
        let idx = sample_index();
        assert_eq!(run(&idx, "ext:pdf"), vec!["docs\\sub\\Report.PDF"]);
        let mut jpg = run(&idx, "ext:jpg");
        jpg.sort();
        assert_eq!(jpg, vec!["photo.JPG"]);
    }

    /// `ext:` with no extension finds the files without one, as in es, where
    /// a folder has no extension; it matched every entry before (#21). `!`
    /// and `|` take it as one term.
    #[test]
    fn ext_alone_finds_files_without_an_extension() {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        for r in [
            enum_rec(frn(100), NTFS_ROOT_FRN, "src", true),
            enum_rec(frn(101), NTFS_ROOT_FRN, "LICENSE", false),
            enum_rec(frn(102), NTFS_ROOT_FRN, "main.rs", false),
            enum_rec(frn(103), NTFS_ROOT_FRN, "conf.rs", true),
        ] {
            idx.insert_enum(&r);
        }
        assert_eq!(run(&idx, "ext:"), ["LICENSE"]);
        // A folder has no extension, whatever its name.
        assert_eq!(run(&idx, "ext:rs"), ["main.rs"]);
        assert_eq!(run(&idx, "!ext:"), ["conf.rs", "main.rs", "src"]);
        assert_eq!(run(&idx, "ext:|ext:rs"), ["LICENSE", "main.rs"]);
    }

    #[test]
    fn size_filter_excludes_unknown_and_out_of_range() {
        let idx = sample_index();
        // > 1MB → only the 2MB photo.
        assert_eq!(run(&idx, "size:>1mb"), vec!["photo.JPG"]);
        // tiny (≤10KB) → notes (200B) and report (4KB), not the 2MB photo.
        let mut tiny = run(&idx, "size:tiny");
        tiny.sort();
        assert_eq!(tiny, vec!["docs\\notes.txt", "docs\\sub\\Report.PDF"]);
    }

    /// Once the size and date pass ends, a folder has the size of the files
    /// below it: `docs` holds 4,096 + 200 bytes, more than any file in it.
    /// The totals still add only files, so a folder and its files count each
    /// file once.
    #[test]
    fn a_folder_is_as_large_as_the_files_below_it() {
        let mut idx = sample_index();
        idx.compute_folder_sizes();
        assert_eq!(run(&idx, "size:>4kb"), ["docs", "photo.JPG"]);

        let parsed = compiled("");
        let by_size = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec::default_for(SortKey::Size),
            None,
        );
        let order: Vec<String> = texts(&by_size);
        assert_eq!(
            order,
            [
                "photo.JPG",
                "docs",
                "docs\\sub\\Report.PDF",
                "docs\\sub",
                "docs\\notes.txt"
            ]
        );

        let docs = resolve_scope(&idx, b"docs", false).unwrap();
        let totals = run_totals(&idx, "", &parsed, Some(docs), &SeenBy::Everything).unwrap();
        assert_eq!((totals.count, totals.size), (3, 4096 + 200));
    }

    /// After a size pass that stopped, a folder's size counts only the files
    /// the pass read. A size test matches it only when every larger size
    /// passes too, because its unread files could take it past an upper
    /// bound: `size:<1mb` would otherwise list a folder that may hold
    /// gigabytes. `docs` holds at least 4,296 bytes and `docs\sub` at least
    /// 4,096.
    #[test]
    fn a_folder_size_after_a_stopped_pass_is_a_lower_bound() {
        let mut idx = sample_index();
        idx.compute_folder_sizes();
        idx.record_size_pass_stopped();
        assert_eq!(run(&idx, "folder: size:>4kb"), ["docs"]);
        assert!(run(&idx, "folder: size:<1mb").is_empty());
        // `docs\sub` may or may not pass, so its negation does not match it
        // either.
        assert!(run(&idx, "folder: !size:>4kb").is_empty());
    }

    #[test]
    fn kind_filters() {
        let idx = sample_index();
        let mut folders = run(&idx, "folder:");
        folders.sort();
        assert_eq!(folders, vec!["docs", "docs\\sub"]);
    }

    #[test]
    fn wildcard_whole_name() {
        let idx = sample_index();
        assert_eq!(run(&idx, "*.pdf"), vec!["docs\\sub\\Report.PDF"]);
        assert!(run(&idx, "*.zip").is_empty());
    }

    #[test]
    fn path_term_matches_full_path() {
        let idx = sample_index();
        let mut hits = run(&idx, "docs\\sub");
        hits.sort();
        // Every entry whose full path contains "docs\sub".
        assert_eq!(hits, vec!["docs\\sub", "docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn scope_restricts_to_descendants() {
        let idx = sample_index();
        let parsed = compiled("");
        let scope = Some(r"C:\docs\sub");
        let out = run_query(&idx, r"C:\", &parsed, scope, SortSpec::default(), None);
        let paths: Vec<String> = texts(&out);
        assert_eq!(paths, vec![r"C:\docs\sub\Report.PDF"]); // descendants only, not "docs\sub" itself
    }

    #[test]
    fn sort_path_orders_by_parent_then_name() {
        let idx = sample_index();
        let parsed = compiled("");
        let out = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec {
                key: SortKey::Path,
                dir: SortDir::Asc,
            },
            None,
        );
        let paths: Vec<String> = texts(&out);
        // Root-level entries (empty parent) sort first, then docs\, then docs\sub\.
        assert_eq!(
            paths,
            vec![
                "docs",
                "photo.JPG",
                "docs\\notes.txt",
                "docs\\sub",
                "docs\\sub\\Report.PDF",
            ]
        );
    }

    #[test]
    fn limit_cuts_the_page_but_total_is_full_count() {
        let idx = sample_index();
        let parsed = compiled("");
        let out = run_query(
            &idx,
            "",
            &parsed,
            None,
            SortSpec {
                key: SortKey::Name,
                dir: SortDir::Asc,
            },
            Some(2),
        );
        assert_eq!(out.total, 5);
        assert_eq!(out.rows.len(), 2);
    }

    #[test]
    fn case_sensitive_query_respects_case() {
        let idx = sample_index();
        // Report.PDF has capital R; case: makes "report" not match.
        assert!(run(&idx, "report case:").is_empty());
        assert_eq!(run(&idx, "Report case:"), vec!["docs\\sub\\Report.PDF"]);
    }

    #[test]
    fn ext_filter_is_case_insensitive_even_under_case_directive() {
        // Report.PDF has an uppercase extension; ext: is case-insensitive
        // even when the query is otherwise case-sensitive.
        let idx = sample_index();
        assert_eq!(
            run(&idx, "Report ext:pdf case:"),
            vec!["docs\\sub\\Report.PDF"]
        );
        assert_eq!(
            run(&idx, "Report ext:PDF case:"),
            vec!["docs\\sub\\Report.PDF"]
        );
    }

    // -- path terms and path wildcards --------------------------------------

    /// `docs\` at the root and deeper, so a path wildcard is tested at both.
    /// `readme.md` repeats at every level, so a name term drives the arena
    /// scan to every one of them and the path wildcard must pick.
    fn docs_index() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        let root = NTFS_ROOT_FRN;
        for r in [
            enum_rec(frn(100), root, "docs", true),
            enum_rec(frn(101), root, "repo", true),
            enum_rec(frn(102), frn(101), "docs", true),
            enum_rec(frn(103), frn(101), "src", true),
            enum_rec(frn(200), frn(100), "readme.md", false),
            enum_rec(frn(201), frn(102), "readme.md", false),
            enum_rec(frn(202), frn(102), "guide.MD", false),
            enum_rec(frn(203), frn(103), "readme.md", false),
            enum_rec(frn(204), frn(103), "main.cs", false),
            enum_rec(frn(205), frn(101), "readme.md", false),
        ] {
            idx.insert_enum(&r);
        }
        idx
    }

    fn paths_under(
        idx: &VolumeIndex,
        prefix: &str,
        query: &str,
        scope: Option<&str>,
    ) -> Vec<String> {
        let parsed = compiled(query);
        let out = run_query(
            idx,
            prefix,
            &parsed,
            scope,
            SortSpec::default_for(SortKey::Path),
            None,
        );
        texts(&out)
    }

    /// A wildcard with a separator matches the whole full path, on every scan
    /// path: the path candidate walk, the folded-arena scan (a name term drives it), and
    /// the scoped subtree walk. The wildcard's literal `\docs\` is in no name,
    /// so feeding it to the arena prefilter would find nothing.
    #[test]
    fn a_wildcard_with_a_separator_matches_the_full_path() {
        let idx = docs_index();
        let docs = [
            r"C:\docs\readme.md",
            r"C:\repo\docs\guide.MD",
            r"C:\repo\docs\readme.md",
        ];

        // Path candidates: no name term drives the scan.
        assert_eq!(paths_under(&idx, r"C:\", r"*\docs\*.md", None), docs);
        // Folded-arena scan: `readme` drives it.
        assert_eq!(
            paths_under(&idx, r"C:\", r"*\docs\*.md readme", None),
            [r"C:\docs\readme.md", r"C:\repo\docs\readme.md"]
        );
        // Scoped walk.
        assert_eq!(
            paths_under(&idx, r"C:\", r"*\docs\*.md", Some(r"C:\repo")),
            [r"C:\repo\docs\guide.MD", r"C:\repo\docs\readme.md"]
        );
        // `path:` with a wildcard is a path wildcard too.
        assert_eq!(paths_under(&idx, r"C:\", r"path:*\docs\*.md", None), docs);
    }

    /// The full path starts at the mount prefix, so an anchored wildcard needs
    /// the drive, as in es: `C:\repo\*.cs` finds the `.cs` files under `repo`,
    /// and `repo\*.cs`, which no full path starts with, finds nothing.
    #[test]
    fn a_path_wildcard_is_anchored_to_the_whole_full_path() {
        let idx = docs_index();
        assert_eq!(
            paths_under(&idx, r"C:\", r"C:\repo\*.cs", None),
            [r"C:\repo\src\main.cs"]
        );
        assert!(paths_under(&idx, r"C:\", r"repo\*.cs", None).is_empty());
        // `?` stands for one character, a separator included.
        assert_eq!(
            paths_under(&idx, r"C:\", r"C:\repo?src\main.c?", None),
            [r"C:\repo\src\main.cs"]
        );
    }

    /// A case-sensitive path wildcard matches the raw full path.
    #[test]
    fn a_case_sensitive_path_wildcard_matches_the_raw_path() {
        let idx = docs_index();
        assert_eq!(
            paths_under(&idx, r"C:\", r"*\docs\*.MD case:", None),
            [r"C:\repo\docs\guide.MD"]
        );
    }

    /// Path terms see the same full path, mount prefix included.
    #[test]
    fn a_path_term_matches_anywhere_in_the_full_path() {
        let idx = docs_index();
        assert_eq!(
            paths_under(&idx, r"C:\", r"c:\docs", None),
            [r"C:\docs", r"C:\docs\readme.md"]
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"repo\src", None),
            [
                r"C:\repo\src",
                r"C:\repo\src\main.cs",
                r"C:\repo\src\readme.md"
            ]
        );
    }

    /// `parent:` finds the entries directly in a folder, on the full scan,
    /// the folded-arena scan and the scoped walk, in either case form. A
    /// trailing separator, a root's included, is ignored.
    #[test]
    fn parent_finds_the_entries_directly_in_a_folder() {
        let idx = docs_index();
        let repo = [r"C:\repo\docs", r"C:\repo\readme.md", r"C:\repo\src"];
        assert_eq!(paths_under(&idx, r"C:\", r"parent:C:\repo", None), repo);
        assert_eq!(
            paths_under(&idx, r"C:\", r"parent:c:\REPO\", Some(r"C:\repo")),
            repo
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"parent:C:\repo readme", None),
            [r"C:\repo\readme.md"]
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"parent:C:\", None),
            [r"C:\docs", r"C:\repo"]
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"parent:C:\repo case:", None),
            repo
        );
        assert!(paths_under(&idx, r"C:\", r"parent:C:\Repo case:", None).is_empty());
    }

    // -- whole words and regular expressions --------------------------------

    /// A whole word has no letter or digit right before or after it, so `_`
    /// and punctuation end a word, as in Everything, and a letter outside
    /// ASCII does not. An occurrence inside a longer word does not hide a
    /// later one that overlaps it. The name term drives the folded-arena
    /// scan, and the path term the full scan.
    #[test]
    fn a_whole_word_has_no_letter_or_digit_beside_it() {
        let idx = flat_index(&[
            "my_REPORT.txt".to_string(),
            "report-v2.txt".to_string(),
            "reports.txt".to_string(),
            "2report.txt".to_string(),
            "üreport.txt".to_string(),
            "xa-a-a".to_string(),
        ]);
        let found = |query: &str| paths_under(&idx, r"C:\", query, None);
        assert_eq!(
            found("ww:report"),
            [r"C:\my_REPORT.txt", r"C:\report-v2.txt"]
        );
        assert_eq!(found("ww:a-a"), [r"C:\xa-a-a"]);
        assert_eq!(found(r"ww:\report"), [r"C:\report-v2.txt"]);
        assert!(found(r"ww:\repor").is_empty());
    }

    /// A word starts at the start of the name or after a character that is
    /// not a letter or digit, a term that starts with such a character, as
    /// `.txt` does, starts its own word, and `startwith:` matches only at
    /// the start of the name; with `-p`, both read the full path, which
    /// starts with `C:`.
    /// The name terms drive the folded-arena scan, and `-case` the full scan.
    #[test]
    fn word_start_and_name_start_read_where_the_name_or_a_word_starts() {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        idx.insert_enum(&enum_rec(frn(100), NTFS_ROOT_FRN, "docs", true));
        let names = [
            "Paracetamol 500mg.pdf",
            "IMG_1234.jpg",
            "comparator.txt",
            "my report.txt",
        ];
        for (i, name) in names.into_iter().enumerate() {
            idx.insert_enum(&enum_rec(frn(200 + i as u64), frn(100), name, false));
        }
        let found = |query: &str, match_case, match_path| {
            let options = ParseOptions {
                match_case,
                match_path,
                ..ParseOptions::default()
            };
            paths_of(&idx, parse_query_with(query, options).unwrap())
        };
        let in_names = |query: &str| found(query, false, false);
        let pills = [r"C:\docs\Paracetamol 500mg.pdf"];
        assert_eq!(in_names("wordstart:para wordstart:50"), pills);
        assert_eq!(in_names("wordstart:1234"), [r"C:\docs\IMG_1234.jpg"]);
        assert_eq!(in_names("wordstart:para"), pills);
        assert_eq!(in_names("wordstart:c"), [r"C:\docs\comparator.txt"]);
        assert_eq!(in_names("startwith:para"), pills);
        assert_eq!(in_names("wordstart:report"), [r"C:\docs\my report.txt"]);
        assert_eq!(
            in_names("wordstart:.txt"),
            [r"C:\docs\comparator.txt", r"C:\docs\my report.txt"]
        );
        assert!(in_names("startwith:report").is_empty());
        assert_eq!(in_names("wordstart:PARA"), pills);
        assert!(found("wordstart:PARA", true, false).is_empty());

        assert_eq!(in_names("wordstart:docs"), [r"C:\docs"]);
        assert_eq!(
            found("wordstart:docs", false, true),
            [
                r"C:\docs",
                r"C:\docs\comparator.txt",
                r"C:\docs\IMG_1234.jpg",
                r"C:\docs\my report.txt",
                r"C:\docs\Paracetamol 500mg.pdf",
            ]
        );
        assert!(found("wordstart:ocs", false, true).is_empty());
    }

    /// The window shows these parts bold as the reason a row matched, so a
    /// part must be where a term matched: a lost word-start check would
    /// bold `para` in `comparator.txt`, and a range of the folded text used
    /// as it is would cut the stored name where folding changed a length.
    #[test]
    fn matched_parts_are_where_the_terms_matched() {
        let parts = |query: &str, term_match, path: &'static str| -> Vec<&'static str> {
            let options = ParseOptions {
                term_match,
                ..ParseOptions::default()
            };
            let query = parse_query_with(query, options).unwrap();
            let ranges = MatchedParts::new(&query).unwrap().ranges(path);
            ranges.into_iter().map(|r| &path[r]).collect()
        };
        let pills = r"C:\docs\Paracetamol 500mg.pdf";
        assert_eq!(
            parts("para 50", TermMatch::WordStart, pills),
            ["Para", "50"]
        );
        assert!(parts("para", TermMatch::WordStart, r"C:\docs\comparator.txt").is_empty());
        // İ folds to the one byte of i.
        assert_eq!(
            parts("report", TermMatch::Anywhere, r"C:\İİ report.txt"),
            ["report"]
        );
        // A path term reaches into the folder, a wildcard gives its
        // literal characters, and the other tests give nothing.
        assert_eq!(
            parts(r"docs\para size:>1 !x", TermMatch::Anywhere, pills),
            [r"docs\Para"]
        );
        assert_eq!(parts("*.pdf", TermMatch::Anywhere, pills), [".pdf"]);
    }

    /// Each character boundary of `raw` with the folded length of the whole
    /// text before it, folded the slow way that assumes nothing about how
    /// folded lengths grow.
    fn every_boundary(raw: &str) -> Vec<(usize, usize)> {
        raw.char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(raw.len()))
            .map(|i| (i, crate::fold::fold(&raw.as_bytes()[..i]).len()))
            .collect()
    }

    /// Maps a compared range back by a scan of `every_boundary`.
    fn raw_range_by_scan(
        raw: &str,
        boundaries: &[(usize, usize)],
        range: std::ops::Range<usize>,
    ) -> std::ops::Range<usize> {
        let start = boundaries
            .iter()
            .rev()
            .find(|&&(_, at)| at <= range.start)
            .map_or(0, |&(raw_at, _)| raw_at);
        let end = boundaries
            .iter()
            .find(|&&(_, at)| at >= range.end)
            .map_or(raw.len(), |&(raw_at, _)| raw_at);
        start..end.max(start)
    }

    /// Checks [`Compared::raw_range`] against the slow mapping for every
    /// start and every end a range of `raw`'s folded text can have; a range
    /// maps each end on its own.
    fn assert_raw_range_agrees(raw: &str) {
        let compared = Compared::new(raw, false);
        let boundaries = every_boundary(raw);
        let len = compared.text.len();
        for range in (0..=len).map(|at| at..len).chain((0..=len).map(|at| 0..at)) {
            assert_eq!(
                compared.raw_range(range.clone()),
                raw_range_by_scan(raw, &boundaries, range.clone()),
                "{raw:?} {range:?}"
            );
        }
    }

    /// The folded length of a prefix can shrink as the prefix grows: `o`
    /// with U+0304 and U+0323 folds to `ọ` and U+0304, and U+0328 after them
    /// reorders the marks and composes `ǭ`, which is shorter. A mapping that
    /// assumed otherwise bolded two marks before ` report`.
    #[test]
    fn raw_range_agrees_where_a_mark_reorders_and_composes() {
        for letter in ["o\u{0304}\u{0323}", "\u{014D}\u{0323}", "\u{1ECD}\u{0304}"] {
            assert_raw_range_agrees(&format!("M{letter}\u{0328} report.txt"));
        }
    }

    /// Letters that fold to another length, letters and marks that compose
    /// or reorder, `o` and its marks among them, Hangul jamo that compose
    /// into one syllable, and a backslash and ASCII between them.
    const FOLDED_CHARACTERS: &str = "aoE\\ éÉİßẞΣΩЙἀK\u{212A}\u{014D}\u{1ECD}\u{01ED}\u{0301}\
        \u{0304}\u{0306}\u{0308}\u{0323}\u{0328}\u{0345}\u{1100}\u{1161}\u{11A8}\u{AC00}\
        \u{0B47}\u{0B3E}\u{1D15E}😀";

    proptest! {
        /// A mapping that differs from folding every prefix bolds the wrong
        /// characters of a name.
        #[test]
        fn raw_range_agrees_with_mapping_every_boundary(
            characters in prop::collection::vec(
                prop::sample::select(FOLDED_CHARACTERS.chars().collect::<Vec<_>>()),
                1..40,
            ),
        ) {
            assert_raw_range_agrees(&characters.into_iter().collect::<String>());
        }
    }

    /// A regular expression matches anywhere in the name unless it anchors,
    /// in the query's case. With `\\` it matches the full path, whose
    /// separators are `\`.
    #[test]
    fn a_regex_matches_the_name_or_with_a_backslash_the_full_path() {
        let idx = docs_index();
        let readmes = [
            r"C:\docs\readme.md",
            r"C:\repo\readme.md",
            r"C:\repo\docs\readme.md",
            r"C:\repo\src\readme.md",
        ];
        assert_eq!(
            paths_under(&idx, r"C:\", r"regex:^read.e\.md$", None),
            readmes
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"regex:\.MD$ case:", None),
            [r"C:\repo\docs\guide.MD"]
        );
        assert_eq!(
            paths_under(&idx, r"C:\", r"regex:^c:\\repo\\docs\\", None),
            [r"C:\repo\docs\guide.MD", r"C:\repo\docs\readme.md"]
        );
    }

    // -- clauses ------------------------------------------------------------

    fn query_of(clauses: Vec<Vec<Term>>) -> ParsedQuery {
        ParsedQuery {
            clauses: clauses.into_iter().map(|terms| Clause { terms }).collect(),
            match_case: false,
        }
    }

    fn term(negated: bool, test: Test) -> Term {
        Term { negated, test }
    }

    fn name(text: &str) -> Test {
        Test::Name(text.to_string())
    }

    fn paths_of(idx: &VolumeIndex, query: ParsedQuery) -> Vec<String> {
        let query = CompiledQuery::new(&query).unwrap();
        let out = run_query(
            idx,
            r"C:\",
            &query,
            None,
            SortSpec::default_for(SortKey::Path),
            None,
        );
        texts(&out)
    }

    #[test]
    fn ordered_terms_reject_reversed_and_overlapping_matches() {
        let mut idx = sample_index();
        for (i, name) in [
            "Paracetamol 500mg",
            "500mg paracetamol",
            "comparator 500mg paracetamol",
            "paracetamol",
        ]
        .into_iter()
        .enumerate()
        {
            idx.insert_enum(&enum_rec(frn(200 + i as u64), NTFS_ROOT_FRN, name, false));
        }
        idx.insert_enum(&enum_rec(frn(300), NTFS_ROOT_FRN, "Paracetamol", true));
        idx.insert_enum(&enum_rec(frn(301), frn(300), "500mg.txt", false));
        for match_path in [false, true] {
            let mut query = parse_query_with(
                "para 50",
                ParseOptions {
                    match_path,
                    term_match: TermMatch::WordStart,
                    ..ParseOptions::default()
                },
            )
            .unwrap();
            query.order_terms(TermMatch::WordStart);
            let mut found = paths_of(&idx, query);
            found.sort();
            let expected = if match_path {
                vec![r"C:\Paracetamol 500mg", r"C:\Paracetamol\500mg.txt"]
            } else {
                vec![r"C:\Paracetamol 500mg"]
            };
            assert_eq!(found, expected);
        }
        let mut query = parse_query("paracetamol paracetamol").unwrap();
        query.order_terms(TermMatch::Anywhere);
        assert!(paths_of(&idx, query).is_empty());
    }

    #[test]
    fn ordered_names_do_not_bypass_other_path_constraints() {
        let mut idx = sample_index();
        idx.insert_enum(&enum_rec(
            frn(200),
            NTFS_ROOT_FRN,
            "Paracetamol 500mg",
            false,
        ));
        idx.insert_enum(&enum_rec(frn(201), frn(100), "Paracetamol 500mg", false));
        let ordered = |text| {
            let mut parsed = parse_query_with(
                text,
                ParseOptions {
                    match_path: true,
                    term_match: TermMatch::WordStart,
                    ..ParseOptions::default()
                },
            )
            .unwrap();
            parsed.order_terms(TermMatch::WordStart);
            parsed
        };
        assert_eq!(
            paths_of(&idx, ordered("para 50 !path:docs")),
            [r"C:\Paracetamol 500mg"]
        );
        assert!(paths_of(&idx, ordered("para 50 ww:para")).is_empty());
        let links = Links::new(vec![crate::links::FolderLink {
            link: r"C:\docs\jump".into(),
            target: r"D:\target".into(),
        }]);
        let query =
            CompiledQuery::with_links(&ordered("para 50"), &links, Some(r"C:\docs")).unwrap();
        let out = run_query(&idx, r"C:\", &query, None, SortSpec::default(), None);
        assert_eq!(texts(&out), [r"C:\docs\Paracetamol 500mg"]);
    }

    /// The owner's bug, at the scan: the index holds each entry once, at its
    /// real path, so without the links a path through a junction, and a
    /// scope that holds one, find nothing below its target.
    #[test]
    fn a_search_through_links_finds_what_is_below_their_targets() {
        let mut idx = sample_index();
        idx.insert_enum(&enum_rec(frn(105), NTFS_ROOT_FRN, "proj", true));
        idx.insert_enum(&enum_rec(frn(106), frn(105), "out", true));
        let links = Links::new(vec![
            crate::links::FolderLink {
                link: r"C:\proj\out".into(),
                target: r"C:\docs\sub".into(),
            },
            crate::links::FolderLink {
                link: r"C:\other\via".into(),
                target: r"C:\docs\sub".into(),
            },
        ]);
        let found = |query: &str, scope: Option<&str>| {
            let parsed = parse_query(query).unwrap();
            let query = CompiledQuery::with_links(&parsed, &links, scope).unwrap();
            let sort = SortSpec::default_for(SortKey::Path);
            texts(&run_query(&idx, r"C:\", &query, scope, sort, None))
        };
        let report = vec![r"C:\docs\sub\Report.PDF".to_string()];
        assert_eq!(found(r"proj\out\rep", None), report);
        assert_eq!(found(r"proj\out\rep other\via\rep", None), report);
        assert!(found(r"proj\out\rep !other\via\rep", None).is_empty());
        assert!(found(r"proj\out\rep !C:\docs\", None).is_empty());
        assert!(found(r"report path:proj", None).is_empty());
        assert_eq!(found("report", Some(r"C:\proj\out")), report);
        assert_eq!(
            found("report", Some(r"C:\proj")),
            report,
            "the scope holds the junction"
        );
        assert!(found("notes", Some(r"C:\proj")).is_empty());
    }

    #[test]
    fn path_candidates_preserve_matches_through_links_and_count_each_entry_once() {
        let mut index = sample_index();
        for n in 0..2048 {
            index.insert_enum(&enum_rec(
                frn(10_000 + n),
                NTFS_ROOT_FRN,
                &format!("unrelated_{n}"),
                false,
            ));
        }
        let links = Links::new(vec![
            crate::links::FolderLink {
                link: r"C:\proj\out".into(),
                target: r"C:\docs\sub".into(),
            },
            crate::links::FolderLink {
                link: r"C:\again\out".into(),
                target: r"C:\docs\sub".into(),
            },
        ]);
        for text in [
            r"proj\out\rep",
            r"proj\out\",
            r"*\out\*.pdf",
            r"C:\proj\out\Report.PDF",
            r"C:\proj\out\*.pdf",
            r"docs\sub\rep",
            r"*\docs\*.pdf",
            r"C:\*\sub\*",
            r"docs\sub\rep|proj\out\rep",
            r"proj\out\rep !missing",
        ] {
            let parsed = parse_query_with(
                text,
                ParseOptions {
                    match_path: true,
                    ..ParseOptions::default()
                },
            )
            .unwrap();
            let query = CompiledQuery::with_links(&parsed, &links, None).unwrap();
            let seen = SeenBy::Everything;
            let actual = scan(&index, r"C:\", &query, None, &seen);
            let mut full = Scan::new(&index, r"C:\", &query, &seen);
            for entry in index
                .live_entries()
                .filter(|&entry| !index.is_synthetic(entry))
            {
                full.visit(entry, None);
            }
            let sorted = |scan: Scan<'_, '_>| {
                let mut entries: Vec<_> = scan.records.iter().map(|record| record.idx()).collect();
                entries.sort_unstable();
                entries
            };
            assert_eq!(sorted(actual), sorted(full), "{text}");
        }
    }

    /// The evaluator reads what the parser writes for `!` and `|`: a clause
    /// passes when any of its terms does, and a negated term
    /// when its test fails, except that a size test of an unknown size fails
    /// either way. Only a clause of one positive name term may drive the
    /// folded-arena scan: `guide|main` driven by `guide` would never reach
    /// `main.cs`, and `!readme` driven by `readme` would find nothing.
    #[test]
    fn clauses_are_ors_of_possibly_negated_terms() {
        let idx = docs_index();
        let guide = r"C:\repo\docs\guide.MD";
        let main = r"C:\repo\src\main.cs";
        let file = || term(false, Test::Kind(Kind::File));
        assert_eq!(
            paths_of(
                &idx,
                query_of(vec![vec![
                    term(false, name("guide")),
                    term(false, name("main"))
                ]])
            ),
            [guide, main]
        );
        assert_eq!(
            paths_of(
                &idx,
                query_of(vec![vec![term(true, name("readme"))], vec![file()]])
            ),
            [guide, main]
        );
        assert_eq!(
            paths_of(
                &idx,
                query_of(vec![
                    vec![term(true, Test::Extension(vec!["MD".to_string()]))],
                    vec![file()]
                ])
            ),
            [main]
        );

        // Every size and date in the fixture is unknown, so neither a date
        // test nor its negation matches.
        let any_date = Test::DateModified(DateRange {
            min: i64::MIN,
            max: i64::MAX,
        });
        assert!(paths_of(&idx, query_of(vec![vec![term(true, any_date)]])).is_empty());
        let big = || {
            Test::Size(SizeRange {
                min: 2,
                max: u64::MAX,
            })
        };
        assert!(paths_of(&idx, query_of(vec![vec![term(true, big())]])).is_empty());
        assert_eq!(
            paths_of(
                &idx,
                query_of(vec![vec![term(false, big()), term(false, name("guide"))]])
            ),
            [guide]
        );
    }

    /// Every term costs each candidate a test under the index read lock, so
    /// compiling refuses a query past the cap rather than holding the lock
    /// for minutes. It counts each `ext:` value, and a clause without terms
    /// as one, because a query from the pipe can hold any number of either.
    #[test]
    fn queries_through_links_keep_the_user_term_limit() {
        let text = |names: usize, exts: usize| {
            let exts: Vec<String> = (0..exts).map(|i| format!("e{i}")).collect();
            format!("{} ext:{}", vec!["a"; names].join(" "), exts.join(";"))
        };
        let compile = |query: &ParsedQuery| CompiledQuery::new(query).err();
        assert_eq!(compile(&parse_query(&text(200, 56)).unwrap()), None);
        let links = Links::new(vec![crate::links::FolderLink {
            link: r"C:\scope\alias".into(),
            target: r"D:\data".into(),
        }]);
        assert!(
            CompiledQuery::with_links(
                &parse_query(&text(200, 56)).unwrap(),
                &links,
                Some(r"C:\scope")
            )
            .is_ok()
        );
        assert_eq!(
            CompiledQuery::with_links(
                &parse_query(&text(200, 57)).unwrap(),
                &links,
                Some(r"C:\scope")
            )
            .err(),
            Some(QueryError::TooManyTerms(257))
        );
        assert_eq!(
            compile(&parse_query(&text(200, 57)).unwrap()),
            Some(QueryError::TooManyTerms(257))
        );
        assert_eq!(
            compile(&query_of(vec![Vec::new(); 257])),
            Some(QueryError::TooManyTerms(257))
        );
    }

    /// A regular expression arrives from any local user, so one that is
    /// malformed, compiles past the size limit, or comes with more than the
    /// cap allows is refused before the query runs. A pattern a person
    /// writes, with Unicode classes repeated, still fits.
    #[test]
    fn a_malformed_or_oversized_regex_is_refused() {
        let regexes = |patterns: &[&str]| {
            query_of(
                patterns
                    .iter()
                    .map(|p| vec![term(false, Test::NameRegex(p.to_string()))])
                    .collect(),
            )
        };
        let refusal = |patterns: &[&str]| CompiledQuery::new(&regexes(patterns)).err();
        for pattern in ["(", r"\w{1000}"] {
            assert!(
                matches!(
                    refusal(&[pattern]),
                    Some(QueryError::Malformed(Malformed::Regex(_)))
                ),
                "{pattern}"
            );
        }
        assert_eq!(refusal(&[r"\w{20}"]), None);
        assert_eq!(refusal(&["a"; MAX_QUERY_REGEXES]), None);
        assert_eq!(
            refusal(&["a"; MAX_QUERY_REGEXES + 1]),
            Some(QueryError::TooManyRegexes(MAX_QUERY_REGEXES + 1))
        );
    }

    mod untrusted {
        use super::*;
        use crate::proto::{Request, Returns, SearchRequest, decode_request};
        use proptest::prelude::*;

        fn arb_test() -> impl Strategy<Value = Test> {
            let text = || "[a-zA-Z.*?\\\\/ ]{0,6}";
            // Always valid: letters, `.` and escaped backslashes.
            let regex = || r"([a-c.]|\\\\){0,4}";
            prop_oneof![
                text().prop_map(Test::Name),
                text().prop_map(Test::Path),
                text().prop_map(Test::NameWildcard),
                text().prop_map(Test::PathWildcard),
                text().prop_map(Test::Parent),
                text().prop_map(Test::NameWholeWord),
                text().prop_map(Test::PathWholeWord),
                text().prop_map(Test::NameWordStart),
                text().prop_map(Test::PathWordStart),
                text().prop_map(Test::NameStart),
                text().prop_map(Test::PathStart),
                (prop::collection::vec(text(), 0..3), any::<bool>()).prop_map(
                    |(terms, match_path)| Test::Ordered {
                        terms,
                        term_match: TermMatch::WordStart,
                        match_path,
                    }
                ),
                regex().prop_map(Test::NameRegex),
                regex().prop_map(Test::PathRegex),
                prop::collection::vec(text(), 0..3).prop_map(Test::Extension),
                (any::<u64>(), any::<u64>())
                    .prop_map(|(min, max)| Test::Size(SizeRange { min, max })),
                prop_oneof![Just(Kind::File), Just(Kind::Folder)].prop_map(Test::Kind),
                (any::<i64>(), any::<i64>())
                    .prop_map(|(min, max)| Test::DateModified(DateRange { min, max })),
                "[rhsdavxntlcoieupm]{0,3}".prop_map(|letters| Test::Attributes(
                    Attributes::from_letters(&letters).expect("every letter is an attribute")
                )),
            ]
        }

        /// Up to 5 clauses of up to 4 terms, so every query is under the cap.
        fn arb_query() -> impl Strategy<Value = ParsedQuery> {
            let term =
                (any::<bool>(), arb_test()).prop_map(|(negated, test)| Term { negated, test });
            let clause = prop::collection::vec(term, 0..4).prop_map(|terms| Clause { terms });
            (prop::collection::vec(clause, 0..5), any::<bool>()).prop_map(
                |(clauses, match_case)| ParsedQuery {
                    clauses,
                    match_case,
                },
            )
        }

        proptest! {
            /// A query arrives from any local user, so the daemon runs every
            /// combination of clauses, terms, negations and scope; a panic
            /// would end the query. The totals count what a page counts.
            #[test]
            fn any_query_runs_and_its_totals_count_its_matches(
                query in arb_query(),
                scoped in any::<bool>(),
            ) {
                let idx = docs_index();
                let query = match CompiledQuery::new(&query) {
                    Err(QueryError::TooManyRegexes(_)) => return Ok(()),
                    compiled => compiled.expect("under the term cap, with valid regexes"),
                };
                let page = run_query(&idx, r"C:\", &query, scoped.then_some(r"C:\repo"), SortSpec::default(), None);
                let scope = scoped.then(|| resolve_scope(&idx, b"repo", false).expect("repo exists"));
                let totals = run_totals(&idx, r"C:\", &query, scope, &SeenBy::Everything).expect("no size is known");
                prop_assert_eq!(totals.count, page.total);
            }

            /// A search request arrives as JSON from the pipe, so any bytes
            /// can arrive; they must decode to a refusal or a query the
            /// daemon compiles and runs, never a panic.
            #[test]
            fn a_corrupted_search_request_errors_instead_of_panicking(
                query in arb_query(),
                patches in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
                cut in any::<prop::sample::Index>(),
            ) {
                let request = Request::Search(SearchRequest {
                    query,
                    scope: None,
                    returns: Returns::Totals,
                });
                let mut json = serde_json::to_vec(&request).expect("serializes");
                let len = json.len();
                for (at, byte) in &patches {
                    json[at.index(len)] = *byte;
                }
                json.truncate(cut.index(len + 1));
                if let Ok(Request::Search(search)) = decode_request(&json)
                    && let Ok(query) = CompiledQuery::new(&search.query)
                {
                    let _ = run_totals(&docs_index(), r"C:\", &query, None, &SeenBy::Everything);
                }
            }
        }
    }

    // -- result order -------------------------------------------------------

    fn add_file(
        idx: &mut VolumeIndex,
        f: Frn,
        links: &[(Frn, &str)],
        size: Option<u64>,
        mtime: Option<i64>,
    ) {
        use crate::layout::{LayoutFile, LayoutName};
        idx.enrich(&LayoutFile {
            frn: f,
            attributes: 0x20,
            size,
            mtime_ft: mtime,
            created_ft: None,
            security: None,
            names: links
                .iter()
                .map(|&(parent_frn, name)| LayoutName {
                    parent_frn,
                    name: name.as_bytes().to_vec(),
                    dos_only: false,
                })
                .collect(),
        });
    }

    /// Every way two hits can tie or nearly tie: es path order against a byte
    /// compare (`a~qx7zv` against `a\qx7zv`, `a` against `a b`), equal sizes and
    /// dates, an unknown size, names equal after folding, and one file with
    /// two links of the same name.
    fn order_index() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        let root = NTFS_ROOT_FRN;
        for r in [
            enum_rec(frn(100), root, "P", true),
            enum_rec(frn(101), frn(100), "a", true),
            enum_rec(frn(102), frn(100), "a b", true),
            enum_rec(frn(103), frn(101), "x", true),
        ] {
            idx.insert_enum(&r);
        }
        add_file(
            &mut idx,
            frn(200),
            &[(frn(100), "a~qx7zv")],
            Some(10),
            Some(5),
        );
        add_file(
            &mut idx,
            frn(201),
            &[(frn(101), "qx7zv")],
            Some(10),
            Some(5),
        );
        add_file(
            &mut idx,
            frn(202),
            &[(frn(102), "QX7ZV")],
            Some(30),
            Some(7),
        );
        add_file(&mut idx, frn(203), &[(frn(103), "qx7zv")], None, None);
        add_file(
            &mut idx,
            frn(204),
            &[(frn(101), "link.qx7zv"), (frn(102), "link.qx7zv")],
            Some(10),
            Some(5),
        );
        add_file(
            &mut idx,
            frn(205),
            &[(frn(102), "b.qx7zv")],
            Some(10),
            Some(5),
        );
        idx
    }

    fn all_sorts() -> Vec<SortSpec> {
        let mut sorts = Vec::new();
        for key in [
            SortKey::Name,
            SortKey::Path,
            SortKey::Size,
            SortKey::DateModified,
            SortKey::Extension,
        ] {
            for dir in [SortDir::Asc, SortDir::Desc] {
                sorts.push(SortSpec { key, dir });
            }
        }
        sorts
    }

    /// The full path of every row of `out`, in its order.
    fn texts(out: &QueryOutcome) -> Vec<String> {
        out.rows
            .iter()
            .map(|row| crate::wtf8::to_string_lossy(&row.path))
            .collect()
    }

    /// The rows of `answers` on the volumes of `indexes`, read as pages of
    /// `page_rows`, with `between` run on the indexes before each page after
    /// the first, and the total the last page reports.
    fn read_pages(
        reply: &mut Reply,
        indexes: &mut [&mut VolumeIndex],
        page_rows: usize,
        mut between: impl FnMut(usize, &mut [&mut VolumeIndex]),
    ) -> (Vec<String>, u64) {
        let mut paths = Vec::new();
        for page in 0.. {
            if page > 0 {
                between(page, indexes);
            }
            let held: Vec<Option<&VolumeIndex>> = indexes
                .iter()
                .enumerate()
                .map(|(v, i)| reply.reads_volume(v).then_some(&**i))
                .collect();
            let mut taken = 0;
            let more = reply.fill(&held, usize::MAX, |row, names| {
                if taken == page_rows {
                    return false;
                }
                taken += 1;
                let mut path = Vec::new();
                names.full_path_into(row.folder, row.name, &mut path);
                paths.push(crate::wtf8::to_string_lossy(&path));
                true
            });
            if !more {
                break;
            }
        }
        (paths, reply.total())
    }

    /// The rows of one volume's `answer`, read as one page.
    fn read_all(answer: VolumeRows, index: &mut VolumeIndex) -> (Vec<String>, u64) {
        let mut reply = Reply::new(vec![(answer, SeenBy::Everything)], 0, None);
        read_pages(&mut reply, &mut [index], usize::MAX, |_, _| {})
    }

    fn delete(f: Frn) -> ParsedUsnRecord {
        ParsedUsnRecord {
            reason: USN_REASON_FILE_DELETE,
            ..enum_rec(f, frn(100), "", false)
        }
    }

    fn rename(f: Frn, parent: Frn, name: &str, is_dir: bool) -> ParsedUsnRecord {
        ParsedUsnRecord {
            reason: USN_REASON_RENAME_NEW_NAME,
            ..enum_rec(f, parent, name, is_dir)
        }
    }

    /// `-sort path` orders by parent folder, then name, as
    /// [`SortKey::Path`] documents. `P\a~qx7zv` sits in `P`, so it comes before
    /// everything in `P\a`, though `\` sorts before `~` in a byte compare of
    /// the whole path.
    #[test]
    fn path_order_is_parent_folder_then_name() {
        let idx = order_index();
        let parsed = compiled("qx7zv");
        let out = run_query(
            &idx,
            r"C:\",
            &parsed,
            None,
            SortSpec::default_for(SortKey::Path),
            None,
        );
        assert_eq!(
            texts(&out),
            [
                r"C:\P\a~qx7zv",
                r"C:\P\a\link.qx7zv",
                r"C:\P\a\qx7zv",
                r"C:\P\a b\b.qx7zv",
                r"C:\P\a b\link.qx7zv",
                r"C:\P\a b\QX7ZV",
                r"C:\P\a\x\qx7zv",
            ]
        );
    }

    /// An unknown size or date sorts last whichever way the key runs.
    #[test]
    fn unknown_sizes_and_dates_sort_last() {
        let idx = order_index();
        let parsed = compiled("qx7zv");
        for sort in all_sorts() {
            if matches!(sort.key, SortKey::Size | SortKey::DateModified) {
                let out = run_query(&idx, r"C:\", &parsed, None, sort, None);
                assert_eq!(texts(&out).last().unwrap(), r"C:\P\a\x\qx7zv", "{sort:?}");
            }
        }
    }

    /// A limited query is a prefix of the same query without a limit, under
    /// every sort, and the name ranks change no order: without them every
    /// name key is equal, so [`compare`] alone orders each run of equal
    /// primary keys.
    #[test]
    fn a_limited_query_is_a_prefix_of_the_unlimited_one() {
        let unranked = order_index();
        let mut ranked = order_index();
        ranked.rank_names();
        for query in ["qx7zv", ""] {
            let parsed = compiled(query);
            for sort in all_sorts() {
                let unlimited = texts(&run_query(&ranked, r"C:\", &parsed, None, sort, None));
                assert_eq!(
                    texts(&run_query(&unranked, r"C:\", &parsed, None, sort, None)),
                    unlimited,
                    "{query:?} {sort:?}: the ranks changed the order"
                );
                for k in 0..=unlimited.len() {
                    let page = run_query(&ranked, r"C:\", &parsed, None, sort, Some(k as u32));
                    assert_eq!(texts(&page), unlimited[..k], "{query:?} {sort:?} -n {k}");
                }
            }
        }
    }

    /// A limited list stays the start of the full one when a run of equal
    /// keys crosses its end: the files share one name, size and date, and
    /// their file IDs fall as their entries rise, so only [`compare`] puts
    /// them in order, and the quickselect leaves part of the run on each side
    /// of the cut.
    #[test]
    fn a_tie_run_across_the_limit_keeps_a_limited_list_the_start_of_the_full_one() {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        for (i, name) in ["d", "e", "f"].iter().enumerate() {
            idx.insert_enum(&enum_rec(frn(100 + i as u64), NTFS_ROOT_FRN, name, true));
        }
        for i in 0..24u64 {
            let file = frn(900 - i);
            let name = if i % 3 == 0 { "SAME.txt" } else { "same.txt" };
            idx.insert_enum(&enum_rec(file, frn(100 + i % 3), name, false));
            idx.set_stat(file, 7, 70);
        }
        idx.insert_enum(&enum_rec(frn(950), frn(100), "other.txt", false));
        idx.rank_names();
        let parsed = compiled("txt");
        for sort in all_sorts() {
            let unlimited = texts(&run_query(&idx, r"C:\", &parsed, None, sort, None));
            assert_eq!(unlimited.len(), 25);
            for k in 0..=unlimited.len() {
                let page = run_query(&idx, r"C:\", &parsed, None, sort, Some(k as u32));
                assert_eq!(texts(&page), unlimited[..k], "{sort:?} -n {k}");
            }
        }
    }

    /// A folder inside another matching folder adds to the count but not to
    /// the folders' bytes, because its size is already in its parent's; the
    /// status bar would otherwise show `docs` and `docs\sub` as 8,392
    /// bytes. A file with no size is counted apart, and after a size pass
    /// that stopped the folders' bytes are only a lower bound.
    #[test]
    fn a_nested_folder_adds_its_count_but_not_its_size() {
        let mut idx = sample_index();
        idx.insert_enum(&enum_rec(frn(105), frn(101), "unread.bin", false));
        idx.compute_folder_sizes();
        let parsed = compiled("");
        let sums = |idx: &VolumeIndex| {
            find_matches(
                idx,
                "",
                &parsed,
                None,
                SortSpec::default(),
                None,
                true,
                &SeenBy::Everything,
            )
            .origin
            .sums
            .expect("the search asked for sums")
        };
        assert_eq!(
            sums(&idx),
            MatchSums {
                files: 4,
                file_bytes: 4096 + 200 + 2 * 1024 * 1024,
                files_without_size: 1,
                folders: 2,
                folder_bytes: 4096 + 200,
                folder_bytes_at_least: false,
            }
        );
        idx.record_size_pass_stopped();
        assert!(sums(&idx).folder_bytes_at_least);
    }

    /// Matches whose index got new entry indexes between the two locks, as
    /// `optimize_storage` gives them once a removed entry left a slot, make
    /// no rows, because their records would name other entries. An
    /// `optimize_storage` with no free slot moves no entry, so it keeps a
    /// search's matches.
    #[test]
    fn matches_from_an_index_that_changed_make_no_rows() {
        let mut idx = sample_index();
        let parsed = compiled("");
        let matches = |idx: &VolumeIndex| {
            find_matches(
                idx,
                "",
                &parsed,
                None,
                SortSpec::default(),
                None,
                false,
                &SeenBy::Everything,
            )
            .order()
        };
        let kept = matches(&idx);
        idx.optimize_storage();
        assert!(
            kept.read_rows(&idx, &SeenBy::Everything).is_ok(),
            "no entry moved"
        );
        let ordered = matches(&idx);
        idx.apply_batch(&[delete(frn(103))]);
        idx.optimize_storage();
        assert_eq!(
            ordered.read_rows(&idx, &SeenBy::Everything).err(),
            Some(IndexChanged)
        );
    }

    /// An entry removed between the two locks is left out, and out of the
    /// total, so `gozler` does not report the answer as cut short; the tie
    /// runs never read its slot, which names no name. The rows are then fewer
    /// than the limit.
    #[test]
    fn an_entry_removed_between_the_locks_is_left_out_of_the_rows_and_the_total() {
        let mut idx = sample_index();
        let parsed = compiled("");
        let sort = SortSpec::default_for(SortKey::Size);
        let ordered = find_matches(
            &idx,
            "",
            &parsed,
            None,
            sort,
            Some(3),
            false,
            &SeenBy::Everything,
        )
        .order();
        idx.apply_batch(&[delete(frn(103))]);
        let answer = ordered
            .read_rows(&idx, &SeenBy::Everything)
            .expect("the index kept its entry indexes");
        let (paths, total) = read_all(answer, &mut idx);
        assert_eq!(paths, ["photo.JPG", "docs\\sub\\Report.PDF"]);
        assert_eq!(total, 4);
    }

    /// A slot that a new file took over between the two locks names a match
    /// that died, so that row is left out and counted out of the total,
    /// instead of showing the new file, which the search never matched.
    #[test]
    fn a_slot_reused_between_the_locks_is_left_out() {
        let mut idx = sample_index();
        let parsed = compiled("");
        let sort = SortSpec::default();
        let ordered = find_matches(
            &idx,
            "",
            &parsed,
            None,
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order();
        let notes = idx.head_of(frn(103));
        idx.apply_batch(&[delete(frn(103))]);
        idx.apply_batch(&[enum_rec(frn(300), NTFS_ROOT_FRN, "new.txt", false)]);
        assert_eq!(idx.head_of(frn(300)), notes, "the new file took the slot");
        let answer = ordered
            .read_rows(&idx, &SeenBy::Everything)
            .expect("one reuse is kept");
        let (paths, total) = read_all(answer, &mut idx);
        assert_eq!(
            paths,
            ["docs", "photo.JPG", "docs\\sub\\Report.PDF", "docs\\sub"]
        );
        assert_eq!(total, 4);
    }

    /// Past the reused slots the index keeps a note of, the second lock
    /// cannot tell which records name dead entries, so it makes no rows.
    #[test]
    fn more_reused_slots_than_the_index_keeps_make_no_rows() {
        let mut idx = sample_index();
        let parsed = compiled("");
        let ordered = find_matches(
            &idx,
            "",
            &parsed,
            None,
            SortSpec::default(),
            None,
            false,
            &SeenBy::Everything,
        )
        .order();
        let mut last = frn(103);
        for i in 0..=crate::index::store::REUSED_SLOTS_KEPT as u64 {
            let next = frn(1000 + i);
            idx.apply_batch(&[delete(last)]);
            idx.apply_batch(&[enum_rec(next, NTFS_ROOT_FRN, "churn.tmp", false)]);
            last = next;
        }
        assert_eq!(
            ordered.read_rows(&idx, &SeenBy::Everything).err(),
            Some(IndexChanged)
        );
    }

    /// `docs` with `a.txt`, `b.txt`, `c.txt` and `sub\d.txt`.
    fn docs_tree() -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        for r in [
            enum_rec(frn(100), NTFS_ROOT_FRN, "docs", true),
            enum_rec(frn(101), frn(100), "a.txt", false),
            enum_rec(frn(102), frn(100), "b.txt", false),
            enum_rec(frn(103), frn(100), "c.txt", false),
            enum_rec(frn(104), frn(100), "sub", true),
            enum_rec(frn(105), frn(104), "d.txt", false),
        ] {
            idx.insert_enum(&r);
        }
        idx
    }

    /// A row is the file when its page is read: a row deleted, or moved into
    /// a folder with no full path (one under lost+found, first copied when
    /// the page is read), after the rows were made is left out, and the total
    /// counts each out, so `gozler` does not report the answer as cut short.
    #[test]
    fn rows_deleted_or_without_a_path_at_their_page_are_left_out() {
        let mut idx = docs_tree();
        let parsed = compiled("");
        let sort = SortSpec::default();
        let answer = find_matches(
            &idx,
            "",
            &parsed,
            None,
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        idx.apply_batch(&[
            delete(frn(101)),
            // A folder whose parent the index does not know sits under
            // lost+found.
            enum_rec(frn(500), frn(999), "orphan", true),
            rename(frn(103), frn(500), "c.txt", false),
        ]);
        let (paths, total) = read_all(answer, &mut idx);
        assert_eq!(
            paths,
            [r"docs\b.txt", r"docs\sub\d.txt", "docs", r"docs\sub"]
        );
        assert_eq!(total, 4);
    }

    /// A row moved out of the search's scope after the rows were made is left
    /// out, and the total counts it out.
    #[test]
    fn a_row_that_left_the_scope_before_its_page_is_left_out() {
        let mut idx = docs_tree();
        let parsed = compiled("");
        let sort = SortSpec::default();
        let docs = resolve_scope(&idx, b"docs", false).expect("docs exists");
        let answer = find_matches(
            &idx,
            "",
            &parsed,
            Some(docs),
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        idx.apply_batch(&[rename(frn(102), NTFS_ROOT_FRN, "b.txt", false)]);
        let (paths, total) = read_all(answer, &mut idx);
        assert_eq!(
            paths,
            [r"docs\a.txt", r"docs\c.txt", r"docs\sub\d.txt", r"docs\sub"]
        );
        assert_eq!(total, 4);
    }

    /// A row shows the name its file has when the page is read, as
    /// Everything's list shows a renamed file.
    #[test]
    fn a_row_renamed_before_its_page_shows_its_new_name() {
        let mut idx = sample_index();
        let parsed = compiled("photo");
        let sort = SortSpec::default();
        let answer = find_matches(
            &idx,
            "",
            &parsed,
            None,
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        idx.apply_batch(&[rename(frn(104), NTFS_ROOT_FRN, "holiday.JPG", false)]);
        let (paths, total) = read_all(answer, &mut idx);
        assert_eq!(paths, ["holiday.JPG"]);
        assert_eq!(total, 1);
    }

    /// A folder whose slot another folder took over between two pages gives
    /// a row of the new folder its own path, not the path the first page
    /// copied for the slot.
    #[test]
    fn a_reused_folder_slot_gives_no_row_another_folders_path() {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        for r in [
            enum_rec(frn(100), NTFS_ROOT_FRN, "A", true),
            enum_rec(frn(101), frn(100), "f1.txt", false),
            enum_rec(frn(102), frn(100), "f2.txt", false),
        ] {
            idx.insert_enum(&r);
        }
        let parsed = compiled("txt");
        let sort = SortSpec::default();
        let answer = find_matches(
            &idx,
            r"C:\",
            &parsed,
            None,
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        let a = idx.head_of(frn(100));
        let mut reply = Reply::new(vec![(answer, SeenBy::Everything)], 0, None);
        let (paths, total) = read_pages(&mut reply, &mut [&mut idx], 1, |_, indexes| {
            let idx = &mut *indexes[0];
            idx.apply_batch(&[
                rename(frn(102), NTFS_ROOT_FRN, "f2.txt", false),
                delete(frn(101)),
                delete(frn(100)),
                enum_rec(frn(200), NTFS_ROOT_FRN, "B", true),
                rename(frn(102), frn(200), "f2.txt", false),
            ]);
            assert_eq!(idx.head_of(frn(200)), a, "B took A's slot");
        });
        assert_eq!(paths, [r"C:\A\f1.txt", r"C:\B\f2.txt"]);
        assert_eq!(total, 2);
    }

    /// A folder with no full path on one page sends its rows on a later page
    /// once it has one, as when the journal names the parent of a folder
    /// under lost+found: `sub` moves under a folder the index does not know
    /// before the first page, so `a1.txt` is left out, and that folder is
    /// created before the second page, so `d1.txt` is sent.
    #[test]
    fn a_folder_that_gets_a_path_again_sends_its_later_rows() {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        for r in [
            enum_rec(frn(100), NTFS_ROOT_FRN, "sub", true),
            enum_rec(frn(101), frn(100), "a1.txt", false),
            enum_rec(frn(102), NTFS_ROOT_FRN, "b.txt", false),
            enum_rec(frn(103), NTFS_ROOT_FRN, "c.txt", false),
            enum_rec(frn(104), frn(100), "d1.txt", false),
        ] {
            idx.insert_enum(&r);
        }
        let parsed = compiled("txt");
        let sort = SortSpec::default();
        let answer = find_matches(
            &idx,
            r"C:\",
            &parsed,
            None,
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        idx.apply_batch(&[rename(frn(100), frn(999), "sub", true)]);
        let mut reply = Reply::new(vec![(answer, SeenBy::Everything)], 0, None);
        let (paths, total) = read_pages(&mut reply, &mut [&mut idx], 1, |_, indexes| {
            indexes[0].apply_batch(&[enum_rec(frn(999), NTFS_ROOT_FRN, "found", true)]);
        });
        assert_eq!(paths, [r"C:\b.txt", r"C:\c.txt", r"C:\found\sub\d1.txt"]);
        assert_eq!(total, 3);
    }

    /// Rows whose index got new entry indexes after they were made are not
    /// sent, the total counts them out, and the reply says its index
    /// changed, which the daemon reports as an incomplete answer.
    #[test]
    fn rows_of_an_index_that_changed_after_they_were_made_are_not_sent() {
        let mut idx = sample_index();
        let parsed = compiled("");
        let sort = SortSpec::default();
        let answer = find_matches(
            &idx,
            "",
            &parsed,
            None,
            sort,
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        idx.apply_batch(&[delete(frn(103))]);
        idx.optimize_storage();
        let mut reply = Reply::new(vec![(answer, SeenBy::Everything)], 0, None);
        let (paths, total) = read_pages(&mut reply, &mut [&mut idx], usize::MAX, |_, _| {});
        assert!(paths.is_empty());
        assert_eq!(total, 0);
        assert!(reply.index_changed());
    }

    /// The same holds across volumes. Each volume picks its own top `k`, and
    /// the reply merges them; the result equals the first `k` of every
    /// match merged. The two prefixes interleave in path order (`C:\mnt\P`
    /// sorts between `C:` and `C:\P`), and the identical fixtures make every
    /// name tie across volumes.
    #[test]
    fn a_limited_query_is_a_prefix_across_volumes() {
        let mut volumes = [(r"C:\", order_index()), (r"C:\mnt\", order_index())];
        let parsed = compiled("qx7zv");
        for sort in all_sorts() {
            let mut merged = |limit: Option<u32>| {
                let answers = volumes
                    .iter()
                    .map(|(prefix, idx)| {
                        let answer = find_matches(
                            idx,
                            prefix,
                            &parsed,
                            None,
                            sort,
                            limit,
                            false,
                            &SeenBy::Everything,
                        )
                        .order()
                        .read_rows(idx, &SeenBy::Everything)
                        .expect("nothing changed");
                        (answer, SeenBy::Everything)
                    })
                    .collect();
                let mut reply = Reply::new(answers, 0, limit);
                let mut indexes: Vec<&mut VolumeIndex> =
                    volumes.iter_mut().map(|(_, idx)| idx).collect();
                read_pages(&mut reply, &mut indexes, usize::MAX, |_, _| {}).0
            };
            let unlimited = merged(None);
            assert_eq!(unlimited.len(), 14);
            for k in 0..=unlimited.len() {
                assert_eq!(merged(Some(k as u32)), unlimited[..k], "{sort:?} -n {k}");
            }
        }
    }

    /// A limited search over several volumes ends short, rather than send
    /// another volume's later row first, when a volume whose limit cut off
    /// matches runs out of rows because one was left out. With `-n 3`, `C:`
    /// keeps `a c d` of `a c d e`, and `c` is deleted before the first page:
    /// sending `D:\f.txt` next would skip `e`, and the window's next chunk
    /// would show `f` again. Both a merge by comparison and volumes that
    /// follow each other end there.
    #[test]
    fn a_volume_out_of_rows_before_its_limit_ends_the_reply() {
        let files = |names: &[&str]| {
            let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
            for (i, name) in names.iter().enumerate() {
                idx.insert_enum(&enum_rec(frn(100 + i as u64), NTFS_ROOT_FRN, name, false));
            }
            idx
        };
        let parsed = compiled("txt");
        for sort in [SortSpec::default(), SortSpec::default_for(SortKey::Path)] {
            let mut c = files(&["a.txt", "c.txt", "d.txt", "e.txt"]);
            let mut d = files(&["f.txt"]);
            let answers: Vec<(VolumeRows, SeenBy)> = [(&c, r"C:\"), (&d, r"D:\")]
                .into_iter()
                .map(|(idx, prefix)| {
                    let answer = find_matches(
                        idx,
                        prefix,
                        &parsed,
                        None,
                        sort,
                        Some(3),
                        false,
                        &SeenBy::Everything,
                    )
                    .order()
                    .read_rows(idx, &SeenBy::Everything)
                    .expect("nothing changed");
                    (answer, SeenBy::Everything)
                })
                .collect();
            c.apply_batch(&[delete(frn(101))]);
            let mut reply = Reply::new(answers, 0, Some(3));
            let (paths, total) =
                read_pages(&mut reply, &mut [&mut c, &mut d], usize::MAX, |_, _| {});
            assert_eq!(paths, [r"C:\a.txt", r"C:\d.txt"], "{sort:?}");
            assert_eq!(total, 4, "{sort:?}");
        }
    }

    /// A row skipped for the offset counts against a page's rows, so an
    /// offset far into the matches, such as a pipe client's `u32::MAX`, is
    /// skipped a page at a time instead of under one hold of the lock: each
    /// page skips at most its rows, and goes out empty with more to come.
    #[test]
    fn an_offset_is_skipped_a_page_of_rows_at_a_time() {
        let idx = docs_tree();
        let parsed = compiled("txt");
        let answer = find_matches(
            &idx,
            "",
            &parsed,
            None,
            SortSpec::default(),
            None,
            false,
            &SeenBy::Everything,
        )
        .order()
        .read_rows(&idx, &SeenBy::Everything)
        .expect("nothing changed");
        let mut reply = Reply::new(vec![(answer, SeenBy::Everything)], 3, None);
        let mut pages: Vec<Vec<String>> = Vec::new();
        loop {
            let mut page = Vec::new();
            let more = reply.fill(&[Some(&idx)], 2, |row, _| {
                page.push(crate::wtf8::to_string_lossy(row.name));
                true
            });
            pages.push(page);
            if !more {
                break;
            }
        }
        assert_eq!(pages, [vec![], vec!["d.txt".to_string()]]);
    }

    /// A file of [`packed_keys_agree_with_the_result_order`]: its name, which
    /// of three folders holds it, its size and its date.
    type KeyFile = (String, u64, Option<u64>, Option<i64>);

    fn key_file() -> impl Strategy<Value = KeyFile> {
        (
            "[aAb.~ ]{1,3}",
            0u64..3,
            prop_oneof![
                Just(None),
                Just(Some(0)),
                Just(Some(u64::MAX - 1)),
                (0u64..3).prop_map(Some),
            ],
            prop_oneof![
                Just(None),
                Just(Some(i64::MIN + 1)),
                Just(Some(i64::MAX)),
                (-1i64..2).prop_map(Some),
            ],
        )
    }

    proptest! {
        /// When one match's packed keys are smaller than another's, the
        /// result order puts it first, under every sort and in both
        /// directions, or the sort prints rows out of that order. Names are
        /// ranked, then more are added, some of them new spellings of ranked
        /// names, and some are released; sizes and dates include 0, unknown
        /// and their extremes. Names with a `.` give extensions, and the
        /// folders give the path sort several folder ranks.
        #[test]
        fn packed_keys_agree_with_the_result_order(
            ranked in prop::collection::vec(key_file(), 1..16),
            added in prop::collection::vec(key_file(), 0..8),
            removed in prop::collection::vec(any::<prop::sample::Index>(), 0..4),
        ) {
            let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
            for (i, name) in ["d", "D e", "~"].iter().enumerate() {
                idx.insert_enum(&enum_rec(frn(100 + i as u64), NTFS_ROOT_FRN, name, true));
            }
            let mut files = Vec::new();
            let mut add = |idx: &mut VolumeIndex, batch: &[KeyFile]| {
                for (name, folder, size, mtime) in batch {
                    let file = frn(1000 + files.len() as u64);
                    idx.insert_enum(&enum_rec(file, frn(100 + folder), name, false));
                    idx.set_stat(file, size.unwrap_or(u64::MAX), mtime.unwrap_or(i64::MIN));
                    files.push(file);
                }
            };
            add(&mut idx, &ranked);
            idx.rank_names();
            add(&mut idx, &added);
            let deletes: Vec<ParsedUsnRecord> = removed
                .iter()
                .map(|at| ParsedUsnRecord {
                    reason: crate::usn::record::USN_REASON_FILE_DELETE,
                    ..enum_rec(files[at.index(files.len())], NTFS_ROOT_FRN, "", false)
                })
                .collect();
            idx.apply_batch(&deletes);

            let parsed = compiled("");
            for sort in all_sorts() {
                let ordered = find_matches(&idx, r"C:\", &parsed, None, sort, None, false, &SeenBy::Everything).order();
                let everything = SeenBy::Everything;
                let ranked = |record| Ranked { index: &idx, seen_by: &everything, sort, record };
                for &a in &ordered.records {
                    for &b in &ordered.records {
                        if a.keys() < b.keys() {
                            prop_assert_eq!(
                                compare(sort, &ranked(a), &ranked(b)),
                                core::cmp::Ordering::Less,
                                "{:?}: {:?} against {:?}",
                                sort,
                                idx.entry(a.idx()).name(),
                                idx.entry(b.idx()).name(),
                            );
                        }
                    }
                }
            }
        }
    }

    // -- oracle property test --------------------------------------------

    use proptest::prelude::*;

    /// A flat index of random-named files at the root.
    fn flat_index(names: &[String]) -> VolumeIndex {
        let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
        for (i, n) in names.iter().enumerate() {
            idx.insert_enum(&enum_rec(frn(1000 + i as u64), NTFS_ROOT_FRN, n, false));
        }
        idx
    }

    /// Naive oracle: a file matches iff its folded name contains every folded
    /// term as a substring.
    fn oracle(names: &[String], terms: &[String]) -> BTreeSet<String> {
        names
            .iter()
            .filter(|n| {
                let fn_ = crate::fold::fold(n.as_bytes());
                terms.iter().all(|t| {
                    let ft = crate::fold::fold(t.as_bytes());
                    fn_.windows(ft.len().max(1)).any(|w| w == ft.as_slice()) || ft.is_empty()
                })
            })
            .cloned()
            .collect()
    }

    use std::collections::BTreeSet;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(200))]

        /// The engine's name-term matching agrees with the naive oracle over
        /// random corpora and needles (mixed case, unicode letters).
        #[test]
        fn matcher_agrees_with_oracle(
            names in prop::collection::vec("[a-zA-Zçğ0-9._]{1,10}", 1..30),
            terms in prop::collection::vec("[a-zA-Zçğ]{1,4}", 0..3),
        ) {
            // Names must be unique so a path set is a faithful comparison.
            let mut uniq: Vec<String> = Vec::new();
            for (i, n) in names.iter().enumerate() {
                uniq.push(format!("{n}_{i}"));
            }
            let idx = flat_index(&uniq);
            let query = terms.join(" ");
            let parsed = compiled(&query);
            let out = run_query(&idx, "", &parsed, None, SortSpec::default(), None);
            let engine: BTreeSet<String> = texts(&out).into_iter().collect();
            prop_assert_eq!(engine, oracle(&uniq, &terms));
        }

        /// A path term finds exactly the entries whose full path, built by
        /// walking to the root, contains it: over random trees that include
        /// orphans, which have no path.
        #[test]
        fn path_terms_agree_with_a_naive_full_path_search(
            parents in prop::collection::vec(0usize..14, 1..40),
            names in prop::collection::vec("[a-cA-C]{1,3}", 40),
            before in "[a-cA-C]{0,2}",
            after in "[a-cA-C]{0,2}",
        ) {
            let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
            for (i, &p) in parents.iter().enumerate() {
                let parent = match p {
                    p if p < i => frn(1000 + p as u64),
                    12 => frn(999), // never inserted: an orphan's parent
                    _ => NTFS_ROOT_FRN,
                };
                idx.insert_enum(&enum_rec(frn(1000 + i as u64), parent, &names[i], i % 3 != 0));
            }
            let term = format!("{before}\\{after}");
            let parsed = compiled(&term);
            let out = run_query(&idx, r"C:\", &parsed, None, SortSpec::default(), None);
            let engine: BTreeSet<String> = texts(&out).into_iter().collect();

            let needle = crate::fold::fold(term.as_bytes());
            let mut naive = BTreeSet::new();
            for e in idx.live_entries().filter(|&e| !idx.is_synthetic(e)) {
                let mut rel = Vec::new();
                if idx.path_of(e, &mut rel) != crate::index::PathStatus::Ok {
                    continue;
                }
                let full = [b"C:\\".as_slice(), &rel].concat();
                let folded = crate::fold::fold(&full);
                if folded.windows(needle.len()).any(|w| w == needle.as_slice()) {
                    naive.insert(crate::wtf8::to_string_lossy(&full));
                }
            }
            prop_assert_eq!(engine, naive);
        }

        /// With -p, terms without a separator, negated or not, of AND and OR
        /// clauses, in each match mode, and wildcards, which one name decides
        /// only as `*a` or `*a*`, find exactly the entries whose full path,
        /// built by walking to the root, passes them. The root's name `C:`
        /// holds `c` and starts every path, a root mounted nowhere leaves the
        /// path to start with the name below it, folders hold the matches
        /// below them, and orphans have no path, so a component walk or a
        /// test of the components that misses any of them fails.
        #[test]
        fn match_path_terms_agree_with_a_test_of_the_full_path(
            parents in prop::collection::vec(0usize..14, 1..80),
            names in prop::collection::vec("[a-cA-C ]{1,3}", 80),
            query in "!?[a-cA-C*?]{1,3}([ |]!?[a-cA-C*?]{1,3}){0,2}",
            term_match in prop::sample::select(&[
                TermMatch::Anywhere,
                TermMatch::WholeWords,
                TermMatch::WordStart,
                TermMatch::NameStart,
            ][..]),
            match_case: bool,
            mount_prefix in prop::sample::select(&[r"C:\", ""][..]),
        ) {
            let mut idx = VolumeIndex::new(NTFS_ROOT_FRN);
            for (i, &p) in parents.iter().enumerate() {
                let parent = match p {
                    p if p < i => frn(1000 + p as u64),
                    12 => frn(999), // never inserted: an orphan's parent
                    _ => NTFS_ROOT_FRN,
                };
                idx.insert_enum(&enum_rec(frn(1000 + i as u64), parent, &names[i], i % 3 != 0));
            }
            let options = ParseOptions {
                match_case,
                match_path: true,
                term_match,
                ..ParseOptions::default()
            };
            let parsed = parse_query_with(&query, options).unwrap();
            let compiled = CompiledQuery::new(&parsed).unwrap();
            let out = run_query(&idx, mount_prefix, &compiled, None, SortSpec::default(), None);
            let engine: BTreeSet<String> = texts(&out).into_iter().collect();

            let mut code_points = CodePointBuf::new();
            let mut naive = BTreeSet::new();
            for e in idx.live_entries().filter(|&e| !idx.is_synthetic(e)) {
                let mut rel = Vec::new();
                if idx.path_of(e, &mut rel) != crate::index::PathStatus::Ok {
                    continue;
                }
                let full = [mount_prefix.as_bytes(), &rel].concat();
                let text = if match_case { full.clone() } else { crate::fold::fold(&full) };
                let passes = parsed.clauses.iter().all(|clause| {
                    clause.terms.iter().any(|term| {
                        let Ok(Check::Text(Target::Path, matcher)) = Check::new(&term.test, match_case)
                        else {
                            panic!("-p makes every term a path text term");
                        };
                        matcher.matches(&text, &mut code_points) != term.negated
                    })
                });
                if passes {
                    naive.insert(crate::wtf8::to_string_lossy(&full));
                }
            }
            prop_assert_eq!(engine, naive);
        }
    }
}
