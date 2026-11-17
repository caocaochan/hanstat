//! Unsupervised discovery of book-specific words (mainly names) that the
//! segmenter splits or glues to a neighbour, so the user can force them without having read the book.
//!
//! Classic "new word discovery" (新词发现) on runs of Chinese characters:
//! * frequency — the string recurs;
//! * cohesion — min over split points of f(w)·N / (f(left)·f(right)): its parts
//!   co-occur far more than chance (pointwise mutual information);
//! * boundary freedom — entropy of the characters seen immediately left/right:
//!   a real word appears in many contexts, a fragment like 简言 (of 温简言) does not;
//!
//! plus evidence from the segmenter itself: how often it kept the string whole.

use crate::chars;
use crate::extract::Book;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

const MAX_N: usize = 6;
const EDGE: u32 = 0;

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub word: String,
    /// Occurrences of the string in the text.
    pub occurrences: u32,
    /// How many of those the segmenter got right: the token is this string, or
    /// a longer dictionary word containing it. The rest were split, or glued to
    /// a neighbour (蒋舸没).
    pub kept_whole: u32,
    pub cohesion: f64,
    /// Min over split points of f(w) / min(f(left), f(right)): the share of the rarer
    /// part's occurrences that fall inside this string (≈1 for names).
    pub dependency: f64,
    pub left_entropy: f64,
    pub right_entropy: f64,
    /// Known to a general dictionary, jieba's or CC-CEDICT (then it's usually a granularity
    /// difference, not an unknown name).
    pub in_dictionary: bool,
    /// Pre-selected: high confidence it is a book-specific word.
    pub recommended: bool,
}

impl Candidate {
    pub fn split_occurrences(&self) -> u32 {
        self.occurrences.saturating_sub(self.kept_whole)
    }
}

#[inline]
fn key(cs: &[char]) -> u128 {
    cs.iter()
        .enumerate()
        .fold(0u128, |k, (i, &c)| k | ((c as u128) << (21 * i)))
}

fn unkey(mut k: u128) -> String {
    let mut s = String::new();
    while k != 0 {
        s.push(char::from_u32((k & 0x1F_FFFF) as u32).unwrap_or('?'));
        k >>= 21;
    }
    s
}

/// `c` or its Simplified form is in `set` (the stop lists are written in
/// Simplified; the n-grams keep the book's own characters).
fn in_set(set: &str, c: char) -> bool {
    set.contains(c)
        || crate::script::TABLES
            .t2s
            .get(&c)
            .is_some_and(|&s| set.contains(s))
}

/// Particles/function characters that essentially never start or end a name.
/// 著 is listed itself: it is also Simplified, so OpenCC has no 著→着 char mapping.
fn is_edge_stop(c: char) -> bool {
    in_set(
        "的了着著地得过吗呢吧啊呀么嘛哦是在和与就都也又还很被把将从向对给让跟这那些个",
        c,
    )
}

/// Grammatical characters that mark a phrase when they occur anywhere inside.
fn is_inner_stop(c: char) -> bool {
    is_edge_stop(c) || in_set("一不没上下来去起", c)
}

/// How many occurrences of each 2..=MAX_N-char string the segmenter got right:
/// the string is the whole token, or lies inside a longer token that is a real
/// word (`is_word`: 什麼 in 為什麼, 朋友 in 男朋友). Inside a longer token that is
/// no word, the string was glued to its neighbours (蒋舸 in 蒋舸没, 蒋舸问), which
/// is as wrong as splitting it, so those occurrences aren't counted.
fn within_token_counts(
    word_freq: &[(String, u32)],
    is_word: impl Fn(&str) -> bool,
) -> FxHashMap<u128, u32> {
    let mut m = FxHashMap::<u128, u32>::default();
    for (w, n) in word_freq {
        let cs: Vec<char> = w.chars().collect();
        if (2..=MAX_N).contains(&cs.len()) {
            *m.entry(key(&cs)).or_default() += n;
        }
        if cs.len() <= 2 || !is_word(w) {
            continue;
        }
        for len in 2..=MAX_N.min(cs.len() - 1) {
            for i in 0..=cs.len() - len {
                *m.entry(key(&cs[i..i + len])).or_default() += n;
            }
        }
    }
    m
}

