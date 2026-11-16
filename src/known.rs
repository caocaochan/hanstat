//! Words known outside Anki: graded vocabulary levels and imported word lists.
//!
//! Chinese: the HSK 3.0 vocabulary of the 2025 《HSK 考试大纲》
//! (中外语言交流合作中心, in force from July 2026), bundled in
//! `data/HSK30-2025.tsv`; and, ticked separately, the TOCFL levels of SC-TOP's
//! 華語八千詞表 (2024-09 edition, Traditional), in `data/TOCFL-2024.tsv`.
//!
//! Korean: levels 1급–6급 of 국립국어원's 국제 통용 한국어 표준 교육과정
//! vocabulary, bundled in `data/KO-standard-2020.tsv`. TOPIK has no official
//! word list; this is the list TOPIK level vocabularies are built from. It
//! lists most 하다/되다 verbs only through their noun plus the 2급 affixes
//! -하다/-되다, so a book's X하다 or X되다 counts as known when X is listed
//! at a ticked level, or at a lower one if 2급 is ticked (see
//! [`Settings::apply`]).
//!
//! Japanese: JLPT N5–N1 from Jonathan Waller's lists (via
//! stephenmk/yomitan-jlpt-vocab), bundled in `data/JLPT-waller.tsv`. The JLPT
//! publishes no word list; these are the lists jisho.org uses. Optionally the
//! words of the Kaishi 1.5k Anki deck (`data/Kaishi-1.5k.tsv`), for readers who
//! studied it outside the collection hanstat reads. Entries are keyed by
//! Sudachi's normalized form, as the book's words are, and imported Japanese
//! lists are run through Sudachi the same way (see [`Settings::apply`]).
//!
//! The ticked levels and the imported lists (copied in, so the source files
//! may move) are saved per language: `%LOCALAPPDATA%\hanstat\known.json` for
//! Chinese, `known-ko.json` for Korean and `known-ja.json` for Japanese.

use crate::anki::{self, Knowledge};
use crate::chars;
use crate::lang::Language;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static HSK_TSV: &str = include_str!("../data/HSK30-2025.tsv");
static TOCFL_TSV: &str = include_str!("../data/TOCFL-2024.tsv");
static KO_TSV: &str = include_str!("../data/KO-standard-2020.tsv");
static JLPT_TSV: &str = include_str!("../data/JLPT-waller.tsv");
static KAISHI_TSV: &str = include_str!("../data/Kaishi-1.5k.tsv");

/// The Kaishi 1.5k deck's words as (key, headword), in deck order.
static KAISHI: LazyLock<Vec<(&'static str, &'static str)>> = LazyLock::new(|| {
    KAISHI_TSV
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.split_once('\t').expect("key<TAB>headword"))
        .collect()
});

/// The name shown for the Kaishi deck.
pub const KAISHI_NAME: &str = "Kaishi 1.5k";

/// Distinct words (keys) of the Kaishi 1.5k deck.
pub fn kaishi_count() -> usize {
    KAISHI.len()
}

/// HSK bands: 1–6, and 7 for the shared 7–9 list.
pub const BANDS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

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

/// Korean levels 1급–6급.
pub const KO_LEVELS: [u8; 6] = [1, 2, 3, 4, 5, 6];

/// JLPT levels, easiest first: 5 is N5.
pub const JLPT_LEVELS: [u8; 5] = [5, 4, 3, 2, 1];

/// The level at which the Korean affixes -하다 and -되다 are introduced.
const KO_HADA_LEVEL: u8 = 2;

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
static TOCFL: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(TOCFL_TSV));
static KO: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(KO_TSV));
static JLPT: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(JLPT_TSV));
/// JLPT key → the list's headword (会う, 為る → する), for the kanji it teaches.
static JLPT_HEAD: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    JLPT_TSV
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t').skip(1);
            Some((f.next()?, f.next()?))
        })
        .collect()
});

fn table(lang: Language) -> &'static [(u8, &'static str)] {
    match lang {
        Language::Chinese => &HSK,
        Language::Korean => &KO,
        Language::Japanese => &JLPT,
    }
}

