//! Bundled general-language word lists, and the book-level clean-up of the
//! segmenter's rare unknown tokens.
//!
//! A statistical segmenter invents a "word" wherever it misplaces a boundary:
//! 他/妈凭/什么, 木柯跑, 时此刻 (of 此时此刻). Each such token occurs once or
//! twice, but together they made up about a third of a novel's distinct words
//! (measured on 我在无限游戏里封神 with LTP: 43,748 → 29,392 once split again).
//! [`resplit_map`] splits every token that no dictionary knows and that the book
//! itself produces fewer than [`KEEP_MIN`] times into dictionary words and the
//! book's own frequent tokens (its names: 木柯跑 → 木柯/跑).

use crate::script;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// A token the book produces at least this often is kept whole (a name, a term
/// of the book); a rarer one is split again unless a dictionary knows it.
/// Measured on one novel: 3 → 30,377 distinct words, 5 → 29,377, 10 → 28,828.
pub const KEEP_MIN: u32 = 5;

/// Longest piece tried when splitting a token.
const MAX_PIECE: usize = 8;

/// jieba's dictionary, built once: building it takes a noticeable moment.
pub fn jieba() -> &'static jieba_rs::Jieba {
    static JIEBA: OnceLock<jieba_rs::Jieba> = OnceLock::new();
    JIEBA.get_or_init(jieba_rs::Jieba::new)
}

fn list(src: &'static str) -> HashSet<&'static str> {
    src.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// CC-CEDICT's general-language words that jieba's dictionary lacks (靠谱,
/// 抛尸, 副市长), from `data/CEDICT-words.txt` (built by examples/cedict_words.rs).
pub fn cedict_words() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| list(include_str!("../data/CEDICT-words.txt")))
}

/// Words of four modern dictionaries that neither of the above has (修眉, 餍足,
/// 幺蛾子), in Simplified, from `data/extra-words.txt` (examples/headwords.rs).
/// Not used by word discovery: they would hide names that are also rare words.
pub fn extra_words() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| list(include_str!("../data/extra-words.txt")))
}

/// Idioms of 4+ characters in Simplified, from `data/chengyu.txt` (examples/headwords.rs).
pub fn chengyu() -> &'static [&'static str] {
    static LIST: OnceLock<Vec<&'static str>> = OnceLock::new();
    LIST.get_or_init(|| {
        list(include_str!("../data/chengyu.txt"))
            .into_iter()
            .collect()
    })
}

fn chengyu_set() -> &'static HashSet<&'static str> {
    static SET: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SET.get_or_init(|| chengyu().iter().copied().collect())
}

/// `w` (Simplified) is in one of the bundled dictionaries.
fn known_simplified(w: &str) -> bool {
    jieba().has_word(w)
        || cedict_words().contains(w)
        || extra_words().contains(w)
        || chengyu_set().contains(w)
}

/// A word of the general language, in either script: jieba's dictionary,
/// CC-CEDICT, the extra words or an idiom.
pub fn is_dictionary_word(w: &str) -> bool {
    let s = script::fold_t2s(w);
    known_simplified(&s) || (s != w && known_simplified(w))
}

/// A rare unknown token that occurs at least this often in the text…
const NAME_MIN_TEXT: u32 = 2;
/// …and whose parts occur mostly inside it is taken for a name and kept whole:
/// min over its split points of f(token) / min(f(left), f(right)), as word
/// discovery's dependency. 白衢: 衢 occurs only there (1.0); 妈凭: 凭 occurs
/// mostly elsewhere (0.02).
const NAME_MIN_DEPENDENCY: f64 = 0.6;

