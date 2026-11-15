//! Words known outside Anki: HSK 3.0 bands and imported word lists.
//!
//! The HSK vocabulary is the 2025 《HSK 考试大纲》 (中外语言交流合作中心,
//! in force from July 2026), bundled in `data/HSK30-2025.tsv`. The choice of
//! bands and the imported lists (copied in, so the source files may move) are
//! saved per language: `%LOCALAPPDATA%\hanstat\known.json` for Chinese and
//! `known-ko.json` for Korean (word lists and marked words only).

use crate::anki::{self, Knowledge};
use crate::chars;
use crate::lang::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

static HSK_TSV: &str = include_str!("../data/HSK30-2025.tsv");

/// HSK bands: 1–6, and 7 for the shared 7–9 list.
pub const BANDS: [u8; 7] = [1, 2, 3, 4, 5, 6, 7];

/// (band, word), each word once, at the lowest band it is introduced.
static HSK: LazyLock<Vec<(u8, &'static str)>> = LazyLock::new(|| {
    HSK_TSV
        .lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let (b, w) = l.split_once('\t').expect("band<TAB>word");
            (b.parse().expect("band number"), w)
        })
        .collect()
});

pub fn band_label(band: u8) -> &'static str {
    match band {
        1 => "HSK 1",
        2 => "HSK 2",
        3 => "HSK 3",
        4 => "HSK 4",
        5 => "HSK 5",
        6 => "HSK 6",
        _ => "HSK 7–9",
    }
}

/// Words introduced at `band`.
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

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Ticked HSK bands (see [`BANDS`]).
    pub hsk: Vec<u8>,
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
        self.hsk.is_empty()
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
        self.hsk.contains(&band)
    }

    pub fn set_band(&mut self, band: u8, on: bool) {
        self.hsk.retain(|b| *b != band);
        if on {
            self.hsk.push(band);
            self.hsk.sort_unstable();
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
        let hsk = HSK
            .iter()
            .filter(|(b, _)| self.hsk.contains(b))
            .map(|(_, w)| *w);
        hsk.chain(
            self.lists
                .iter()
                .flat_map(|l| l.words.iter().map(String::as_str)),
        )
        .chain(self.marked.iter().map(String::as_str))
    }

    /// "HSK 1–3 + 2 word lists + 15 marked".
    pub fn label(&self) -> String {
        let n = self.lists.iter().filter(|l| !l.words.is_empty()).count();
        let mut parts = Vec::new();
        if !self.hsk.is_empty() {
            parts.push(bands_label(&self.hsk));
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
    fn band_labels() {
        assert_eq!(bands_label(&[1, 2, 3]), "HSK 1–3");
        assert_eq!(bands_label(&[3, 1]), "HSK 1, 3");
        assert_eq!(bands_label(&[1, 2, 3, 4, 5, 6, 7]), "HSK 1–9");
        assert_eq!(bands_label(&[7]), "HSK 7–9");
        assert_eq!(bands_label(&[5, 6, 7]), "HSK 5–9");
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
            hsk: vec![],
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
