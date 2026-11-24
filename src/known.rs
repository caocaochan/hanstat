//! Words known outside Anki: graded vocabulary levels and imported word lists.
//!
//! Chinese: the HSK 3.0 vocabulary of the 2025 《HSK 考试大纲》
//! (中外语言交流合作中心, in force from July 2026), bundled in
//! `data/HSK30-2025.tsv`; and, ticked separately, the HSK 2.0 levels 1–6 (the
//! 2012 revision of the new HSK lists, Simplified) in `data/HSK20-2012.tsv`,
//! and the TOCFL levels of SC-TOP's 華語八千詞表 (2024-09 edition,
//! Traditional), in `data/TOCFL-2024.tsv`.
//!
//! Books marked read in the Library count too: the most frequent of their
//! words, taken when the book was marked ([`ReadBook`], [`read_selection`]).
//!
//! Only imported lists are given to the segmenter, to join words it split
//! ([`Settings::segmenter_words`]). Words marked with + and the words of books
//! read are the segmenter's own tokens: joining them elsewhere gains little,
//! and each change would mean segmenting every Chinese book again.
//!
//! The ticked levels and the imported lists (copied in, so the source files
//! may move) are saved in `%LOCALAPPDATA%\hanstat\known.json`.

use crate::anki::{self, Knowledge};
use crate::chars;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static HSK_TSV: &str = include_str!("../data/HSK30-2025.tsv");
static HSK2_TSV: &str = include_str!("../data/HSK20-2012.tsv");
static TOCFL_TSV: &str = include_str!("../data/TOCFL-2024.tsv");

/// HSK bands: 1–6, and 7 for the shared 7–9 list.
pub const BANDS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

/// HSK 2.0 levels 1–6 (2012 lists).
pub const HSK2_LEVELS: [u8; 6] = [1, 2, 3, 4, 5, 6];

/// Words introduced at HSK 2.0 `level`.
pub fn hsk2_words(level: u8) -> impl Iterator<Item = &'static str> {
    HSK2.iter()
        .filter(move |(b, _)| *b == level)
        .map(|(_, w)| *w)
}

pub fn hsk2_count(level: u8) -> usize {
    hsk2_words(level).count()
}

/// "HSK 2.0 1–4", "HSK 2.0 1, 3–4".
pub fn hsk2_label(levels: &[u8]) -> String {
    let parts: Vec<String> = runs(levels)
        .into_iter()
        .map(|(a, b)| {
            if a == b {
                a.to_string()
            } else {
                format!("{a}–{b}")
            }
        })
        .collect();
    format!("HSK 2.0 {}", parts.join(", "))
}

/// Sorted, deduplicated `levels` as runs of consecutive numbers (first, last).
fn runs(levels: &[u8]) -> Vec<(u8, u8)> {
    let mut b: Vec<u8> = levels.to_vec();
    b.sort_unstable();
    b.dedup();
    let mut out: Vec<(u8, u8)> = Vec::new();
    for x in b {
        match out.last_mut() {
            Some((_, last)) if *last + 1 == x => *last = x,
            _ => out.push((x, x)),
        }
    }
    out
}

/// TOCFL word-list levels, easiest first: Novice 1–2 (準備級), then Level 1
/// (入門級, band A) to Level 5 (流利級, band C). Level 6 (精通級) has no list
/// of its own; it is tested on Level 5's.
pub const TOCFL_LEVELS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

/// "Novice 1", "Level 1", "Level 5".
pub fn tocfl_level_name(level: u8) -> &'static str {
    match level {
        1 => "Novice 1",
        2 => "Novice 2",
        3 => "Level 1",
        4 => "Level 2",
        5 => "Level 3",
        6 => "Level 4",
        7 => "Level 5",
        _ => "?",
    }
}

/// "Novice", "A", "B", "C".
pub fn tocfl_band(level: u8) -> &'static str {
    match level {
        1 | 2 => "Novice",
        3 | 4 => "A",
        5 | 6 => "B",
        _ => "C",
    }
}

/// Words introduced at TOCFL `level`.
pub fn tocfl_words(level: u8) -> impl Iterator<Item = &'static str> {
    TOCFL
        .iter()
        .filter(move |(b, _)| *b == level)
        .map(|(_, w)| *w)
}

pub fn tocfl_count(level: u8) -> usize {
    tocfl_words(level).count()
}