/// The levels that can be ticked in `lang`.
pub fn bands(lang: Language) -> &'static [u8] {
    match lang {
        Language::Chinese => &BANDS,
        Language::Korean => &KO_LEVELS,
        Language::Japanese => &JLPT_LEVELS,
    }
}

/// "HSK 3", "HSK 7–9", "3급", "N5".
pub fn band_label(lang: Language, band: u8) -> String {
    match (lang, band) {
        (Language::Chinese, 7) => "HSK 7–9".to_owned(),
        (Language::Chinese, b) => format!("HSK {b}"),
        (Language::Korean, b) => format!("{b}급"),
        (Language::Japanese, b) => format!("N{b}"),
    }
}

/// Words introduced at `band` in `lang`.
pub fn band_words(lang: Language, band: u8) -> impl Iterator<Item = &'static str> {
    table(lang)
        .iter()
        .filter(move |(b, _)| *b == band)
        .map(|(_, w)| *w)
}

pub fn band_count(lang: Language, band: u8) -> usize {
    band_words(lang, band).count()
}

/// Words introduced at HSK `band`.
pub fn hsk_words(band: u8) -> impl Iterator<Item = &'static str> {
    band_words(Language::Chinese, band)
}

pub fn hsk_count(band: u8) -> usize {
    band_count(Language::Chinese, band)
}