/// How to split again the rare unknown tokens of a book (see the module docs):
/// token → its pieces. `counts` are the book's token counts (all sections);
/// `protected` words (the user dictionary, known words; any script) are never
/// split and may be pieces; `text` is the book's (normalised) text, in which
/// rare names are recognised (see [`NAME_MIN_DEPENDENCY`]): they are not split
/// either, and may be pieces. Only all-Han tokens of 2+ characters are split.
pub fn resplit_map(
    counts: &HashMap<String, u32>,
    protected: &[String],
    text: &[String],
) -> HashMap<String, Vec<String>> {
    let mut protected: HashSet<String> = protected.iter().map(|w| script::fold_t2s(w)).collect();
    let rare_unknown = |w: &str, n: u32| {
        n < KEEP_MIN
            && w.chars().nth(1).is_some()
            && w.chars().all(crate::chars::is_han)
            && !is_dictionary_word(w)
            && !protected.contains(&script::fold_t2s(w))
    };
    let candidates: Vec<&str> = counts
        .iter()
        .filter(|(w, n)| rare_unknown(w, **n))
        .map(|(w, _)| w.as_str())
        .collect();
    let names = names(&candidates, text);
    protected.extend(names.iter().map(|w| script::fold_t2s(w)));
    // Score of a piece, or None if it can't be one. Splits with fewer pieces win;
    // among those, the more frequent pieces.
    let score = |p: &str| -> Option<f64> {
        let s = script::fold_t2s(p);
        if jieba().has_word(&s) {
            return Some((jieba().suggest_freq(&s) as f64).ln());
        }
        if let Some(&n) = counts.get(p).filter(|&&n| n >= KEEP_MIN) {
            return Some(((n + 1) as f64).ln());
        }
        if known_simplified(&s) || protected.contains(&s) {
            return Some(1.0);
        }
        (p.chars().count() == 1).then_some(0.0)
    };
    candidates
        .into_iter()
        .filter(|w| !names.contains(*w))
        .map(|w| (w.to_owned(), split(w, &score)))
        .collect()
}

/// The `candidates` that look like names in `text` (see [`NAME_MIN_DEPENDENCY`]).
fn names<'a>(candidates: &[&'a str], text: &[String]) -> HashSet<&'a str> {
    // Each candidate and its prefixes and suffixes, counted (overlapping) in the text.
    let mut index: HashMap<&str, usize> = HashMap::new();
    for w in candidates {
        let offs: Vec<usize> = w.char_indices().map(|(i, _)| i).chain([w.len()]).collect();
        for &o in &offs[..offs.len() - 1] {
            let n = index.len();
            index.entry(&w[o..]).or_insert(n);
        }
        for &o in &offs[1..] {
            let n = index.len();
            index.entry(&w[..o]).or_insert(n);
        }
    }
    if index.is_empty() {
        return HashSet::new();
    }
    let mut pats: Vec<&str> = vec![""; index.len()];
    for (p, &i) in &index {
        pats[i] = p;
    }
    let ac = aho_corasick::AhoCorasick::new(&pats).expect("name patterns");
    let mut f = vec![0u32; pats.len()];
    // The characters seen just left and right of each pattern.
    let mut sides = vec![(Side::Unseen, Side::Unseen); pats.len()];
    for t in text {
        for m in ac.find_overlapping_iter(t) {
            let i = m.pattern().as_usize();
            f[i] += 1;
            sides[i].0.see(t[..m.start()].chars().next_back());
            sides[i].1.see(t[m.end()..].chars().next());
        }
    }
    let count = |p: &str| f[index[p]];
    candidates
        .iter()
        .copied()
        .filter(|w| {
            let n = count(w);
            let (l, r) = sides[index[w]];
            n >= NAME_MIN_TEXT
                && l == Side::Mixed
                && r == Side::Mixed
                && w.char_indices().skip(1).all(|(o, _)| {
                    let parts = count(&w[..o]).min(count(&w[o..])).max(1);
                    n as f64 / parts as f64 >= NAME_MIN_DEPENDENCY
                })
        })
        .collect()
}

/// The neighbours seen on one side of a string. A string that always has the
/// same character on one side is a fragment of a longer one (三鹦 of 杜三鹦,
/// 歇斯 of 歇斯底里), not a name.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Unseen,
    Same(char),
    /// Different characters, or a run edge (punctuation, paragraph start or end).
    Mixed,
}

impl Side {
    fn see(&mut self, c: Option<char>) {
        *self = match (*self, c.filter(|&c| crate::chars::is_han(c))) {
            (Side::Unseen, Some(c)) => Side::Same(c),
            (Side::Same(a), Some(c)) if a == c => Side::Same(a),
            _ => Side::Mixed,
        };
    }
}

