//! A Library row: what is recorded of a book when it is analysed. Part of
//! `HANSTAT_LIBRARY_FINGERPRINT` (build.rs): changing what a row holds, or
//! how it is made from a report, analyses the Library's books again.

use super::Known;
use crate::analyze::Report;
use crate::lang::Language;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    /// SHA-256 of the file, lowercase hex.
    pub id: String,
    pub path: PathBuf,
    /// The book's own title, or the file name without extension.
    pub title: String,
    /// Seconds since the Unix epoch.
    pub analysed: u64,
    pub stats: Stats,
    /// Which of the book's sections were included, in order; `None` for an
    /// entry recorded before they were kept (the book's default sections).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sections: Option<Vec<bool>>,
    /// What else the row was computed from: the build, the user dictionary,
    /// the known words and the settings for its language (`App::lang_inputs`
    /// in the window). Empty if unknown. A row whose inputs differ from the
    /// current ones is analysed again in the background.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub inputs: String,
}

impl Entry {
    /// The sections saved for this book, if they fit a book of `n` sections.
    pub fn sections_for(&self, n: usize) -> Option<&[bool]> {
        self.sections.as_deref().filter(|s| s.len() == n)
    }
}

/// Everything shown for a book; an entry is only rewritten when these change
/// (or the book is analysed again).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stats {
    pub format: String,
    pub language: Language,
    /// Simplified, Traditional, Mixed or Unknown (Chinese only).
    pub variant: String,
    pub segmenter: String,
    pub sections_included: usize,
    pub sections_total: usize,
    /// Chinese characters, or kanji in a Japanese book; zero for Korean.
    pub han_total: u64,
    pub han_unique: u64,
    /// Japanese: kana, counted with the kanji for reading time.
    #[serde(default)]
    pub kana_total: u64,
    pub words_total: u64,
    pub words_unique: u64,
    /// Most frequent words needed to cover 98% of the running words.
    pub words_for_98: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub known: Option<Known>,
}

impl Stats {
    /// From the report the Overview shows.
    pub fn from_report(r: &Report) -> Stats {
        let known = r.anki.as_ref().map(Known::from_coverage);
        Stats {
            format: r.format.to_owned(),
            language: r.language,
            variant: format!("{:?}", r.variant),
            segmenter: r.segmenter.label().to_owned(),
            sections_included: r.sections.iter().filter(|s| s.included).count(),
            sections_total: r.sections.len(),
            han_total: r.han_total,
            han_unique: r.han_unique as u64,
            kana_total: r.kana_total,
            words_total: r.words_total,
            words_unique: r.words_unique as u64,
            words_for_98: r
                .word_coverage
                .iter()
                .find(|c| c.percent == 98.0)
                .map_or(0, |c| c.items as u64),
            known,
        }
    }
}
