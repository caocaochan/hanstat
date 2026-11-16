//! The book's language, detected from its text. It picks the analyser
//! (Chinese segmenters, Kiwi for Korean, Sudachi for Japanese) and which
//! saved settings (Anki selection, word lists, marked words) are in use.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Language {
    #[default]
    Chinese,
    Korean,
    Japanese,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::Chinese, Language::Korean, Language::Japanese];

    /// English name.
    pub fn name(self) -> &'static str {
        match self {
            Language::Chinese => "Chinese",
            Language::Korean => "Korean",
            Language::Japanese => "Japanese",
        }
    }

    /// Suffix of this language's settings files: `anki.json`, `anki-ko.json`, `anki-ja.json`.
    /// Chinese keeps the original names, so existing settings stay in place.
    pub fn file_suffix(self) -> &'static str {
        match self {
            Language::Chinese => "",
            Language::Korean => "-ko",
            Language::Japanese => "-ja",
        }
    }

    /// `name.json` for Chinese, `name-ko.json` for Korean, `name-ja.json` for Japanese, in the app folder.
    pub fn settings_path(self, stem: &str) -> std::path::PathBuf {
        crate::models::app_dir().join(format!("{stem}{}.json", self.file_suffix()))
    }
}

/// Korean if the text has more Hangul syllables than Han characters (Korean
/// books may use some Hanja; Chinese books contain no Hangul). Otherwise
/// Japanese if kana are at least a tenth as many as Han characters: Japanese
/// prose has more kana than kanji, while a Chinese book has at most a few
/// kana in Japanese names or loanwords.
pub fn classify(hangul: u64, han: u64, kana: u64) -> Language {
    if hangul > han {
        Language::Korean
    } else if kana > 0 && kana * 10 >= han {
        Language::Japanese
    } else {
        Language::Chinese
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_majority() {
        assert_eq!(classify(0, 0, 0), Language::Chinese);
        assert_eq!(classify(0, 500, 0), Language::Chinese);
        assert_eq!(classify(10, 500, 0), Language::Chinese);
        assert_eq!(classify(9000, 400, 0), Language::Korean);
        // A Chinese book quoting a few Japanese names stays Chinese.
        assert_eq!(classify(0, 100_000, 300), Language::Chinese);
        assert_eq!(classify(0, 26_538, 64_072), Language::Japanese);
        assert_eq!(classify(0, 1000, 100), Language::Japanese);
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
        assert!(
            Language::Japanese
                .settings_path("anki")
                .ends_with("anki-ja.json")
        );
    }
}