/// Split `w` into the fewest pieces `score` accepts (never `w` itself), the
/// highest total score breaking ties. Single characters are always accepted by
/// the caller, so a split always exists.
fn split(w: &str, score: &dyn Fn(&str) -> Option<f64>) -> Vec<String> {
    let offs: Vec<usize> = w.char_indices().map(|(i, _)| i).chain([w.len()]).collect();
    let n = offs.len() - 1;
    // best[i] = (pieces, score, start of the last piece) for the prefix of i chars
    let mut best: Vec<(usize, f64, usize)> = vec![(usize::MAX, 0.0, 0); n + 1];
    best[0] = (0, 0.0, 0);
    for i in 1..=n {
        for j in i.saturating_sub(MAX_PIECE)..i {
            if best[j].0 == usize::MAX || (j == 0 && i == n) {
                continue;
            }
            let Some(v) = score(&w[offs[j]..offs[i]]) else {
                continue;
            };
            let cand = (best[j].0 + 1, best[j].1 + v);
            if cand.0 < best[i].0 || (cand.0 == best[i].0 && cand.1 > best[i].1) {
                best[i] = (cand.0, cand.1, j);
            }
        }
    }
    let mut out = Vec::new();
    let mut i = n;
    while i > 0 {
        let j = best[i].2;
        out.push(w[offs[j]..offs[i]].to_owned());
        i = j;
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn counts(v: &[(&str, u32)]) -> HashMap<String, u32> {
        v.iter().map(|(w, n)| (w.to_string(), *n)).collect()
    }

    #[test]
    fn lists_load() {
        assert!(chengyu().len() > 50_000);
        assert!(chengyu_set().contains("若有所思"));
        assert!(extra_words().len() > 5_000);
        assert!(is_dictionary_word("若有所思"));
        assert!(is_dictionary_word("聲嘶力竭")); // Traditional
        assert!(!is_dictionary_word("木柯跑"));
    }

    #[test]
    fn rare_unknown_tokens_split_around_the_books_names() {
        let c = counts(&[
            ("木柯", 2692),
            ("木柯跑", 1),
            ("妈凭", 1),
            ("苏恙", 692),
            ("苏恙手", 2),
            ("時此刻", 1),
            ("白柳", 18502),
        ]);
        let m = resplit_map(&c, &[], &[]);
        assert_eq!(m["木柯跑"], ["木柯", "跑"]);
        assert_eq!(m["苏恙手"], ["苏恙", "手"]);
        assert_eq!(m["妈凭"], ["妈", "凭"]);
        assert_eq!(m["時此刻"], ["時", "此刻"]); // pieces stay in the book's script
        // Frequent tokens and single characters are left alone.
        assert!(!m.contains_key("木柯") && !m.contains_key("白柳"));
    }

    #[test]
    fn dictionary_frequent_and_protected_tokens_stay_whole() {
        let c = counts(&[
            ("若有所思", 1), // idiom
            ("修眉", 1),     // extra-words
            ("生命值", 440), // the book's own term
            ("救救我", 1),   // forced by the user
            ("刘佳仪", 3),   // known word, in the other script
        ]);
        let m = resplit_map(&c, &["救救我".into(), "劉佳儀".into()], &[]);
        assert!(m.is_empty(), "{m:?}");
    }

    #[test]
    fn rare_names_stay_whole_and_free_their_glued_forms() {
        // 白衢 recurs and 衢 occurs nowhere else; 妈凭's 凭 occurs elsewhere.
        let mut text: Vec<String> = [
            "他看白衢说",
            "白衢说话",
            "白天凭什么",
            "他妈凭什么",
            "凭你说",
        ]
        .map(String::from)
        .into();
        text.extend(
            ["白衢来了", "我说", "你说", "她说"]
                .iter()
                .cycle()
                .take(24)
                .map(|s| s.to_string()),
        );
        let c = counts(&[("白衢", 4), ("白衢说", 2), ("妈凭", 1), ("白天", 9)]);
        let m = resplit_map(&c, &[], &text);
        assert!(!m.contains_key("白衢"), "{m:?}");
        // Twice in the text, but mostly 白衢 and 说 occur apart: glued.
        assert_eq!(m["白衢说"], ["白衢", "说"]);
        assert_eq!(m["妈凭"], ["妈", "凭"]);
        // Always after 杜: a fragment of 杜三鹦, not a name.
        let text: Vec<String> = ["杜三鹦来了", "看杜三鹦说"].map(String::from).into();
        let m = resplit_map(&counts(&[("三鹦", 2)]), &[], &text);
        assert_eq!(m["三鹦"], ["三", "鹦"]);
    }
}
