//! Finite matcher states at folder-link targets. A state records what a term
//! still needs, so cycles reach a fixed point without alternate path strings
//! or a cutoff that loses results.

use super::engine::{Matcher, is_word};
use super::wildcard::CodePointBuf;
use crate::fold::fold;
use crate::links::Links;
use crate::wtf8::{CodePoints, encode_code_point};
use rustc_hash::{FxHashMap, FxHashSet};
use std::ops::Range;

pub(super) struct Target {
    pub(super) raw: String,
}

pub(super) struct LinkPaths {
    pub(super) targets: Vec<Target>,
    seeds: Vec<(usize, Vec<u8>)>,
    edges: Vec<Vec<(usize, Vec<u8>)>>,
}

impl LinkPaths {
    pub(super) fn new(links: &Links, match_case: bool) -> Self {
        let form = |s: &str| {
            if match_case {
                s.as_bytes().to_vec()
            } else {
                fold(s.as_bytes())
            }
        };
        let mut targets = Vec::new();
        let mut numbers = FxHashMap::default();
        let mut seeds = Vec::new();
        for link in links.as_slice() {
            let path = fold(link.target.as_bytes());
            let next = targets.len();
            let number = *numbers.entry(path.clone()).or_insert_with(|| {
                targets.push(Target {
                    raw: link.target.clone(),
                });
                next
            });
            seeds.push((number, form(&link.link)));
        }
        let mut edges = vec![Vec::new(); targets.len()];
        // Descent between nested targets carries the ancestor's states too,
        // allowing the scan to consult only its nearest target.
        for (number, target) in targets.iter().enumerate() {
            let path = form(&target.raw);
            if let Some((above, at)) = nearest(&numbers, &path, false) {
                edges[above].push((number, path[at..].to_vec()));
            }
        }
        for (number, path) in &seeds {
            if let Some((above, at)) = nearest(&numbers, path, true) {
                edges[above].push((*number, path[at..].to_vec()));
            }
        }
        Self {
            targets,
            seeds,
            edges,
        }
    }

    pub(super) fn compile(&self, matcher: &Matcher) -> Through {
        let mut states = vec![FxHashSet::default(); self.targets.len()];
        let mut queue = Vec::new();
        let mut add = |target: usize, mut state, queue: &mut Vec<_>| {
            if states[target].contains(&State::Matched) {
                return;
            }
            if let State::Position { at, .. } = state {
                let complete = match matcher {
                    Matcher::WholeWord(finder) => at == finder.needle().len(),
                    Matcher::Wildcard(wildcard) => wildcard.accepts_any_remainder(at),
                    _ => false,
                };
                if complete {
                    state = State::Matched;
                }
            }
            if state == State::Matched {
                states[target].clear();
            }
            if states[target].insert(state) {
                queue.push((target, state));
            }
        };
        for (target, path) in &self.seeds {
            for state in advance(matcher, State::Position { at: 0, word: false }, path) {
                add(*target, state, &mut queue);
            }
        }
        while let Some((target, state)) = queue.pop() {
            for (next, rest) in &self.edges[target] {
                for state in advance(matcher, state, rest) {
                    add(*next, state, &mut queue);
                }
            }
        }
        Through {
            states: states
                .into_iter()
                .map(|s| {
                    s.into_iter()
                        .filter(|state| match state {
                            State::Matched => true,
                            State::Position { at, word } => match matcher.literal() {
                                Some(text) => {
                                    (*at > 0 || text.first() == Some(&b'\\'))
                                        && !(*at == 0
                                            && *word
                                            && matches!(
                                                matcher,
                                                Matcher::WholeWord(_) | Matcher::WordStart(_)
                                            ))
                                }
                                None => true,
                            },
                        })
                        .collect()
                })
                .collect(),
        }
    }
}

pub(super) fn nearest(
    numbers: &FxHashMap<Vec<u8>, usize>,
    path: &[u8],
    include_self: bool,
) -> Option<(usize, usize)> {
    let compared = fold(path);
    if include_self && let Some(&number) = numbers.get(&compared) {
        return Some((number, path.len()));
    }
    let mut ends = path
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, b)| **b == b'\\')
        .map(|(at, _)| at);
    compared
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, b)| **b == b'\\')
        .find_map(|(at, _)| {
            let raw = ends.next().expect("folding preserves separators");
            numbers.get(&compared[..at]).map(|&number| (number, raw))
        })
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum State {
    Matched,
    // At position zero, `word` decides whether the next character starts a
    // whole word, including a term whose first character is a separator.
    Position { at: usize, word: bool },
}

pub(super) struct Through {
    states: Vec<Vec<State>>,
}

