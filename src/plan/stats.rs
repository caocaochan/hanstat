//! Statistics of a whole library of one language, for the Library's Stats
//! tab: how many distinct words and characters its books use between them,
//! how widely the words are shared, and how much of it is known.

use super::{Known, Library};
use crate::anki::Knowledge;
use crate::profile::Profile;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

/// The shares of running words (or characters) that coverage is given at.
pub const COVER_AT: [f64; 4] = [80.0, 90.0, 95.0, 98.0];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibStats {
    /// Distinct words across the library (after folding, as the reading path).
    pub vocab: usize,
    /// Running words of every book, names included.
    pub running: u64,
    /// (n, percent of the library's running words that its n most frequent
    /// words make up), at roughly log-spaced n, ending at `vocab`.
    pub coverage: Vec<(usize, f64)>,
    /// The most frequent words needed for each of [`COVER_AT`].
    pub cover_at: [usize; 4],
    /// `spread[n - 1]`: distinct words found in exactly n books.
    pub spread: Vec<usize>,
    /// Distinct words of the library that are known.
    pub known_words: usize,
    /// Running words known across the library, names left out as on the Overview.
    pub known_running: f64,
    /// Per book of [`Library::books`]: running words known now.
    pub book_known: Vec<f64>,
    /// Unknown words (not names) found in the most books.
    pub widespread: Vec<WideWord>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WideWord {
    /// The word as the books' word data has it, which is what marking it
    /// known records, as in the Words tab: a book's own spelling (not folded).
    pub word: String,
    /// How a book writes it.
    pub shown: String,
    pub books: usize,
    /// Occurrences in all the books.
    pub count: u64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CharStats {
    /// Chinese characters in the running text of every book.
    pub total: u64,
    pub unique: usize,
    pub cover_at: [usize; 4],
    /// Distinct characters known.
    pub known: usize,
}

impl Library {
    /// `k` from [`Library::known`]; `top`: how many widespread unknown words to list.
    pub fn stats(&self, k: &Known, top: usize) -> LibStats {
        let n = self.vocab.len();
        let mut freq = vec![0u64; n];
        let mut books_in = vec![0u32; n];
        let mut name = vec![false; n];
        let mut unknown = vec![false; n];
        let (mut known, mut total) = (0u64, 0u64);
        let mut book_known = Vec::with_capacity(self.books.len());
        for (i, b) in self.books.iter().enumerate() {
            for (&(w, c), &nm) in b.words.iter().zip(&b.name) {
                let w = w as usize;
                freq[w] += c as u64;
                books_in[w] += 1;
                name[w] |= nm;
                unknown[w] |= !nm && !k.has(w as u32);
            }
            let s = self.scan(i, k);
            known += s.known;
            total += s.total;
            book_known.push(s.percent());
        }
        let mut spread = vec![0usize; self.books.len()];
        for &c in &books_in {
            if c > 0 {
                spread[c as usize - 1] += 1;
            }
        }
        let mut wide: Vec<usize> = (0..n).filter(|&w| unknown[w] && !name[w]).collect();
        wide.sort_unstable_by(|&a, &b| {
            books_in[b]
                .cmp(&books_in[a])
                .then(freq[b].cmp(&freq[a]))
                .then(a.cmp(&b))
        });
        let widespread = wide
            .into_iter()
            .take(top)
            .map(|w| {
                let shown = self.spelled(w as u32).to_owned();
                WideWord {
                    // Folded keys are kept as a book spells them; otherwise
                    // the key is the profile's own word.
                    word: if self.fold {
                        shown.clone()
                    } else {
                        self.vocab[w].clone()
                    },
                    shown,
                    books: books_in[w] as usize,
                    count: freq[w],
                }
            })
            .collect();
        let (coverage, cover_at) = coverage(freq);
        LibStats {
            vocab: n,
            running: coverage_total(&coverage),
            coverage: coverage.into_iter().map(|(r, p, _)| (r, p)).collect(),
            cover_at,
            spread,
            known_words: k.len(),
            known_running: if total == 0 {
                100.0
            } else {
                known as f64 * 100.0 / total as f64
            },
            book_known,
            widespread,
        }
    }

    /// The word as a book of the library spells it.
    fn spelled(&self, w: u32) -> &str {
        self.books
            .iter()
            .find_map(|b| b.spell.get(&w))
            .map_or(self.vocab[w as usize].as_str(), String::as_str)
    }
}

/// Characters of the books' words. With `fold`, Traditional
/// characters count as their Simplified form (說 and 说 are one), as the
/// library's words do.
pub fn char_stats(profiles: &[Profile], k: Option<&Knowledge>, fold: bool) -> CharStats {
    let mut counts: FxHashMap<char, u64> = FxHashMap::default();
    for p in profiles {
        for (w, n) in &p.words {
            for c in w.chars().filter(|&c| crate::chars::is_han(c)) {
                *counts.entry(crate::anki::fold_char(c, fold)).or_default() += *n as u64;
            }
        }
    }
    let known = k.map_or(0, |k| {
        counts.keys().filter(|&&c| k.char(c).is_known()).count()
    });
    let unique = counts.len();
    let (cov, cover_at) = coverage(counts.into_values().collect());
    CharStats {
        total: coverage_total(&cov),
        unique,
        cover_at,
        known,
    }
}

/// From item frequencies: (n, percent covered, occurrences covered) at
/// log-spaced n, and the items needed for each of [`COVER_AT`].
fn coverage(mut freq: Vec<u64>) -> (Vec<(usize, f64, u64)>, [usize; 4]) {
    freq.retain(|&f| f > 0);
    freq.sort_unstable_by(|a, b| b.cmp(a));
    let total: u64 = freq.iter().sum();
    let mut out = Vec::new();
    let mut at = [freq.len(); 4];
    let mut next = 1usize;
    let mut cum = 0u64;
    for (i, f) in freq.iter().enumerate() {
        cum += f;
        let r = i + 1;
        for (j, t) in COVER_AT.iter().enumerate() {
            if at[j] == freq.len() && cum as f64 * 100.0 >= t * total as f64 {
                at[j] = r;
            }
        }
        if r == next || r == freq.len() {
            out.push((r, cum as f64 * 100.0 / total as f64, cum));
            next = ((next as f64 * 1.06).ceil() as usize).max(next + 1);
        }
    }
    (out, at)
}

fn coverage_total(c: &[(usize, f64, u64)]) -> u64 {
    c.last().map_or(0, |x| x.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(id: &str, words: &[(&str, u32)], names: &[&str]) -> (String, Profile) {
        (
            id.to_uppercase(),
            Profile {
                format: crate::profile::FORMAT,
                id: id.into(),
                variant: "Simplified".into(),
                segmenter: "LTP".into(),
                words: words.iter().map(|(a, b)| (a.to_string(), *b)).collect(),
                names: names.iter().map(|s| s.to_string()).collect(),
                chars: None,
            },
        )
    }

    #[test]
    fn library_totals_spread_and_widespread() {
        let books = vec![
            profile("a", &[("我们", 60), ("今天", 30), ("小明", 10)], &["小明"]),
            profile("b", &[("我们", 50), ("明天", 30), ("今天", 20)], &[]),
            profile("c", &[("我们", 40), ("明天", 40), ("苹果", 20)], &[]),
        ];
        let lib = Library::build(false, false, books.clone());
        let s = lib.stats(&lib.known(None), 10);
        assert_eq!(s.vocab, 5);
        assert_eq!(s.running, 300);
        // 我们 in three books, 今天 and 明天 in two, 小明 and 苹果 in one.
        assert_eq!(s.spread, [2, 2, 1]);
        // 我们 150 + 明天 70 = 220 of 300 (73%), + 今天 50 = 270 (90%).
        assert_eq!(s.cover_at, [3, 3, 4, 5]);
        assert_eq!(s.coverage.last().unwrap(), &(5, 100.0));
        // Names are never listed; the widest first, then the most frequent.
        let w: Vec<&str> = s.widespread.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(w, ["我们", "明天", "今天", "苹果"]);
        assert_eq!(
            s.widespread[0],
            WideWord {
                word: "我们".into(),
                shown: "我们".into(),
                books: 3,
                count: 150
            }
        );
        assert_eq!((s.known_words, s.known_running), (0, 0.0));

        let profiles: Vec<Profile> = books.into_iter().map(|b| b.1).collect();
        let c = char_stats(&profiles, None, false);
        assert_eq!(c.total, 600);
        assert_eq!(c.unique, 8); // 我们今天小明苹果
        assert_eq!(c.known, 0);
    }

    /// A Traditional and a Simplified book with the same words have the
    /// same characters under folding, and a character known in one script
    /// is counted once.
    #[test]
    fn chars_fold_across_scripts() {
        let (_, simp) = profile("s", &[("我们", 5), ("说话", 3)], &[]);
        let (_, mut trad) = profile("t", &[("我們", 5), ("說話", 3)], &[]);
        trad.variant = "Traditional".into();
        let both = [simp.clone(), trad];
        let k = crate::known::Settings {
            marked: vec!["说".into()],
            ..Default::default()
        }
        .apply(None, true)
        .unwrap();
        let folded = char_stats(&both, Some(&k), true);
        assert_eq!((folded.unique, folded.known, folded.total), (4, 1, 32));
        assert_eq!(
            folded.unique,
            char_stats(std::slice::from_ref(&simp), None, true).unique
        );
        let apart = char_stats(&both, Some(&k), false);
        // 我们说话 and 們說話.
        assert_eq!((apart.unique, apart.total), (7, 32));
    }

    /// A listed word, marked known as the Words tab does, is known at the
    /// next count: a book's own spelling is kept under folding.
    #[test]
    fn widespread_words_mark_as_counted() {
        let (t, mut trad) = profile("t", &[("我們", 9), ("今天", 3)], &[]);
        trad.variant = "Traditional".into();
        let lib = Library::build(true, false, vec![(t, trad)]);
        let s = lib.stats(&lib.known(None), 10);
        let w = &s.widespread[0];
        assert_eq!((w.word.as_str(), w.shown.as_str()), ("我們", "我們"));
        let marked = crate::known::Settings {
            marked: vec![w.word.clone()],
            ..Default::default()
        };
        let k = marked.apply(None, true).unwrap();
        let s = lib.stats(&lib.known(Some(&k)), 10);
        let left: Vec<&str> = s.widespread.iter().map(|w| w.word.as_str()).collect();
        assert_eq!(left, ["今天"]);
    }
}