/// "TOCFL Novice 1–Level 2", "TOCFL Level 1, Level 3–5".
pub fn tocfl_label(levels: &[u8]) -> String {
    let mut b: Vec<u8> = levels.to_vec();
    b.sort_unstable();
    b.dedup();
    let mut parts = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let mut j = i;
        while j + 1 < b.len() && b[j + 1] == b[j] + 1 {
            j += 1;
        }
        let (first, last) = (tocfl_level_name(b[i]), tocfl_level_name(b[j]));
        parts.push(if i == j {
            first.to_owned()
        } else if first.split(' ').next() == last.split(' ').next() {
            // Same kind: "Level 3–5", "Novice 1–2".
            format!("{first}–{}", last.rsplit(' ').next().unwrap())
        } else {
            format!("{first}–{last}")
        });
        i = j + 1;
    }
    format!("TOCFL {}", parts.join(", "))
}

/// `level<TAB>word[<TAB>note]` lines, each word once, at its lowest level.
fn parse_levels(tsv: &'static str) -> Vec<(u8, &'static str)> {
    tsv.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let mut f = l.split('\t');
            let b = f.next().unwrap().parse().expect("level number");
            (b, f.next().expect("level<TAB>word"))
        })
        .collect()
}

/// (band, word), each word once, at the lowest band it is introduced.
static HSK: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(HSK_TSV));
static HSK2: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(HSK2_TSV));
static TOCFL: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(TOCFL_TSV));
/// "HSK 3", "HSK 7–9".
pub fn band_label(band: u8) -> String {
    match band {
        7 => "HSK 7–9".to_owned(),
        b => format!("HSK {b}"),
    }
}

/// Words introduced at HSK `band`.
pub fn hsk_words(band: u8) -> impl Iterator<Item = &'static str> {
    HSK.iter().filter(move |(b, _)| *b == band).map(|(_, w)| *w)
}

pub fn hsk_count(band: u8) -> usize {
    hsk_words(band).count()
}

/// "HSK 1–3", "HSK 1, 3, 5–6", "HSK 1–9".
pub fn bands_label(bands: &[u8]) -> String {
    let mut b: Vec<u8> = bands.to_vec();
    b.sort_unstable();
    b.dedup();
    let num = |x: u8| {
        if x == 7 {
            "9".to_owned()
        } else {
            x.to_string()
        }
    };
    let first = |x: u8| {
        if x == 7 {
            "7–9".to_owned()
        } else {
            x.to_string()
        }
    };
    let mut parts = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let mut j = i;
        while j + 1 < b.len() && b[j + 1] == b[j] + 1 {
            j += 1;
        }
        parts.push(if i == j {
            first(b[i])
        } else {
            format!("{}–{}", b[i], num(b[j]))
        });
        i = j + 1;
    }
    format!("HSK {}", parts.join(", "))
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WordList {
    /// File name shown in the window.
    pub name: String,
    /// Where it was imported from; importing the same file again replaces it.
    pub path: PathBuf,
    pub words: Vec<String>,
}

/// A book marked read in the Library: the words it counts as known.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadBook {
    /// The History entry's id (the SHA-256 of the book's file).
    pub id: String,
    /// The book's title when it was marked, shown once it has left the Library.
    pub title: String,
    /// When it was marked read, in seconds since the Unix epoch.
    pub read: u64,
    /// The share of the words occurring at least `min_count` times that was
    /// taken, most frequent first (see [`read_selection`]).
    pub percent: u8,
    pub min_count: u32,
    /// The words taken, as the book's profile keys them, most frequent first.
    /// Kept as they were, so analysing the book again doesn't change them.
    pub words: Vec<String>,
    /// How many of them were not known yet when the book was marked.
    pub added: usize,
}

/// The words a read book teaches: of its words occurring at least
/// `min_count` times, the most frequent `percent`% (rounded up). `words` is
/// most frequent first, as a profile keeps them.
pub fn read_selection(words: &[(String, u32)], percent: u8, min_count: u32) -> Vec<&(String, u32)> {
    let mut often: Vec<&(String, u32)> = words.iter().filter(|(_, n)| *n >= min_count).collect();
    // Stable: words of equal count keep the profile's order.
    often.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let take = (often.len() * percent.min(100) as usize).div_ceil(100);
    often.truncate(take);
    often
}

