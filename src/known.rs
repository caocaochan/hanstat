//! Words known outside Anki: graded vocabulary levels and imported word lists.
//!
//! Chinese: the HSK 3.0 vocabulary of the 2025 《HSK 考试大纲》
//! (中外语言交流合作中心, in force from July 2026), bundled in
//! `data/HSK30-2025.tsv`.
//!
//! Korean: levels 1급–6급 of 국립국어원's 국제 통용 한국어 표준 교육과정
//! vocabulary, bundled in `data/KO-standard-2020.tsv`. TOPIK has no official
//! word list; this is the list TOPIK level vocabularies are built from. It
//! lists most 하다/되다 verbs only through their noun plus the 2급 affixes
//! -하다/-되다, so a book's X하다 or X되다 counts as known when X is listed
//! at a ticked level, or at a lower one if 2급 is ticked (see
//! [`Settings::apply`]).
//!
//! The ticked levels and the imported lists (copied in, so the source files
//! may move) are saved per language: `%LOCALAPPDATA%\hanstat\known.json` for
//! Chinese and `known-ko.json` for Korean.

use crate::anki::{self, Knowledge};
use crate::chars;
use crate::lang::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static HSK_TSV: &str = include_str!("../data/HSK30-2025.tsv");
static KO_TSV: &str = include_str!("../data/KO-standard-2020.tsv");

/// HSK bands: 1–6, and 7 for the shared 7–9 list.
pub const BANDS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

/// Korean levels 1급–6급.
pub const KO_LEVELS: [u8; 6] = [1, 2, 3, 4, 5, 6];

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
static KO: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| parse_levels(KO_TSV));

fn table(lang: Language) -> &'static [(u8, &'static str)] {
    match lang {
        Language::Chinese => &HSK,
        Language::Korean => &KO,
    }
}

/// The levels that can be ticked in `lang`.
pub fn bands(lang: Language) -> &'static [u8] {
    match lang {
        Language::Chinese => &BANDS,
        Language::Korean => &KO_LEVELS,
    }
}

/// "HSK 3", "HSK 7–9", "3급".
pub fn band_label(lang: Language, band: u8) -> String {
    match (lang, band) {
        (Language::Chinese, 7) => "HSK 7–9".to_owned(),
        (Language::Chinese, b) => format!("HSK {b}"),
        (Language::Korean, b) => format!("{b}급"),
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

/// "HSK 1–3", "HSK 1, 3, 5–6", "HSK 1–9"; Korean "1–3급", "1, 3, 5–6급".
pub fn bands_label(lang: Language, bands: &[u8]) -> String {
    let mut b: Vec<u8> = bands.to_vec();
    b.sort_unstable();
    b.dedup();
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
    /// Ticked levels (see [`bands`]): HSK bands for Chinese, 급 for Korean.
    /// Saved as `hsk` before Korean levels existed.
    #[serde(alias = "hsk")]
    pub levels: Vec<u8>,
    pub lists: Vec<WordList>,
    /// Words marked known one by one with + in the Words tab, as the book wrote them.
    pub marked: Vec<String>,
    /// Which language's file these settings belong to (not stored in it).
    #[serde(skip)]
    pub language: Language,
}

impl Settings {
    /// The Chinese file (`known.json`).
    pub fn path() -> PathBuf {
        Self::path_for(Language::Chinese)
    }

    /// `known.json` (Chinese) or `known-ko.json` (Korean).
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
        levels
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

    /// "HSK 1–3 + 2 word lists + 15 marked".
    pub fn label(&self) -> String {
        let n = self.lists.iter().filter(|l| !l.words.is_empty()).count();
        let mut parts = Vec::new();
        if !self.levels.is_empty() {
            parts.push(bands_label(self.language, &self.levels));
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
        k.hada_stems = self.hada_stems().into_iter().map(str::to_owned).collect();
        Some(k)
    }
}

/// Read a Chinese word list (see [`read_word_list_in`]).
pub fn read_word_list(path: &Path) -> Result<WordList, String> {
    read_word_list_in(path, Language::Chinese)
}

/// Read a word list: one word per line. On each line the first item (split at
/// tabs, commas, semicolons and spaces) that contains Chinese (Korean: Hangul)
/// is taken, so exports with pinyin or definitions after the word work; HTML
/// and `[readings]` are dropped as in Anki fields. `#` starts a comment line.
/// Korean words should be dictionary forms (먹다, 공부하다), as the book's words are.
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

/// Distinct words in `lang` (runs of Han characters, or of Hangul syllables), in file order.
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
