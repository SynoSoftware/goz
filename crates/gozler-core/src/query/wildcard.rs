//! The `*` and `?` wildcard matcher the engine compiles a wildcard term into.

use smallvec::SmallVec;

/// Scratch buffer of decoded code points, reused across candidates so a
/// wildcard scan doesn't heap-allocate per candidate. NTFS names are <= 255
/// code points, so the inline capacity never spills.
pub(crate) type CodePointBuf = SmallVec<[u32; 256]>;

/// A compiled wildcard pattern, matched against a whole name or, for a path
/// wildcard, a whole full path.
#[derive(Debug)]
pub(crate) struct Wildcard {
    tokens: Vec<GlobTok>,
}

#[derive(Debug, PartialEq, Eq)]
enum GlobTok {
    Lit(u32),
    /// `?`: exactly one code point.
    AnyOne,
    /// `*`: any run of code points (including empty).
    AnyRun,
}

impl Wildcard {
    pub(super) fn has_separator(&self) -> bool {
        self.tokens.contains(&GlobTok::Lit(u32::from(b'\\')))
    }

    pub(super) fn accepts_any_remainder(&self, at: usize) -> bool {
        self.tokens[at..] == [GlobTok::AnyRun]
    }

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
    pub(crate) fn matches_into(&self, name: &[u8], buf: &mut CodePointBuf) -> bool {
        self.matches_remainder(0, name, buf)
    }

    pub(super) fn matches_remainder(&self, at: usize, name: &[u8], buf: &mut CodePointBuf) -> bool {
        buf.clear();
        buf.extend(crate::wtf8::CodePoints::new(name));
        glob_match(&self.tokens[at..], buf.as_slice(), |_, _| {})
    }

    /// Every token position left after reading a path prefix; a star can
    /// consume another code point or let the following token consume it.
    pub(super) fn advance(&self, at: usize, text: &[u8]) -> Vec<usize> {
        let close = |states: &mut Vec<usize>| {
            let n = states.len();
            for i in 0..n {
                if self.tokens.get(states[i]) == Some(&GlobTok::AnyRun) {
                    states.push(states[i] + 1);
                }
            }
            states.sort_unstable();
            states.dedup();
        };
        let mut states = vec![at];
        close(&mut states);
        for cp in crate::wtf8::CodePoints::new(text) {
            states = states
                .into_iter()
                .filter_map(|at| match self.tokens.get(at) {
                    Some(GlobTok::AnyRun) => Some(at),
                    Some(GlobTok::AnyOne) => Some(at + 1),
                    Some(GlobTok::Lit(wanted)) if *wanted == cp => Some(at + 1),
                    _ => None,
                })
                .collect();
            close(&mut states);
        }
        states
    }

    /// Appends to `parts` the byte ranges of `text` that the pattern's
    /// literal characters matched, when the pattern matches the whole of
    /// `text`; one range per character, which the caller merges.
    pub(super) fn literal_parts(
        &self,
        text: &[u8],
        buf: &mut CodePointBuf,
        parts: &mut Vec<std::ops::Range<usize>>,
    ) {
        self.remainder_parts(0, text, buf, parts);
    }

    pub(super) fn remainder_parts(
        &self,
        at: usize,
        text: &[u8],
        buf: &mut CodePointBuf,
        parts: &mut Vec<std::ops::Range<usize>>,
    ) {
        let tokens = &self.tokens[at..];
        buf.clear();
        buf.extend(crate::wtf8::CodePoints::new(text));
        let mut matched_at = vec![0; tokens.len()];
        if !glob_match(tokens, buf.as_slice(), |token, at| {
            matched_at[token] = at;
        }) {
            return;
        }
        let mut offsets = Vec::with_capacity(buf.len() + 1);
        let mut offset = 0;
        for &cp in buf.iter() {
            offsets.push(offset);
            offset += match cp {
                0..0x80 => 1,
                0x80..0x800 => 2,
                0x800..0x1_0000 => 3,
                _ => 4,
            };
        }
        offsets.push(offset);
        for (token, &at) in tokens.iter().zip(&matched_at) {
            if let GlobTok::Lit(_) = token {
                parts.push(offsets[at]..offsets[at + 1]);
            }
        }
    }

