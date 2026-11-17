//! The Library's search box: a fuzzy filter with wildcards.
//!
//! The query is split at whitespace into terms, and a text matches when
//! every term does. A plain term matches when its characters appear in the
//! text in order, not necessarily together (`三体2` finds 三体Ⅱ·黑暗森林's
//! file `三体2-黑暗森林.epub`; `hp` finds "Harry Potter"). A term with `*`
//! (any run of characters, also none) or `?` (one character) must match the
//! whole text instead, as in Everything or a file dialog: `三体*` finds the
//! titles that start with 三体, `*传` those that end with 传. Case is
//! ignored and Traditional characters count as Simplified, so `三體` finds
//! 三体 and the other way round.

use crate::anki;

/// A parsed query; an empty one matches everything.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    terms: Vec<Term>,
}

#[derive(Clone, Debug, PartialEq)]
enum Term {
    /// Characters that must appear in order.
    Fuzzy(Vec<char>),
    /// A whole-text pattern with `*` and `?`.
    Glob(Vec<char>),
}

/// Lower case, Traditional folded to Simplified.
fn normalise(s: &str) -> Vec<char> {
    anki::fold(&s.to_lowercase(), true).chars().collect()
}

impl Query {
    pub fn new(q: &str) -> Query {
        Query {
            terms: q
                .split_whitespace()
                .map(|t| {
                    let cs = normalise(t);
                    if cs.iter().any(|&c| c == '*' || c == '?') {
                        Term::Glob(cs)
                    } else {
                        Term::Fuzzy(cs)
                    }
                })
                .collect(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }

    /// Whether every term matches at least one of `texts` (a title and a
    /// file name, say; each term may match a different one).
    pub fn matches<'a>(&self, texts: impl IntoIterator<Item = &'a str>) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let texts: Vec<Vec<char>> = texts.into_iter().map(normalise).collect();
        self.terms.iter().all(|t| {
            texts.iter().any(|x| match t {
                Term::Fuzzy(p) => subsequence(p, x),
                Term::Glob(p) => glob(p, x),
            })
        })
    }
}

/// `p`'s characters appear in `text` in order.
fn subsequence(p: &[char], text: &[char]) -> bool {
    let mut it = text.iter();
    p.iter().all(|c| it.any(|x| x == c))
}

/// `p` matches all of `text`; `*` is any run of characters, `?` one.
fn glob(p: &[char], text: &[char]) -> bool {
    let (mut i, mut j) = (0, 0);
    // The last `*` seen, and where in `text` it was tried up to.
    let mut star: Option<(usize, usize)> = None;
    while j < text.len() {
        if i < p.len() && (p[i] == '?' || p[i] == text[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some((i, j));
            i += 1;
        } else if let Some((si, sj)) = star {
            // Let the `*` take one more character and try again.
            i = si + 1;
            j = sj + 1;
            star = Some((si, sj + 1));
        } else {
            return false;
        }
    }
    p[i..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(q: &str, texts: &[&str]) -> bool {
        Query::new(q).matches(texts.iter().copied())
    }

    #[test]
    fn empty_matches_all() {
        assert!(Query::new("  ").is_empty());
        assert!(m("", &["anything"]));
    }

    #[test]
    fn fuzzy_in_order() {
        assert!(m("hp", &["Harry Potter"]));
        assert!(m("三体2", &["三体Ⅱ·黑暗森林", "三体2-黑暗森林.epub"]));
        assert!(m("黑森", &["三体Ⅱ·黑暗森林"]));
        assert!(!m("森黑", &["三体Ⅱ·黑暗森林"]));
        assert!(!m("hpx", &["Harry Potter"]));
    }

    #[test]
    fn every_term_must_match() {
        assert!(m("三体 森林", &["三体Ⅱ·黑暗森林"]));
        assert!(!m("三体 球状", &["三体Ⅱ·黑暗森林"]));
        // Terms may match different texts.
        assert!(m("三体 epub", &["三体", "三体.epub"]));
    }

    #[test]
    fn script_and_case_folded() {
        assert!(m("三體", &["三体"]));
        assert!(m("三体", &["三體"]));
        assert!(m("HARRY", &["harry potter"]));
    }

    #[test]
    fn wildcards_match_the_whole_text() {
        assert!(m("三体*", &["三体Ⅱ·黑暗森林"]));
        assert!(!m("森林*", &["三体Ⅱ·黑暗森林"]));
        assert!(m("*森林", &["三体Ⅱ·黑暗森林"]));
        assert!(m("*暗*", &["三体Ⅱ·黑暗森林"]));
        assert!(m("三体?·*", &["三体Ⅱ·黑暗森林"]));
        assert!(!m("三体?", &["三体Ⅱ·黑暗森林"]));
        assert!(m("*", &[""]));
        assert!(m("a*b*c", &["axxbyyc"]));
        assert!(!m("a*b*c", &["axxbyy"]));
        assert!(m("*.txt", &["title", "book.txt"]));
    }
}