impl Through {
    pub(super) fn passes(
        &self,
        target: usize,
        matcher: &Matcher,
        rest: &[u8],
        buf: &mut CodePointBuf,
    ) -> bool {
        self.states[target]
            .iter()
            .any(|&state| passes(matcher, state, rest, buf))
    }

    pub(super) fn parts(
        &self,
        target: usize,
        matcher: &Matcher,
        rest: &[u8],
        buf: &mut CodePointBuf,
        parts: &mut Vec<Range<usize>>,
    ) {
        for &state in &self.states[target] {
            if !passes(matcher, state, rest, buf) {
                continue;
            }
            if let State::Position { at, .. } = state {
                match matcher {
                    Matcher::Wildcard(wildcard) => wildcard.remainder_parts(at, rest, buf, parts),
                    _ => {
                        let end = matcher
                            .literal()
                            .expect("a link matcher is literal or wildcard")
                            .len()
                            - at;
                        parts.push(0..end);
                    }
                }
            }
        }
    }
}

fn passes(matcher: &Matcher, state: State, rest: &[u8], buf: &mut CodePointBuf) -> bool {
    let State::Position { at, word } = state else {
        return true;
    };
    if let Matcher::Wildcard(wildcard) = matcher {
        return wildcard.matches_remainder(at, rest, buf);
    }
    let text = matcher
        .literal()
        .expect("a link matcher is literal or wildcard");
    let remainder = &text[at..];
    rest.starts_with(remainder)
        && !(at == 0 && word && matches!(matcher, Matcher::WholeWord(_) | Matcher::WordStart(_)))
        && !(matches!(matcher, Matcher::WholeWord(_))
            && is_word(CodePoints::new(&rest[remainder.len()..]).next()))
}

