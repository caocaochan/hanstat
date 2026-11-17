//! A book's vocabulary, kept for the reading path ([`crate::plan`]): its
//! word counts over the included sections, the names left out of its
//! difficulty, and (Japanese) how its words are spelled and split. Unlike
//! the analysis cache, which is trimmed to a size and keyed by everything the
//! analysis depends on, a profile lasts as long as the book's History entry.
//!
//! Kept in `profiles/<id>.json.gz` next to `history.json`, where `<id>` is
//! the History entry's id (the SHA-256 of the book's file).

use crate::analyze::Report;
use crate::lang::Language;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Layout of a profile; one of another layout is treated as missing.
pub const FORMAT: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub format: u32,
    /// The History entry's id.
    pub id: String,
    pub language: Language,
    /// Simplified, Traditional, Mixed or Unknown (Chinese only).
    pub variant: String,
    /// The segmenter's label, e.g. "LTP perceptron".
    pub segmenter: String,
    /// Word counts over the included sections, most frequent first (as
    /// `Report::word_freq`).
    pub words: Vec<(String, u32)>,
    /// Names and terms: the user dictionary and the recommended Suggested
    /// words when the profile was written. A word containing one (of two or
    /// more characters) is left out of the running words unless known.
    #[serde(default)]
    pub names: Vec<String>,
    /// Japanese: the short-unit parts of long-unit words (冒険者 → 冒険, 者).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub parts: HashMap<String, Vec<String>>,
    /// Japanese: how the book spells a word key, where it differs (為る → する).
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub display: HashMap<String, String>,
    /// Character (kanji) counts over the included sections, as
    /// `Report::char_freq`: with them a Library row's coverage is computed
    /// again from the profile alone when only the known words change.
    /// `None` in a profile written before they were kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chars: Option<Vec<(char, u32)>>,
}

impl Profile {
    /// From the report the Overview shows. `names` as given to
    /// [`crate::anki::BookCoverage::compute`].
    pub fn from_report<'a>(
        id: &str,
        r: &Report,
        names: impl IntoIterator<Item = &'a String>,
    ) -> Profile {
        let mut names: Vec<String> = names.into_iter().cloned().collect();
        names.sort_unstable();
        names.dedup();
        Profile {
            format: FORMAT,
            id: id.to_owned(),
            language: r.language,
            variant: format!("{:?}", r.variant),
            segmenter: r.segmenter.label().to_owned(),
            words: r.word_freq.clone(),
            names,
            parts: r.word_parts.clone(),
            display: r.word_display.clone(),
            chars: Some(r.char_freq.clone()),
        }
    }

    /// A hash of the contents, to tell whether a profile needs writing again.
    pub fn digest(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        self.id.hash(&mut h);
        self.language.hash(&mut h);
        self.variant.hash(&mut h);
        self.segmenter.hash(&mut h);
        self.words.hash(&mut h);
        self.names.hash(&mut h);
        let mut parts: Vec<_> = self.parts.iter().collect();
        parts.sort_unstable();
        parts.hash(&mut h);
        let mut display: Vec<_> = self.display.iter().collect();
        display.sort_unstable();
        display.hash(&mut h);
        self.chars.hash(&mut h);
        h.finish()
    }

    /// The word as this book spells it (Japanese keys may be normalised forms).
    pub fn shown<'a>(&'a self, w: &'a str) -> &'a str {
        self.display.get(w).map_or(w, String::as_str)
    }

    /// Gzip-compressed JSON, through [`crate::saver`] (in the window,
    /// written in the background).
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
        serde_json::to_writer(&mut gz, self)
            .map_err(std::io::Error::other)
            .and_then(|_| gz.finish())
            .map_err(|e| format!("Cannot write {}: {e}", path.display()))
            .and_then(|bytes| crate::saver::write(path, bytes))
    }

    /// `None` if missing, unreadable or of another layout.
    pub fn load(path: &Path) -> Option<Profile> {
        let bytes = crate::saver::read(path).ok()?;
        let p: Profile = serde_json::from_reader(flate2::read::GzDecoder::new(&bytes[..])).ok()?;
        (p.format == FORMAT).then_some(p)
    }
}

/// `profiles/<id>.json.gz` next to the history file `history`.
pub fn path_in(history: &Path, id: &str) -> PathBuf {
    history
        .with_file_name("profiles")
        .join(format!("{id}.json.gz"))
}

/// Analyse a book file the way the window does, for a Library book analysed
/// in the background. `included`: the sections to include (the book's
/// defaults if `None` or not of its length). `words` and
/// `known` give the user dictionary and the known words passed to the
/// segmenter for a language. `fold_chars`: see [`analyze::build_report`].
/// Downloads the segmenter's model if missing.
pub struct Analysed {
    pub book: crate::extract::Book,
    pub report: Report,
    /// The user dictionary plus the recommended discovered words.
    pub names: Vec<String>,
    pub file_id: Option<String>,
}

pub fn analyse_file(
    path: &Path,
    kind: crate::segment::SegmenterKind,
    included: Option<&[bool]>,
    words: &dyn Fn(Language) -> Vec<String>,
    known: &dyn Fn(Language) -> Vec<String>,
    cache: Option<&crate::cache::Cache>,
    fold_chars: bool,
) -> Result<Analysed, String> {
    use crate::{analyze, discover, extract};
    let book = extract::load(path, extract::EncodingChoice::Auto)?;
    let lang = analyze::detect_language(&book);
    let (tr, si) = analyze::detect_variant(&book);
    if let Some(m) =
        crate::segment::model_for(kind.resolve_in(lang, crate::script::classify(tr, si)))
        && !m.is_cached()
    {
        m.ensure(&|_, _| {})?;
    }
    let user = words(lang);
    let prep = analyze::analyze_book_cached(&book, kind, &user, &known(lang), cache, &|_, _| {})?;
    let included: Vec<bool> = match included {
        Some(inc) if inc.len() == book.sections.len() => inc.to_vec(),
        _ => book.sections.iter().map(|s| s.default_included()).collect(),
    };
    let report = analyze::build_report(&book, &prep, &included, fold_chars);
    let have: std::collections::HashSet<String> = user.iter().cloned().collect();
    let mut names = user;
    names.extend(
        discover::discover(
            &book,
            &included,
            &report.word_freq,
            &have,
            &discover::Options::for_size(report.han_total),
        )
        .into_iter()
        .filter(|c| c.recommended)
        .map(|c| c.word),
    );
    Ok(Analysed {
        file_id: crate::history::file_id(path).ok(),
        book,
        report,
        names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_digest() {
        let dir = std::env::temp_dir().join(format!("hanstat-profile-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let p = Profile {
            format: FORMAT,
            id: "abc".into(),
            language: Language::Japanese,
            variant: "Unknown".into(),
            segmenter: "Sudachi".into(),
            words: vec![("為る".into(), 9), ("冒険者".into(), 3)],
            names: vec!["アリス".into()],
            parts: HashMap::from([("冒険者".into(), vec!["冒険".into(), "者".into()])]),
            display: HashMap::from([("為る".into(), "する".into())]),
            chars: Some(vec![('冒', 3), ('険', 3)]),
        };
        let path = path_in(&dir.join("history.json"), &p.id);
        p.save(&path).unwrap();
        let q = Profile::load(&path).unwrap();
        assert_eq!(q.words, p.words);
        assert_eq!(q.digest(), p.digest());
        assert_eq!(q.shown("為る"), "する");
        let mut r = q.clone();
        r.names.push("ボブ".into());
        assert_ne!(r.digest(), p.digest());
        std::fs::write(&path, b"junk").unwrap();
        assert!(Profile::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