fn entropy(m: &FxHashMap<u32, u32>) -> f64 {
    let total: u32 = m.values().sum();
    if total == 0 {
        return 0.0;
    }
    let t = total as f64;
    let mut h = 0.0;
    for (&c, &n) in m {
        if c == EDGE {
            // Each run edge (punctuation, paragraph start/end) counts as its own
            // distinct neighbour: a boundary there is maximally free.
            h += n as f64 * (t.ln()) / t;
        } else {
            let p = n as f64 / t;
            h -= p * p.ln();
        }
    }
    h
}

pub struct Options {
    pub min_occurrences: u32,
}

impl Options {
    pub fn for_size(han_total: u64) -> Self {
        Options {
            min_occurrences: ((han_total / 200_000) as u32).clamp(5, 50),
        }
    }
}

/// `word_freq`: the segmenter's token counts over the same sections.
/// `known`: words already forced (user dictionary) — never suggested.
/// Chinese only: a Korean or Japanese book has no suggestions.
pub fn discover(
    book: &Book,
    included: &[bool],
    word_freq: &[(String, u32)],
    known: &HashSet<String>,
    opts: &Options,
) -> Vec<Candidate> {
    discover_cancellable(
        book,
        included,
        word_freq,
        known,
        opts,
        &AtomicBool::new(false),
    )
    .unwrap_or_default()
}

/// [`discover`] that gives up, returning `None`, once `cancel` is set (the
/// window starts it again when the included sections change).
pub fn discover_cancellable(
    book: &Book,
    included: &[bool],
    word_freq: &[(String, u32)],
    known: &HashSet<String>,
    opts: &Options,
    cancel: &AtomicBool,
) -> Option<Vec<Candidate>> {
    // Not `lexicon::is_dictionary_word`: its extra words and idioms would hide
    // names that are also rare words. Measured on five novels: jieba plus
    // CC-CEDICT drops 1–3 non-names per book from the recommendations and no
    // name; a dictionary alone, without jieba, would recommend set phrases
    // (摇摇头, 话音刚落) that dictionaries don't list.
    let jieba = crate::lexicon::jieba();
    let cedict = crate::lexicon::cedict_words();
    let is_general = |w: &str| {
        let s = crate::script::fold_t2s(w);
        jieba.has_word(w) || jieba.has_word(&s) || cedict.contains(w) || cedict.contains(s.as_str())
    };
    discover_impl(book, included, word_freq, known, opts, &is_general, cancel)
}

/// [`discover`] with another general dictionary: `is_general` says whether a
/// string (in the book's script) is a word of the general language.
pub fn discover_with(
    book: &Book,
    included: &[bool],
    word_freq: &[(String, u32)],
    known: &HashSet<String>,
    opts: &Options,
    is_general: &(dyn Fn(&str) -> bool + Sync),
) -> Vec<Candidate> {
    discover_impl(
        book,
        included,
        word_freq,
        known,
        opts,
        is_general,
        &AtomicBool::new(false),
    )
    .unwrap_or_default()
}