/// "HSK 1–3", "HSK 1, 3, 5–6", "HSK 1–9"; Korean "1–3급", "1, 3, 5–6급";
/// Japanese "N5–N3", "N5, N3".
pub fn bands_label(lang: Language, bands: &[u8]) -> String {
    let mut b: Vec<u8> = bands.to_vec();
    b.sort_unstable();
    b.dedup();
    if lang == Language::Japanese {
        // Easiest (N5) first.
        b.reverse();
        let mut parts = Vec::new();
        let mut i = 0;
        while i < b.len() {
            let mut j = i;
            while j + 1 < b.len() && b[j + 1] + 1 == b[j] {
                j += 1;
            }
            parts.push(if i == j {
                format!("N{}", b[i])
            } else {
                format!("N{}–N{}", b[i], b[j])
            });
            i = j + 1;
        }
        return parts.join(", ");
    }
    let hsk_top = lang == Language::Chinese;
    let num = |x: u8| {
        if hsk_top && x == 7 {
            "9".to_owned()
        } else {
            x.to_string()
        }
    };
    let first = |x: u8| {
        if hsk_top && x == 7 {
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
    match lang {
        Language::Chinese => format!("HSK {}", parts.join(", ")),
        Language::Korean => format!("{}급", parts.join(", ")),
        Language::Japanese => unreachable!("handled above"),
    }
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ticked levels (see [`bands`]): HSK bands for Chinese, 급 for Korean, N for Japanese.
    /// Saved as `hsk` before Korean levels existed.
    #[serde(alias = "hsk")]
    pub levels: Vec<u8>,
    pub lists: Vec<WordList>,
    /// Words marked known one by one with + in the Words tab, as the book wrote them.
    pub marked: Vec<String>,
    /// Japanese: the words of the Kaishi 1.5k Anki deck count as known.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub kaishi: bool,
    /// Chinese: ticked TOCFL levels (see [`TOCFL_LEVELS`]), alongside the HSK ones.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tocfl: Vec<u8>,
    /// Which language's file these settings belong to (not stored in it).
    #[serde(skip)]
    pub language: Language,
}

impl Settings {
    /// The Chinese file (`known.json`).
    pub fn path() -> PathBuf {
        Self::path_for(Language::Chinese)
    }

    /// `known.json` (Chinese), `known-ko.json` (Korean) or `known-ja.json` (Japanese).
    pub fn path_for(lang: Language) -> PathBuf {
        lang.settings_path("known")
    }

    /// The Chinese settings. Missing file → nothing known. A corrupt file is reported and ignored.
    pub fn load() -> (Settings, Option<String>) {
        Self::load_for(Language::Chinese)
    }

    pub fn load_for(lang: Language) -> (Settings, Option<String>) {
        let (mut s, warn) = Self::load_from(&Self::path_for(lang));
        s.language = lang;
        (s, warn)
    }

    /// Saved to this language's file.
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path_for(self.language))
    }

    fn load_from(p: &Path) -> (Settings, Option<String>) {
        match std::fs::read(p) {
            Err(_) => (Settings::default(), None),
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(s) => (s, None),
                Err(e) => (
                    Settings::default(),
                    Some(format!("Ignoring unreadable {}: {e}", p.display())),
                ),
            },
        }
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a file.
    fn save_to(&self, p: &Path) -> Result<(), String> {
        let dir = p.parent().unwrap();
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let tmp = p.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, p).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Cannot write {}: {e}", p.display())
        })
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
            && self.lists.iter().all(|l| l.words.is_empty())
            && self.marked.is_empty()
            && !self.kaishi_on()
            && self.tocfl_levels().is_empty()
    }

    /// The Kaishi 1.5k words are marked known (Japanese only).
    pub fn kaishi_on(&self) -> bool {
        self.kaishi && self.language == Language::Japanese
    }

    /// The ticked TOCFL levels that apply (Chinese only).
    pub fn tocfl_levels(&self) -> &[u8] {
        if self.language == Language::Chinese {
            &self.tocfl
        } else {
            &[]
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
        let levels = table(self.language)
            .iter()
            .filter(|(b, _)| self.levels.contains(b))
            .map(|(_, w)| *w);
        let tocfl = TOCFL
            .iter()
            .filter(|(b, _)| self.tocfl_levels().contains(b))
            .map(|(_, w)| *w);
        levels
            .chain(tocfl)
            .chain(
                self.lists
                    .iter()
                    .flat_map(|l| l.words.iter().map(String::as_str)),
            )
            .chain(self.marked.iter().map(String::as_str))
    }

    /// Korean words X whose X하다 and X되다 count as known: X is at a level L
    /// such that the later of L and 2급 (where -하다/-되다 are taught) is
    /// ticked. Empty for Chinese.
    pub fn hada_stems(&self) -> HashSet<&'static str> {
        if self.language != Language::Korean {
            return HashSet::new();
        }
        KO.iter()
            .filter(|(b, _)| self.levels.contains(&(*b).max(KO_HADA_LEVEL)))
            .map(|(_, w)| *w)
            .collect()
    }

    /// The words given to the segmenter (imported lists and marked words, not
    /// the levels), sorted and deduplicated so two sets compare equal.
    pub fn segmenter_words(&self) -> Vec<String> {
        let mut w: Vec<String> = self
            .lists
            .iter()
            .flat_map(|l| l.words.iter())
            .chain(&self.marked)
            .cloned()
            .collect();
        w.sort_unstable();
        w.dedup();
        w
    }

    /// "HSK 1–3 + 2 word lists + 15 marked", "TOCFL Novice 1–A2", "N5–N4 + Kaishi 1.5k".
    pub fn label(&self) -> String {
        let n = self.lists.iter().filter(|l| !l.words.is_empty()).count();
        let mut parts = Vec::new();
        if !self.levels.is_empty() {
            parts.push(bands_label(self.language, &self.levels));
        }
        if !self.tocfl_levels().is_empty() {
            parts.push(tocfl_label(self.tocfl_levels()));
        }
        if self.kaishi_on() {
            parts.push(KAISHI_NAME.to_owned());
        }
        match n {
            0 => {}
            1 => parts.push("1 word list".to_owned()),
            n => parts.push(format!("{n} word lists")),
        }
        if !self.marked.is_empty() {
            parts.push(format!("{} marked", self.marked.len()));
        }
        parts.join(" + ")
    }

    /// `base` (Anki, if on) plus these words; `None` if there is neither.
    /// `fold`: match Traditional ↔ Simplified, used when there is no `base`
    /// (Chinese only).
    pub fn apply(&self, base: Option<&Knowledge>, fold: bool) -> Option<Knowledge> {
        if self.is_empty() {
            return base.cloned();
        }
        let mut k = match base {
            Some(b) => b.clone(),
            None => Knowledge::empty(fold && self.language == Language::Chinese),
        };
        if self.language == Language::Japanese {
            let words = self.japanese_words();
            k.add_listed_spelled(
                words.iter().map(|(w, s)| (w.as_str(), s.as_str())),
                self.label(),
            );
        } else {
            k.add_listed(self.words(), self.label());
        }
        k.hada_stems = self.hada_stems().into_iter().map(str::to_owned).collect();
        Some(k)
    }

    /// Japanese: every word marked known as (key, spelling). JLPT and Kaishi
    /// entries are keyed already; list entries are run through Sudachi, so 分かる, わかる
    /// and わかった all mark 分かる (each word of an entry such as 勉強する
    /// counts); marked words are the book's own keys. The spelling gives the
    /// kanji a word teaches (する, not its key 為る).
    fn japanese_words(&self) -> Vec<(String, String)> {
        let mut out: Vec<(String, String)> = JLPT
            .iter()
            .filter(|(b, _)| self.levels.contains(b))
            .map(|(_, w)| (w.to_string(), JLPT_HEAD.get(w).unwrap_or(w).to_string()))
            .collect();
        if self.kaishi_on() {
            out.extend(KAISHI.iter().map(|(k, h)| (k.to_string(), h.to_string())));
        }
        let listed: Vec<&str> = self
            .lists
            .iter()
            .flat_map(|l| l.words.iter().map(String::as_str))
            .collect();
        for (w, keys) in listed.iter().zip(crate::japanese::keys_each(&listed)) {
            out.extend(keys.into_iter().map(|k| (k, w.to_string())));
        }
        out.extend(self.marked.iter().map(|w| (w.clone(), w.clone())));
        out
    }
}

