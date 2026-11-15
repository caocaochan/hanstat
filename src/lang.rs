//! The book's language, detected from its text. It picks the analyser
//! (Chinese segmenters or Kiwi for Korean) and which saved settings (Anki
//! selection, word lists, marked words) are in use.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[default]
    Chinese,
    Korean,
}

impl Language {
    pub const ALL: [Language; 2] = [Language::Chinese, Language::Korean];

    /// English name.
    pub fn name(self) -> &'static str {
        match self {
            Language::Chinese => "Chinese",
            Language::Korean => "Korean",
        }
    }

    /// Suffix of this language's settings files: `anki.json` / `anki-ko.json`.
    /// Chinese keeps the original names, so existing settings stay in place.
    pub fn file_suffix(self) -> &'static str {
        match self {
            Language::Chinese => "",
            Language::Korean => "-ko",
        }
    }

    /// `name.json` for Chinese, `name-ko.json` for Korean, in the app folder.
    pub fn settings_path(self, stem: &str) -> std::path::PathBuf {
        crate::models::app_dir().join(format!("{stem}{}.json", self.file_suffix()))
    }
}

/// Korean if the text has more Hangul syllables than Han characters. Korean
/// books may use some Hanja; Chinese books contain no Hangul.
pub fn classify(hangul: u64, han: u64) -> Language {
    if hangul > han {
        Language::Korean
    } else {
        Language::Chinese
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_majority() {
        assert_eq!(classify(0, 0), Language::Chinese);
        assert_eq!(classify(0, 500), Language::Chinese);
        assert_eq!(classify(10, 500), Language::Chinese);
        assert_eq!(classify(9000, 400), Language::Korean);
    }

    #[test]
    fn settings_files() {
        assert!(
            Language::Chinese
                .settings_path("anki")
                .ends_with("anki.json")
        );
        assert!(
            Language::Korean
                .settings_path("known")
                .ends_with("known-ko.json")
        );
    }
}