fn discover_impl(
    book: &Book,
    included: &[bool],
    word_freq: &[(String, u32)],
    known: &HashSet<String>,
    opts: &Options,
    is_general: &(dyn Fn(&str) -> bool + Sync),
    cancel: &AtomicBool,
) -> Option<Vec<Candidate>> {
    let cancelled = || cancel.load(Ordering::Relaxed);
    if crate::analyze::detect_language(book) != crate::lang::Language::Chinese {
        return Some(Vec::new());
    }
    // Han-only runs of the normalised text.
    let runs: Vec<Vec<char>> = book
        .sections
        .par_iter()
        .zip(included.par_iter())
        .filter(|(_, inc)| **inc)
        .flat_map_iter(|(s, _)| {
            let mut ns = chars::NormStats::default();
            let mut out = Vec::new();
            for p in &s.paragraphs {
                let p = chars::normalize(p, &mut ns);
                let mut run = Vec::new();
                for c in p.chars() {
                    if chars::is_han(c) {
                        run.push(c);
                    } else if !run.is_empty() {
                        out.push(std::mem::take(&mut run));
                    }
                }
                if !run.is_empty() {
                    out.push(run);
                }
            }
            out
        })
        .collect();
    let min = opts.min_occurrences;

    // Level-wise (apriori) counting: an n-gram can only be frequent if its
    // (n-1)-prefix and (n-1)-suffix are.
    let mut freq: Vec<FxHashMap<u128, u32>> = vec![FxHashMap::default(); MAX_N + 1];
    for n in 1..=MAX_N {
        if cancelled() {
            return None;
        }
        let prev = if n > 1 { Some(&freq[n - 1]) } else { None };
        let counts = runs
            .par_chunks(512)
            .fold(FxHashMap::<u128, u32>::default, |mut m, chunk| {
                for r in chunk {
                    if r.len() < n {
                        continue;
                    }
                    for i in 0..=r.len() - n {
                        if let Some(prev) = prev
                            && (!prev.contains_key(&key(&r[i..i + n - 1]))
                                || !prev.contains_key(&key(&r[i + 1..i + n])))
                        {
                            continue;
                        }
                        *m.entry(key(&r[i..i + n])).or_default() += 1;
                    }
                }
                m
            })
            .reduce(FxHashMap::default, |a, b| {
                let (mut big, small) = if a.len() >= b.len() { (a, b) } else { (b, a) };
                for (k, v) in small {
                    *big.entry(k).or_default() += v;
                }
                big
            });
        // Unigrams are kept in full (needed as denominators); others pruned.
        freq[n] = if n == 1 {
            counts
        } else {
            counts.into_iter().filter(|(_, v)| *v >= min).collect()
        };
    }
    let total: f64 = freq[1].values().map(|&v| v as f64).sum();

    // Cohesion filter (cheap, frequency-only).
    let get = |cs: &[char]| freq[cs.len()].get(&key(cs)).copied().unwrap_or(0) as f64;
    // key -> (cohesion, dependency, left neighbours, right neighbours)
    type Neighbours = FxHashMap<u32, u32>;
    let mut cand: FxHashMap<u128, (f64, f64, Neighbours, Neighbours)> = FxHashMap::default();
    for n in 2..=MAX_N {
        for (&k, &f) in &freq[n] {
            let cs: Vec<char> = unkey(k).chars().collect();
            if is_edge_stop(cs[0]) || is_edge_stop(cs[n - 1]) {
                continue;
            }
            let coh = (1..n)
                .map(|s| f as f64 * total / (get(&cs[..s]) * get(&cs[s..])).max(1.0))
                .fold(f64::INFINITY, f64::min);
            let dep = (1..n)
                .map(|s| f as f64 / get(&cs[..s]).min(get(&cs[s..])).max(1.0))
                .fold(f64::INFINITY, f64::min);
            if coh >= 20.0 {
                cand.insert(k, (coh, dep, FxHashMap::default(), FxHashMap::default()));
            }
        }
    }

    if cancelled() {
        return None;
    }
    // Neighbour distributions for surviving candidates.
    for r in &runs {
        for n in 2..=MAX_N.min(r.len()) {
            for i in 0..=r.len() - n {
                if let Some((_, _, l, rt)) = cand.get_mut(&key(&r[i..i + n])) {
                    *l.entry(if i == 0 { EDGE } else { r[i - 1] as u32 })
                        .or_default() += 1;
                    *rt.entry(if i + n == r.len() {
                        EDGE
                    } else {
                        r[i + n] as u32
                    })
                    .or_default() += 1;
                }
            }
        }
    }

    if cancelled() {
        return None;
    }
    let tokens = within_token_counts(word_freq, is_general);
    let mut out: Vec<Candidate> = cand
        .into_iter()
        .filter_map(|(k, (coh, dep, l, r))| {
            let word = unkey(k);
            if known.contains(&word) {
                return None;
            }
            let (hl, hr) = (entropy(&l), entropy(&r));
            if hl.min(hr) < 1.5 || dep < 0.4 {
                return None;
            }
            let occurrences = freq[word.chars().count()][&k];
            let kept_whole = tokens.get(&k).copied().unwrap_or(0).min(occurrences);
            let split = occurrences - kept_whole;
            // Only worth suggesting if the segmenter splits it a meaningful share of the time.
            if split < min || (split as f64) < 0.01 * occurrences as f64 {
                return None;
            }
            let in_dictionary = is_general(&word);
            // Names: parts rarely occur elsewhere (high dependency), free boundaries,
            // and the model already treats it as one word *some* of the time. Always-split
            // strings are usually set phrases (指了指, 精神一振) or compounds the
            // segmentation standard deliberately splits (兴旺/酒店): listed, not pre-selected.
            let whole_share = kept_whole as f64 / occurrences as f64;
            let hmin = hl.min(hr);
            // Model-free path for strong evidence (also covers dictionary-only
            // segmenters like jieba, which never keep unknown words whole):
            // near-total dependency, very free boundaries, and no grammatical
            // particles inside (those make 指了指 / 精神一振 phrases, not names).
            let strong = dep >= 0.8 && hmin >= 2.5 && !word.chars().any(is_inner_stop);
            let recommended = !in_dictionary
                && hmin >= 2.0
                && word.chars().count() <= 4
                && ((dep >= 0.6 && (0.03..=0.99).contains(&whole_share)) || strong);
            Some(Candidate {
                word,
                occurrences,
                kept_whole,
                cohesion: coh,
                dependency: dep,
                left_entropy: hl,
                right_entropy: hr,
                in_dictionary,
                recommended,
            })
        })
        .collect();

    // Drop a candidate that is (almost) always part of a longer accepted one,
    // and a longer one that is just a shorter one plus a free-floating char.
    let words: Vec<(String, u32)> = out
        .iter()
        .map(|c| (c.word.clone(), c.occurrences))
        .collect();
    out.retain(|c| {
        !words.iter().any(|(w, n)| {
            w.len() > c.word.len() && w.contains(&c.word) && *n as f64 >= 0.8 * c.occurrences as f64
        })
    });
    out.sort_by(|a, b| {
        b.recommended.cmp(&a.recommended).then_with(|| {
            if a.recommended {
                b.split_occurrences().cmp(&a.split_occurrences())
            } else {
                b.dependency.total_cmp(&a.dependency)
            }
        })
    });
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_roundtrip() {
        let s: Vec<char> = "温简言𠮷野家".chars().collect();
        assert_eq!(unkey(key(&s)), "温简言𠮷野家");
    }

    #[test]
    fn words_inside_longer_tokens_are_not_split() {
        let freq = [
            ("為什麼".to_owned(), 3),
            ("什麼".to_owned(), 5),
            ("什".to_owned(), 1),
        ];
        let m = within_token_counts(&freq, |w| w == "為什麼");
        let get = |s: &str| m.get(&key(&s.chars().collect::<Vec<_>>())).copied();
        assert_eq!(get("什麼"), Some(8));
        assert_eq!(get("為什"), Some(3));
        assert_eq!(get("為什麼"), Some(3));
    }

    #[test]
    fn names_glued_to_a_neighbour_are_not_kept_whole() {
        let freq = [
            ("蒋舸".to_owned(), 10),
            ("蒋舸没".to_owned(), 2),
            ("蒋舸问".to_owned(), 1),
        ];
        let m = within_token_counts(&freq, |_| false);
        let get = |s: &str| m.get(&key(&s.chars().collect::<Vec<_>>())).copied();
        assert_eq!(get("蒋舸"), Some(10));
        assert_eq!(get("蒋舸没"), Some(2));
        assert_eq!(get("舸没"), None);
    }

    #[test]
    fn stop_chars_match_traditional() {
        for c in ['麼', '么', '著', '過', '這', '還'] {
            assert!(is_edge_stop(c), "{c}");
        }
        assert!(is_inner_stop('沒') && is_inner_stop('來'));
        assert!(!is_edge_stop('阿'));
    }
}