/// Read a Chinese word list (see [`read_word_list_in`]).
pub fn read_word_list(path: &Path) -> Result<WordList, String> {
    read_word_list_in(path, Language::Chinese)
}

/// Read a word list: one word per line. On each line the first item (split at
/// tabs, commas, semicolons and spaces) that contains Chinese (Korean: Hangul;
/// Japanese: kana or kanji) is taken, so exports with pinyin or definitions
/// after the word work; HTML and `[readings]` are dropped as in Anki fields.
/// `#` starts a comment line. Korean words should be dictionary forms (먹다,
/// 공부하다), as the book's words are; Japanese words may be in any form.
pub fn read_word_list_in(path: &Path, lang: Language) -> Result<WordList, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))?;
    let (text, _, _, _) = crate::extract::txt::decode(&bytes, crate::extract::EncodingChoice::Auto);
    let words = parse_word_list_in(&text, lang);
    if words.is_empty() {
        return Err(format!(
            "No {} words found in {}. Expected one word per line.",
            lang.name(),
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

/// Distinct Chinese words, in file order.
pub fn parse_word_list(text: &str) -> Vec<String> {
    parse_word_list_in(text, Language::Chinese)
}

/// Distinct words in `lang` (runs of Han characters, of Hangul syllables, or
/// of kana and kanji), in file order.
pub fn parse_word_list_in(text: &str, lang: Language) -> Vec<String> {
    let is_main = |c: char| chars::is_main(c, lang);
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
        // Saved only when ticked, and only Chinese uses it.
        let off = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!off.contains("tocfl"), "{off}");
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.tocfl, [1, 2, 3]);
        let ko = Settings {
            tocfl: vec![1],
            language: Language::Korean,
            ..Default::default()
        };
        assert!(ko.is_empty() && ko.label().is_empty());
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
        std::fs::write(&p, "{not json").unwrap();
        let (bad, warn) = Settings::load_from(&p);
        assert!(bad.is_empty() && warn.is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn korean_data_matches_the_list() {
        // 10,635 entries (735/1100/1655/2200/2365/2580) → 9,962 distinct words
        // after splitting variant pairs, merging homographs and dropping
        // affixes, plus 15 lemmas from the grammar sheet (6 more lowered).
        let counts: Vec<usize> = KO_LEVELS
            .iter()
            .map(|&b| band_count(Language::Korean, b))
            .collect();
        assert_eq!(counts, [716, 1068, 1528, 2067, 2130, 2468]);
        assert_eq!(KO.len(), 9_977);
        let all: HashSet<&str> = KO.iter().map(|(_, w)| *w).collect();
        assert_eq!(all.len(), KO.len());
        assert!(all.iter().all(|w| w.chars().all(chars::is_hangul)));
        let level = |w: &str| KO.iter().find(|(_, x)| *x == w).map(|(b, _)| *b);
        assert_eq!(level("가다"), Some(1));
        assert_eq!(level("싶다"), Some(1)); // -고 싶다
        assert_eq!(level("않다"), Some(1)); // -지 않다, 3급 as a vocabulary item
        assert_eq!(level("때문"), Some(2));
        assert_eq!(level("말하다"), None); // only 말 + -하다
    }

    #[test]
    fn korean_hada_verbs_follow_their_noun() {
        let mut s = Settings {
            language: Language::Korean,
            ..Default::default()
        };
        s.set_band(1, true);
        // 말 and 생각 are 1급, but -하다 is taught at 2급.
        let k = s.apply(None, false).unwrap();
        assert_eq!(k.word("말"), Status::Listed);
        assert!(!k.word("말하다").is_known());
        s.set_band(2, true);
        let k = s.apply(None, false).unwrap();
        assert_eq!(k.word("말하다"), Status::Listed);
        assert_eq!(k.word("생각되다"), Status::Listed);
        // Not for words outside the ticked levels, nor in Chinese.
        assert!(!k.word("몽유병하다").is_known());
        assert!(s.label().starts_with("1–2급"));
        let mut zh = Settings::default();
        zh.set_band(1, true);
        assert!(zh.hada_stems().is_empty());
        assert_eq!(zh.label(), "HSK 1");
    }

    #[test]
    fn old_settings_files_load() {
        let s: Settings = serde_json::from_str(r#"{"hsk":[1,2],"lists":[],"marked":[]}"#).unwrap();
        assert_eq!(s.levels, [1, 2]);
    }

    #[test]
    fn jlpt_data() {
        let ja = Language::Japanese;
        let counts: Vec<usize> = JLPT_LEVELS.iter().map(|&b| band_count(ja, b)).collect();
        assert_eq!(counts, [639, 604, 1579, 1679, 2951]);
        assert_eq!(JLPT.len(), 7_452);
        let all: HashSet<&str> = JLPT.iter().map(|(_, w)| *w).collect();
        assert_eq!(all.len(), JLPT.len());
        let level = |w: &str| JLPT.iter().find(|(_, x)| *x == w).map(|(b, _)| *b);
        assert_eq!(level("会う"), Some(5));
        // Keys are Sudachi's normalized forms: する is 為る, すぐ is 直ぐ.
        assert_eq!(level("為る"), Some(5));
        assert_eq!(level("直ぐ"), Some(5));
        assert_eq!(JLPT_HEAD.get("直ぐ"), Some(&"すぐに"));
        // お菓子 is split by Sudachi into お + 菓子, and keyed by 菓子.
        assert_eq!(level("菓子"), Some(5));
    }

    #[test]
    fn kaishi_deck() {
        assert_eq!(kaishi_count(), 1_396);
        let keys: HashSet<&str> = KAISHI.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len(), KAISHI.len());
        // First cards of the deck; keys are normalized forms, headwords as the deck writes them.
        assert_eq!(KAISHI[0], ("私", "私"));
        assert_eq!(KAISHI[1], ("貴方", "あなた"));
        let mut s = Settings {
            language: Language::Japanese,
            ..Default::default()
        };
        assert!(s.is_empty());
        s.kaishi = true;
        assert!(!s.is_empty());
        let k = s.apply(None, false).unwrap();
        assert_eq!(k.word("貴方"), Status::Listed);
        assert_eq!(k.word("日本語"), Status::Listed);
        assert_eq!(k.listed.words, 1_396);
        // Kanji from the deck's spelling: これ is keyed 此れ, but 此 is not taught.
        assert_eq!(k.char('語'), Status::Listed);
        assert_eq!(k.word("此れ"), Status::Listed);
        assert!(!k.char('此').is_known());
        s.set_band(5, true);
        assert_eq!(s.label(), "N5 + Kaishi 1.5k");
        // Only in Japanese, and not written to a file when off.
        let zh = Settings {
            kaishi: true,
            ..Default::default()
        };
        assert!(zh.is_empty() && zh.label().is_empty());
        let off = serde_json::to_string(&Settings::default()).unwrap();
        assert!(!off.contains("kaishi"), "{off}");
        let on: Settings = serde_json::from_str(r#"{"kaishi":true}"#).unwrap();
        assert!(on.kaishi);
    }

    #[test]
    fn japanese_levels_mark_keys() {
        let mut s = Settings {
            language: Language::Japanese,
            ..Default::default()
        };
        s.set_band(5, true);
        s.set_marked("冒険者", true);
        let k = s.apply(None, true).unwrap();
        assert_eq!(k.word("会う"), Status::Listed);
        assert_eq!(k.word("冒険者"), Status::Listed);
        assert!(!k.word("冒険").is_known());
        // The kanji of the spelling, not of the key 為る.
        assert_eq!(k.char('会'), Status::Listed);
        assert!(!k.fold);
        assert_eq!(s.label(), "N5 + 1 marked");
    }

    #[test]
    fn band_labels() {
        let zh = Language::Chinese;
        assert_eq!(bands_label(zh, &[1, 2, 3]), "HSK 1–3");
        assert_eq!(bands_label(zh, &[3, 1]), "HSK 1, 3");
        assert_eq!(bands_label(zh, &[1, 2, 3, 4, 5, 6, 7]), "HSK 1–9");
        assert_eq!(bands_label(zh, &[7]), "HSK 7–9");
        assert_eq!(bands_label(zh, &[5, 6, 7]), "HSK 5–9");
        let ko = Language::Korean;
        assert_eq!(bands_label(ko, &[1, 2, 3]), "1–3급");
        assert_eq!(bands_label(ko, &[1, 3, 5, 6]), "1, 3, 5–6급");
        assert_eq!(band_label(ko, 4), "4급");
        assert_eq!(band_label(zh, 7), "HSK 7–9");
        let ja = Language::Japanese;
        assert_eq!(bands_label(ja, &[5, 4, 3]), "N5–N3");
        assert_eq!(bands_label(ja, &[3, 5]), "N5, N3");
        assert_eq!(bands_label(ja, &[1, 2, 3, 4, 5]), "N5–N1");
        assert_eq!(band_label(ja, 2), "N2");
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
    fn korean_word_list_lines() {
        let text =
            "# 단어장\n먹다\tto eat\n1. 공부하다 [study]\n\"사과\",apple\n漢字\nhello\n먹다\n";
        assert_eq!(
            parse_word_list_in(text, Language::Korean),
            ["먹다", "공부하다", "사과"]
        );
        // The Chinese parser ignores Korean, and the other way round.
        assert_eq!(parse_word_list_in(text, Language::Chinese), ["漢字"]);
    }

    #[test]
    fn japanese_word_list_lines() {
        let text = "# 単語\n会う\tto meet\n分かる・わかる\n食べる[たべる]\n먹다\nhello\n会う\n";
        assert_eq!(
            parse_word_list_in(text, Language::Japanese),
            ["会う", "分かる", "わかる", "食べる"]
        );
    }

    #[test]
    fn settings_files_per_language() {
        let (zh, _) = Settings::load_for(Language::Chinese);
        assert_eq!(zh.language, Language::Chinese);
        assert!(Settings::path_for(Language::Korean).ends_with("known-ko.json"));
        assert!(anki::Settings::path_for(Language::Korean).ends_with("anki-ko.json"));
        assert_eq!(Settings::path(), Settings::path_for(Language::Chinese));
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
}