/// A book read as [`across_books`] takes it: its word counts and the words
/// it took ([`ReadBook::words`]).
pub type ReadCounts<'a> = (&'a [(String, u32)], &'a [String]);

/// The words books read teach together, beyond what each teaches alone
/// ([`ReadBook::words`]): a word none of them took counts as known once it
/// occurs in two or more of them, `min_count` or more times in all. (Two
/// books, so that the share a book leaves out of its own frequent words,
/// [`read_selection`], stays left out until the word is met elsewhere too.)
/// `books`: each book read, its word counts and the words it took. `fold`:
/// Traditional and Simplified spellings count as one word, given in its
/// Simplified form. Most met first.
pub fn across_books(books: &[ReadCounts<'_>], min_count: u32, fold: bool) -> Vec<String> {
    let taken: HashSet<String> = books
        .iter()
        .flat_map(|(_, t)| t.iter())
        .map(|w| anki::fold(w, fold))
        .collect();
    // Folded word → (meetings, books, spelling).
    let mut met: std::collections::HashMap<String, (u64, u32, String)> = Default::default();
    for (words, _) in books {
        let mut here: HashSet<String> = HashSet::new();
        for (w, n) in words.iter() {
            let k = anki::fold(w, fold);
            if taken.contains(&k) {
                continue;
            }
            let e = met.entry(k.clone()).or_insert((0, 0, k.clone()));
            e.0 += *n as u64;
            if here.insert(k) {
                e.1 += 1;
            }
        }
    }
    let mut out: Vec<(u64, String)> = met
        .into_values()
        .filter(|(n, b, _)| *b >= 2 && *n >= min_count as u64)
        .map(|(n, _, w)| (n, w))
        .collect();
    out.sort_unstable_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    out.into_iter().map(|x| x.1).collect()
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ticked HSK 3.0 bands (see [`BANDS`]). Saved as `hsk` in old files.
    #[serde(alias = "hsk")]
    pub levels: Vec<u8>,
    pub lists: Vec<WordList>,
    /// Words marked known one by one with + in the Words tab, as the book wrote them.
    pub marked: Vec<String>,
    /// Ticked HSK 2.0 levels (see [`HSK2_LEVELS`]), alongside the HSK 3.0 ones.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hsk2: Vec<u8>,
    /// Ticked TOCFL levels (see [`TOCFL_LEVELS`]), alongside the HSK ones.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tocfl: Vec<u8>,
    /// Books marked read in the Library, in the order they were marked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<ReadBook>,
    /// Words the books read teach together ([`across_books`]): kept up to
    /// date by the window as books are marked and unmarked.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub across: Vec<String>,
}

impl Settings {
    /// `known.json`.
    pub fn path() -> PathBuf {
        crate::models::app_dir().join("known.json")
    }

    /// Missing file → nothing known. A corrupt file is reported and ignored.
    pub fn load() -> (Settings, Option<String>) {
        Self::load_from(&Self::path())
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }

    /// Missing → nothing known. An unreadable file is kept aside (never
    /// overwritten by the next save) and reported by the saver.
    fn load_from(p: &Path) -> (Settings, Option<String>) {
        (crate::saver::load_json(p).unwrap_or_default(), None)
    }

    /// Through [`crate::saver`]: in the window, written in the background.
    fn save_to(&self, p: &Path) -> Result<(), String> {
        crate::saver::write(p, serde_json::to_vec_pretty(self).unwrap())
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
            && self.lists.iter().all(|l| l.words.is_empty())
            && self.marked.is_empty()
            && self.hsk2.is_empty()
            && self.tocfl.is_empty()
            && self.read.iter().all(|b| b.words.is_empty())
            && self.across.is_empty()
    }

    /// The book with History id `id`, if it is marked read.
    pub fn read_book(&self, id: &str) -> Option<&ReadBook> {
        self.read.iter().find(|b| b.id == id)
    }

    /// Mark a book read, replacing an earlier mark of the same book.
    pub fn set_read(&mut self, b: ReadBook) {
        match self.read.iter_mut().find(|x| x.id == b.id) {
            Some(x) => *x = b,
            None => self.read.push(b),
        }
    }

    /// Unmark a book; its words stay known only where something else marks them.
    pub fn unset_read(&mut self, id: &str) -> bool {
        let n = self.read.len();
        self.read.retain(|b| b.id != id);
        self.read.len() != n
    }

    pub fn hsk2_on(&self, level: u8) -> bool {
        self.hsk2.contains(&level)
    }

    pub fn set_hsk2(&mut self, level: u8, on: bool) {
        self.hsk2.retain(|b| *b != level);
        if on {
            self.hsk2.push(level);
            self.hsk2.sort_unstable();
        }
    }

    pub fn tocfl_on(&self, level: u8) -> bool {
        self.tocfl.contains(&level)
    }

    pub fn set_tocfl(&mut self, level: u8, on: bool) {
        self.tocfl.retain(|b| *b != level);
        if on {
            self.tocfl.push(level);
            self.tocfl.sort_unstable();
        }
    }

    pub fn is_marked(&self, w: &str) -> bool {
        self.marked.iter().any(|m| m == w)
    }

    /// Mark or unmark one word; false if nothing changed.
    pub fn set_marked(&mut self, w: &str, on: bool) -> bool {
        let had = self.is_marked(w);
        if on && !had {
            self.marked.push(w.to_owned());
        } else if !on && had {
            self.marked.retain(|m| m != w);
        }
        on != had
    }

    pub fn band_on(&self, band: u8) -> bool {
        self.levels.contains(&band)
    }

    pub fn set_band(&mut self, band: u8, on: bool) {
        self.levels.retain(|b| *b != band);
        if on {
            self.levels.push(band);
            self.levels.sort_unstable();
        }
    }

    /// Add a list, replacing one imported from the same file.
    pub fn add_list(&mut self, l: WordList) {
        match self.lists.iter_mut().find(|x| x.path == l.path) {
            Some(x) => *x = l,
            None => self.lists.push(l),
        }
    }

    /// Every word marked known (may repeat).
    pub fn words(&self) -> impl Iterator<Item = &str> {
        let levels = HSK
            .iter()
            .filter(|(b, _)| self.levels.contains(b))
            .map(|(_, w)| *w);
        let hsk2 = HSK2
            .iter()
            .filter(|(b, _)| self.hsk2.contains(b))
            .map(|(_, w)| *w);
        let tocfl = TOCFL
            .iter()
            .filter(|(b, _)| self.tocfl.contains(b))
            .map(|(_, w)| *w);
        levels
            .chain(hsk2)
            .chain(tocfl)
            .chain(
                self.lists
                    .iter()
                    .flat_map(|l| l.words.iter().map(String::as_str)),
            )
            .chain(self.marked.iter().map(String::as_str))
            .chain(
                self.read
                    .iter()
                    .flat_map(|b| b.words.iter().map(String::as_str)),
            )
            .chain(self.across.iter().map(String::as_str))
    }

    /// The words given to the segmenter: the imported lists' (not the levels,
    /// the marked words or the words of books read), sorted and deduplicated
    /// so two sets compare equal.
    pub fn segmenter_words(&self) -> Vec<String> {
        let mut w: Vec<String> = self
            .lists
            .iter()
            .flat_map(|l| l.words.iter())
            .cloned()
            .collect();
        w.sort_unstable();
        w.dedup();
        w
    }

    /// "HSK 1–3 + 2 word lists + 15 marked", "HSK 2.0 1–4", "TOCFL Novice 1–Level 2".
    pub fn label(&self) -> String {
        let n = self.lists.iter().filter(|l| !l.words.is_empty()).count();
        let mut parts = Vec::new();
        if !self.levels.is_empty() {
            parts.push(bands_label(&self.levels));
        }
        if !self.hsk2.is_empty() {
            parts.push(hsk2_label(&self.hsk2));
        }
        if !self.tocfl.is_empty() {
            parts.push(tocfl_label(&self.tocfl));
        }
        match n {
            0 => {}
            1 => parts.push("1 word list".to_owned()),
            n => parts.push(format!("{n} word lists")),
        }
        if !self.marked.is_empty() {
            parts.push(format!("{} marked", self.marked.len()));
        }
        match self.read.len() {
            0 => {}
            1 => parts.push("1 book read".to_owned()),
            n => parts.push(format!("{n} books read")),
        }
        if !self.across.is_empty() {
            parts.push(format!("{} across books", self.across.len()));
        }
        parts.join(" + ")
    }

    /// `base` (Anki, if on) plus these words; `None` if there is neither.
    /// `fold`: match Traditional ↔ Simplified, used when there is no `base`.
    pub fn apply(&self, base: Option<&Knowledge>, fold: bool) -> Option<Knowledge> {
        if self.is_empty() {
            return base.cloned();
        }
        let mut k = match base {
            Some(b) => b.clone(),
            None => Knowledge::empty(fold),
        };
        k.add_listed(self.words(), self.label());
        Some(k)
    }
}

/// Read a word list: one word per line. On each line the first item (split at
/// tabs, commas, semicolons and spaces) that contains Chinese is taken, so
/// exports with pinyin or definitions after the word work; HTML and
/// `[readings]` are dropped as in Anki fields. `#` starts a comment line.
pub fn read_word_list(path: &Path) -> Result<WordList, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let (text, _, _, _) = crate::extract::txt::decode(&bytes, crate::extract::EncodingChoice::Auto);
    let words = parse_word_list(&text);
    if words.is_empty() {
        return Err(format!(
            "No Chinese words found in {}. Expected one word per line.",
            path.display()
        ));
    }
    Ok(WordList {
        name: path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: path.to_owned(),
        words,
    })
}