    /// The longest run of literal characters in the pattern, as WTF-8 bytes
    /// (already folded iff the query is case-insensitive). A name matching the
    /// pattern must contain this substring, so it can drive a prefilter scan.
    /// `None` if the pattern is all wildcards (e.g. `*`, `?`).
    pub(super) fn longest_literal(&self) -> Option<Vec<u8>> {
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

    /// What this pattern needs of a full path when one name of the path
    /// decides it: a `*`, then characters without a `\`, `*` or `?`, and
    /// perhaps a last `*`. `None` for every other pattern, because its `?`
    /// or a `*` between characters can match a `\`.
    pub(super) fn one_name(&self) -> Option<OneName> {
        let [GlobTok::AnyRun, rest @ ..] = self.tokens.as_slice() else {
            return None;
        };
        let (literal, open_end) = match rest {
            [literal @ .., GlobTok::AnyRun] => (literal, true),
            literal => (literal, false),
        };
        let mut bytes = Vec::new();
        for token in literal {
            match token {
                GlobTok::Lit(cp) if *cp != u32::from(b'\\') => {
                    crate::wtf8::encode_code_point(*cp, &mut bytes);
                }
                _ => return None,
            }
        }
        Some(if open_end {
            OneName::Inside(bytes)
        } else {
            OneName::Last
        })
    }
}

/// Which name of a full path decides a pattern; see [`Wildcard::one_name`].
pub(super) enum OneName {
    /// `*text`: the full path ends with the text exactly when its last name
    /// does, so the pattern can test the name instead.
    Last,
    /// `*text*`: the text, as WTF-8 bytes, lies inside one name of the path.
    Inside(Vec<u8>),
}

/// Iterative glob match with backtracking on `*` (linear-ish; patterns and
/// names are short). `matched(token, at)` hears each pattern token that
/// consumes the code point at `at`; a backtrack hears the tokens after its
/// `*` again, so on success the last call for each token is where it matched.
fn glob_match(pat: &[GlobTok], text: &[u32], mut matched: impl FnMut(usize, usize)) -> bool {
    let (mut ti, mut pi) = (0usize, 0usize);
    let mut star: Option<(usize, usize)> = None; // (pattern idx after '*', text idx)
    while ti < text.len() {
        match pat.get(pi) {
            Some(GlobTok::Lit(c)) if *c == text[ti] => {
                matched(pi, ti);
                pi += 1;
                ti += 1;
            }
            Some(GlobTok::AnyOne) => {
                matched(pi, ti);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fold::fold;

    /// Whether the folded `name` matches the folded `pattern`, as a
    /// case-insensitive query compares them.
    fn matches(pattern: &str, name: &str) -> bool {
        Wildcard::compile(&fold(pattern.as_bytes()))
            .matches_into(&fold(name.as_bytes()), &mut CodePointBuf::new())
    }

    #[test]
    fn wildcards_match_the_whole_name() {
        assert!(matches("*.rs", "main.rs"));
        assert!(!matches("*.rs", "main.rs.bak"));
        assert!(matches("re*rt", "report"));
        assert!(matches("re*rt", "rert")); // "re" + empty + "rt"
        assert!(!matches("re*rt", "rt")); // missing the "re" prefix
        assert!(!matches("re*rt", "reprtx"));
    }

    #[test]
    fn question_mark_matches_one_code_point() {
        assert!(matches("f?o", "foo"));
        assert!(matches("f?o", "fao"));
        assert!(!matches("f?o", "fo"));
        assert!(!matches("f?o", "fooo"));
    }
}