fn advance(matcher: &Matcher, state: State, text: &[u8]) -> Vec<State> {
    let State::Position { at, word } = state else {
        return vec![State::Matched];
    };
    if let Matcher::Wildcard(wildcard) = matcher {
        return wildcard
            .advance(at, text)
            .into_iter()
            .map(|at| State::Position { at, word: false })
            .collect();
    }
    let needle = matcher
        .literal()
        .expect("a link matcher is literal or wildcard");
    let mut states = vec![State::Position { at, word }];
    let mut next = Vec::new();
    let mut bytes = Vec::with_capacity(4);
    for cp in CodePoints::new(text) {
        next.clear();
        bytes.clear();
        encode_code_point(cp, &mut bytes);
        for &state in &states {
            let State::Position { at, word } = state else {
                return vec![State::Matched];
            };
            if at == needle.len() && !is_word(Some(cp)) {
                return vec![State::Matched];
            }
            if !matches!(matcher, Matcher::Start(_)) {
                next.push(State::Position {
                    at: 0,
                    word: is_word(Some(cp)),
                });
                if at != 0
                    && (!word || matches!(matcher, Matcher::Contains(_)))
                    && needle.starts_with(&bytes)
                {
                    let at = bytes.len();
                    if at == needle.len() && !matches!(matcher, Matcher::WholeWord(_)) {
                        return vec![State::Matched];
                    }
                    next.push(State::Position {
                        at,
                        word: is_word(Some(cp)),
                    });
                }
            }
            if !(at == 0
                && word
                && matches!(matcher, Matcher::WholeWord(_) | Matcher::WordStart(_)))
                && needle[at..].starts_with(&bytes)
            {
                let at = at + bytes.len();
                if at == needle.len() && !matches!(matcher, Matcher::WholeWord(_)) {
                    return vec![State::Matched];
                }
                next.push(State::Position {
                    at,
                    word: is_word(Some(cp)),
                });
            }
        }
        next.sort_unstable();
        next.dedup();
        std::mem::swap(&mut states, &mut next);
    }
    states
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::links::FolderLink;
    use crate::query::engine::Check;
    use crate::query::{MatchedParts, Test, parse_query};
    use proptest::prelude::*;

    fn matcher(test: &Test, case: bool) -> Matcher {
        let Check::Text(_, matcher) = Check::new(test, case).unwrap() else {
            unreachable!()
        };
        matcher
    }

    fn passes(links: &Links, test: &Test, path: &str, case: bool) -> bool {
        let matcher = matcher(test, case);
        let paths = LinkPaths::new(links, case);
        let through = paths.compile(&matcher);
        let path = if case {
            path.as_bytes().to_vec()
        } else {
            fold(path.as_bytes())
        };
        let mut buf = CodePointBuf::new();
        let numbers = paths
            .targets
            .iter()
            .enumerate()
            .map(|(number, target)| (fold(target.raw.as_bytes()), number))
            .collect();
        matcher.matches(&path, &mut buf)
            || nearest(&numbers, &path, true)
                .is_some_and(|(number, at)| through.passes(number, &matcher, &path[at..], &mut buf))
    }

    #[test]
    fn loops_and_nested_targets_preserve_remainders_through_links() {
        let links = Links::new(vec![
            FolderLink {
                link: r"C:\loop".into(),
                target: "C:".into(),
            },
            FolderLink {
                link: r"C:\alias".into(),
                target: r"D:\outer".into(),
            },
            FolderLink {
                link: r"C:\nested".into(),
                target: r"D:\outer\inner".into(),
            },
        ]);
        let loops = format!("{}file", r"loop\".repeat(70));
        assert!(passes(
            &links,
            &Test::Path(loops.clone()),
            r"C:\file",
            false
        ));
        assert!(passes(
            &links,
            &Test::PathWildcard(format!("C:\\*{loops}*")),
            r"C:\file",
            false
        ));
        assert!(passes(
            &links,
            &Test::Path(r"alias\inner\file".into()),
            r"D:\outer\inner\file",
            false
        ));
        assert!(passes(
            &links,
            &Test::Path(r"C:\alias".into()),
            r"D:\outer\inner\file",
            false
        ));
        assert!(!passes(
            &links,
            &Test::PathWholeWord(r"lias\inner".into()),
            r"D:\outer\inner\file",
            false
        ));
        assert!(!passes(
            &links,
            &Test::PathWholeWord(r"alias\inner".into()),
            r"D:\outer\inners\file",
            false
        ));
    }

    #[test]
    #[allow(
        clippy::single_range_in_vec_init,
        reason = "the expected highlight is one byte range"
    )]
    fn bold_marks_only_the_visible_remainder_through_links() {
        let links = Links::new(vec![FolderLink {
            link: r"C:\repo\artifacts".into(),
            target: r"V:\build".into(),
        }]);
        let parts =
            MatchedParts::with_links(&parse_query(r"artifacts\évidence").unwrap(), &links).unwrap();
        let path = r"V:\build\Évidence\x";
        assert_eq!(parts.ranges(path), [8..18]);
        let parts =
            MatchedParts::with_links(&parse_query(r"repo\artifacts").unwrap(), &links).unwrap();
        assert!(parts.ranges(path).is_empty());
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        // Each folder has one outgoing link, so this oracle can enumerate
        // whole paths even through cycles. An accepting walk needs no repeated
        // (target, matcher position) pair; twice the literal's byte length
        // also covers word-boundary states and bounds every generated matcher.
        #[test]
        fn states_agree_with_whole_paths_on_random_links(
            next in proptest::array::uniform3(0usize..3),
            names in proptest::array::uniform3(prop::sample::select(vec!["a", "bb", "É", "x-y"])),
            start in 0usize..3, depth in 0usize..5,
            cut1 in any::<usize>(), cut2 in any::<usize>(),
            mode in 0..5, case in any::<bool>(), wild in proptest::collection::vec(0u8..5, 1..40),
        ) {
            let links = Links::new((0..3).map(|i| FolderLink {
                link: format!("C:\\d{i}\\{}", names[i]), target: format!("C:\\d{}", next[i]),
            }).collect());
            let mut named = format!("C:\\d{start}");
            let mut at = start;
            for _ in 0..=depth { named += &format!("\\{}", names[at]); at = next[at]; }
            named += "\\file";
            let chars: Vec<char> = named.chars().collect();
            let a = cut1 % chars.len();
            let b = cut2 % chars.len();
            let text: String = if mode == 4 {
                chars.iter().enumerate().map(|(i, &ch)| match wild[i % wild.len()] {
                    0 => '*', 1 => '?', _ => ch,
                }).collect()
            } else {
                chars[a.min(b)..=a.max(b)].iter().collect()
            };
            let test = match mode {
                0 => Test::Path(text.clone()), 1 => Test::PathWholeWord(text.clone()),
                2 => Test::PathWordStart(text.clone()), 3 => Test::PathStart(text.clone()),
                _ => Test::PathWildcard(text.clone()),
            };
            let matcher = matcher(&test, case);
            let form = |s: &str| if case { s.as_bytes().to_vec() } else { fold(s.as_bytes()) };
            let mut buf = CodePointBuf::new();
            let real = format!("C:\\d{at}\\file");
            let mut expected = matcher.matches(&form(&real), &mut buf);
            for seed in 0..3 {
                let mut path = format!("C:\\d{seed}");
                let mut target = seed;
                for _ in 0..3 * (2 * text.len() + 3) {
                    path += &format!("\\{}", names[target]);
                    target = next[target];
                    if target == at && matcher.matches(&form(&format!("{path}\\file")), &mut buf) {
                        expected = true;
                        break;
                    }
                }
            }
            prop_assert_eq!(passes(&links, &test, &real, case), expected, "{:?} on {:?}", test, links);
        }
    }
}