/// Distinct Chinese words (runs of Han characters), in file order.
pub fn parse_word_list(text: &str) -> Vec<String> {
    let is_main = chars::is_han;
    let mut norm = chars::NormStats::default();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim_start_matches('\u{feff}').trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let Some(item) = l
            .split(|c: char| matches!(c, '\t' | ',' | ';') || c.is_whitespace())
            .find(|t| t.chars().any(is_main))
        else {
            continue;
        };
        let cleaned = anki::clean_field(item, &mut norm);
        for w in cleaned
            .split(|c: char| !is_main(c))
            .filter(|w| !w.is_empty())
        {
            if seen.insert(w.to_owned()) {
                out.push(w.to_owned());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anki::Status;

    #[test]
    fn hsk_data_matches_the_syllabus() {
        // Distinct words per band, after merging homographs (本1/本2) at their
        // first band; the syllabus lists 300/200/500/1000/1600/1800/5600 entries.
        let counts: Vec<usize> = BANDS.iter().map(|&b| hsk_count(b)).collect();
        assert_eq!(counts, [300, 197, 491, 990, 1579, 1777, 5562]);
        assert_eq!(HSK.len(), 10_896);
        let all: HashSet<&str> = HSK.iter().map(|(_, w)| *w).collect();
        assert_eq!(all.len(), HSK.len());
        assert!(all.iter().all(|w| w.chars().all(chars::is_han)));
        assert!(hsk_words(1).any(|w| w == "爸爸"));
    }

    #[test]
    fn tocfl_data_matches_the_list() {
        // 7,517 entries (160/234/347/485/1173/2342/2776) → 7,496 words after
        // splitting variants (你/妳) and optional parts (車(子)), each at its
        // first level, plus Simplified forms folding misses (着 for 著 read zhe).
        let counts: Vec<usize> = TOCFL_LEVELS.iter().map(|&b| tocfl_count(b)).collect();
        assert_eq!(counts, [184, 269, 392, 503, 1128, 2287, 2733]);
        assert_eq!(TOCFL.len(), 7_496);
        let all: HashSet<&str> = TOCFL.iter().map(|(_, w)| *w).collect();
        assert_eq!(all.len(), TOCFL.len());
        assert!(all.iter().all(|w| w.chars().all(chars::is_han)));
        let level = |w: &str| TOCFL.iter().find(|(_, x)| *x == w).map(|(b, _)| *b);
        assert_eq!(level("臺灣"), Some(1));
        assert_eq!(level("台灣"), Some(1));
        assert_eq!(level("車子"), Some(1));
        assert_eq!(level("車"), Some(1));
        // Zhuyin notes dropped: 著(ㄓㄠ)急, 名字(˙ㄗ).
        assert_eq!(level("著急"), Some(5));
        assert_eq!(level("名字"), Some(1));
        assert_eq!(level("古跡"), Some(5));
        // 著 is not in OpenCC's character table: the 着 forms are listed too,
        // but not for 著 read zhù.
        assert_eq!(level("着"), Some(2));
        assert_eq!(level("着急"), Some(5));
        assert_eq!(level("着名"), None);
        assert_eq!(level("借口"), Some(7));
    }

    #[test]
    fn hsk2_data_matches_the_list() {
        // 5,000 entries (150/150/300/600/1300/2500), plus 4 from splitting the
        // paired entries (虽然……但是……), less 9 homographs kept at their first
        // level (得 助词 at 2, 得 助动词 at 4).
        let counts: Vec<usize> = HSK2_LEVELS.iter().map(|&b| hsk2_count(b)).collect();
        assert_eq!(counts, [150, 151, 297, 598, 1300, 2499]);
        assert_eq!(HSK2.len(), 4_995);
        let all: HashSet<&str> = HSK2.iter().map(|(_, w)| *w).collect();
        assert_eq!(all.len(), HSK2.len());
        assert!(all.iter().all(|w| w.chars().all(chars::is_han)));
        let level = |w: &str| HSK2.iter().find(|(_, x)| *x == w).map(|(b, _)| *b);
        assert_eq!(level("爸爸"), Some(1));
        assert_eq!(level("得"), Some(2));
        assert_eq!(level("虽然"), Some(2));
        assert_eq!(level("但是"), Some(2));
        assert_eq!(level("只有"), Some(3));
        // 2012 revision, not the 2010 lists: 饭馆 left level 1, 饭店 came in.
        assert_eq!(level("饭店"), Some(1));
        assert_eq!(level("饭馆"), None);
    }

    #[test]
    fn hsk2_levels_mark_words() {
        let mut s = Settings::default();
        s.set_hsk2(4, true);
        s.set_hsk2(1, true);
        s.set_hsk2(2, true);
        assert_eq!(s.hsk2, [1, 2, 4]);
        assert_eq!(s.label(), "HSK 2.0 1–2, 4");
        s.set_band(1, true);
        s.set_tocfl(1, true);
        assert_eq!(s.label(), "HSK 1 + HSK 2.0 1–2, 4 + TOCFL Novice 1");
        s.levels.clear();
        s.tocfl.clear();
        // 2.0-only words count; Simplified list words match a Traditional book when folding.
        assert_eq!(s.apply(None, false).unwrap().word("宾馆"), Status::Listed);
        let k = s.apply(None, true).unwrap();
        assert_eq!(k.word("賓館"), Status::Listed);
        assert!(!k.word("饭馆").is_known());
        // Saved only when ticked.
        let off = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!off.contains("hsk2"), "{off}");
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.hsk2, [1, 2, 4]);
        assert!(back.levels.is_empty());
        assert_eq!(hsk2_label(&[1, 2, 3, 4, 5, 6]), "HSK 2.0 1–6");
        assert_eq!(hsk2_label(&[3]), "HSK 2.0 3");
    }

    #[test]
    fn tocfl_levels_mark_words() {
        let mut s = Settings::default();
        assert!(s.is_empty());
        s.set_tocfl(3, true);
        s.set_tocfl(1, true);
        s.set_tocfl(2, true);
        assert_eq!(s.tocfl, [1, 2, 3]);
        assert_eq!(s.label(), "TOCFL Novice 1–Level 1");
        s.set_band(1, true);
        assert_eq!(s.label(), "HSK 1 + TOCFL Novice 1–Level 1");
        s.levels.clear();
        // Traditional list words match a Simplified book when folding.
        let k = s.apply(None, true).unwrap();
        assert_eq!(k.word("台湾"), Status::Listed);
        assert_eq!(k.word("臺灣"), Status::Listed);
        assert!(!k.word("著急").is_known());
        assert_eq!(k.word("着"), Status::Listed);
        let k = s.apply(None, false).unwrap();
        assert_eq!(k.word("臺灣"), Status::Listed);
        assert!(!k.word("台湾").is_known());
        // Saved only when ticked.
        let off = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!off.contains("tocfl"), "{off}");
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.tocfl, [1, 2, 3]);
        assert_eq!(tocfl_label(&[3, 5, 6, 7]), "TOCFL Level 1, Level 3–5");
        assert_eq!(tocfl_label(&[1, 2]), "TOCFL Novice 1–2");
        assert_eq!(tocfl_band(7), "C");
    }

    #[test]
    fn settings_persist() {
        let dir = std::env::temp_dir().join(format!("hanstat-known-test-{}", std::process::id()));
        let p = dir.join("sub").join("known.json");
        assert_eq!(Settings::load_from(&p), (Settings::default(), None));
        let mut s = Settings::default();
        s.set_band(3, true);
        s.set_band(1, true);
        s.set_marked("顾九思", true);
        s.add_list(WordList {
            name: "a.txt".into(),
            path: r"C:\lists\a.txt".into(),
            words: vec!["你好".into(), "學習".into()],
        });
        s.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p), (s.clone(), None));
        // Re-importing the same file replaces the list.
        s.add_list(WordList {
            name: "a.txt".into(),
            path: r"C:\lists\a.txt".into(),
            words: vec!["再见".into()],
        });
        assert_eq!(s.lists.len(), 1);
        s.save_to(&p).unwrap();
        assert_eq!(Settings::load_from(&p).0.lists[0].words, ["再见"]);
        // An unreadable file is kept aside, so saving can't lose it.
        std::fs::write(&p, "{not json").unwrap();
        let (bad, _) = Settings::load_from(&p);
        assert!(bad.is_empty());
        assert!(!p.exists());
        let kept = std::fs::read_dir(p.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .find(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("known.json.unreadable-")
            })
            .expect("kept aside");
        assert_eq!(std::fs::read(kept.path()).unwrap(), b"{not json");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_settings_files_load() {
        let s: Settings = serde_json::from_str(r#"{"hsk":[1,2],"lists":[],"marked":[]}"#).unwrap();
        assert_eq!(s.levels, [1, 2]);
    }

    #[test]
    fn band_labels() {
        assert_eq!(bands_label(&[1, 2, 3]), "HSK 1–3");
        assert_eq!(bands_label(&[3, 1]), "HSK 1, 3");
        assert_eq!(bands_label(&[1, 2, 3, 4, 5, 6, 7]), "HSK 1–9");
        assert_eq!(bands_label(&[7]), "HSK 7–9");
        assert_eq!(bands_label(&[5, 6, 7]), "HSK 5–9");
        assert_eq!(band_label(7), "HSK 7–9");
        assert_eq!(band_label(3), "HSK 3");
    }

    #[test]
    fn word_list_lines() {
        let text = "\u{feff}# my list\n你好\n\n爱\tài\tlove\n1. 學習 [xuéxí]\n\"电脑\",diànnǎo\n<b>一路顺风</b>\n你好、再见\nhello\n";
        assert_eq!(
            parse_word_list(text),
            ["你好", "爱", "學習", "电脑", "一路顺风", "再见"]
        );
    }

    #[test]
    fn lists_add_to_anki_without_downgrading() {
        let mut base = Knowledge::empty(true);
        base.words.insert("我".into(), Status::Mature);
        base.words.insert("看".into(), Status::Learning);
        base.chars.insert('我', Status::Mature);
        base.chars.insert('看', Status::Learning);
        let s = Settings {
            levels: vec![],
            marked: vec!["跑步".into(), "我".into()],
            lists: vec![WordList {
                name: "a.txt".into(),
                path: "a.txt".into(),
                words: vec!["我".into(), "看".into(), "學習".into()],
            }],
            ..Default::default()
        };
        let k = s.apply(Some(&base), true).unwrap();
        assert_eq!(k.word("我"), Status::Mature);
        assert_eq!(k.word("看"), Status::Listed);
        // Folded like Anki words: the Simplified book form matches.
        assert_eq!(k.word("学习"), Status::Listed);
        assert_eq!(k.char('习'), Status::Listed);
        assert!(k.word("看").is_known());
        assert_eq!(k.word("跑步"), Status::Listed);
        // 我 is in the list and marked: counted once.
        assert_eq!(k.listed.words, 4);
        assert_eq!(k.listed.new_words, 3);
        assert_eq!(k.listed.new_chars, 5);
        assert_eq!(s.label(), "1 word list + 2 marked");
        // Marking alone is enough to have known words.
        let mut m = Settings::default();
        assert!(m.set_marked("跑步", true) && !m.set_marked("跑步", true));
        assert_eq!(m.apply(None, true).unwrap().word("跑步"), Status::Listed);
        assert!(m.set_marked("跑步", false) && m.is_empty());
        assert!(Settings::default().apply(None, true).is_none());
    }

    fn counts(w: &[(&str, u32)]) -> Vec<(String, u32)> {
        w.iter().map(|(w, n)| (w.to_string(), *n)).collect()
    }

    #[test]
    fn words_across_books() {
        let a = counts(&[
            ("的", 50),
            ("宝剑", 6),
            ("魔法", 12),
            ("灵石", 9),
            ("學習", 4),
        ]);
        let b = counts(&[("的", 40), ("宝剑", 5), ("灵石", 1), ("学习", 7)]);
        let c = counts(&[("魔法", 3)]);
        let ta: Vec<String> = vec!["的".into()];
        let tb: Vec<String> = vec!["的".into()];
        let tc: Vec<String> = Vec::new();
        let books = [(&a[..], &ta[..]), (&b[..], &tb[..])];
        // 宝剑 6 + 5 in two books; 灵石 9 + 1; 魔法 12 in one book only (its
        // own book left it out); 学习 4 + 7 once Traditional and Simplified
        // are one word; 的 was taken.
        assert_eq!(across_books(&books, 10, true), ["学习", "宝剑", "灵石"]);
        assert_eq!(across_books(&books, 10, false), ["宝剑", "灵石"]);
        assert_eq!(across_books(&books, 11, true), ["学习", "宝剑"]);
        assert!(across_books(&books, 12, true).is_empty());
        // Met elsewhere too, 魔法 counts.
        let books3 = [(&a[..], &ta[..]), (&b[..], &tb[..]), (&c[..], &tc[..])];
        assert!(across_books(&books3, 10, true).contains(&"魔法".to_owned()));
        // Unmarking one of the two takes 宝剑 back.
        assert!(across_books(&books[..1], 10, true).is_empty());
        // Words across books count as known.
        let s = Settings {
            across: vec!["宝剑".into()],
            ..Default::default()
        };
        assert!(!s.is_empty());
        assert_eq!(s.apply(None, true).unwrap().word("宝剑"), Status::Listed);
        assert_eq!(s.label(), "1 across books");
    }

    #[test]
    fn read_selection_takes_the_most_frequent_share() {
        let w = counts(&[
            ("的", 50),
            ("他", 40),
            ("说", 12),
            ("走", 10),
            ("跑", 9),
            ("飞", 1),
        ]);
        let pick = |p, m| -> Vec<&str> {
            read_selection(&w, p, m)
                .iter()
                .map(|(w, _)| w.as_str())
                .collect()
        };
        // 4 words occur 10+ times; 90% of 4 is 3.6, rounded up.
        assert_eq!(pick(90, 10), ["的", "他", "说", "走"]);
        assert_eq!(pick(50, 10), ["的", "他"]);
        assert_eq!(pick(100, 1).len(), 6);
        // A sliver of one word still takes it; 0% takes none.
        assert_eq!(pick(1, 10), ["的"]);
        assert!(pick(0, 10).is_empty());
        assert!(pick(100, 100).is_empty());
    }

    #[test]
    fn read_books_count_as_known_but_not_for_the_segmenter() {
        let mut s = Settings::default();
        s.set_marked("顾九思", true);
        s.set_read(ReadBook {
            id: "abc".into(),
            title: "长风渡".into(),
            words: vec!["我们".into(), "學習".into()],
            percent: 90,
            min_count: 10,
            ..Default::default()
        });
        assert!(s.read_book("abc").is_some() && s.read_book("x").is_none());
        let k = s.apply(None, true).unwrap();
        assert_eq!(k.word("我们"), Status::Listed);
        assert_eq!(k.word("学习"), Status::Listed);
        // Neither marked words nor books read go to the segmenter; lists do.
        assert!(s.segmenter_words().is_empty());
        s.add_list(WordList {
            name: "a.txt".into(),
            path: "a.txt".into(),
            words: vec!["学习".into(), "你好".into(), "学习".into()],
        });
        assert_eq!(s.segmenter_words(), ["你好", "学习"]);
        s.lists.clear();
        assert_eq!(s.label(), "1 marked + 1 book read");
        // Marking the same book again replaces it.
        s.set_read(ReadBook {
            id: "abc".into(),
            words: vec!["他们".into()],
            ..Default::default()
        });
        assert_eq!(s.read.len(), 1);
        let k = s.apply(None, true).unwrap();
        assert!(k.word("他们").is_known() && !k.word("我们").is_known());
        // Unmarked: its words are no longer known.
        assert!(s.unset_read("abc") && !s.unset_read("abc"));
        s.set_marked("顾九思", false);
        assert!(s.is_empty());
        // Saved and read back.
        s.set_read(ReadBook {
            id: "j".into(),
            words: vec!["学习".into()],
            ..Default::default()
        });
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
