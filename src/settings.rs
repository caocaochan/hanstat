//! Settings that apply everywhere, from the Settings window (⚙): `settings.json` in
//! [`crate::models::app_dir`].

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Rate the writing style of Chinese books with a local language model
    /// ([`crate::style`]) and count it toward their difficulty and the
    /// reading path.
    pub writing_style: bool,
    /// Percentage points of running words known that one point of writing
    /// style counts for ([`crate::style::target_offset`]).
    pub style_weight: f32,
    /// Leave words that occur only once in a book out of its known-word
    /// shares, difficulty and the reading path
    /// ([`crate::anki::BookCoverage::compute`], [`crate::plan::Library::build`]).
    pub skip_hapax: bool,
    /// Default reading speeds, for the Library and each book's Overview (where
    /// a book's speed can be changed on its own to try another).
    /// Chinese: characters per minute. Used when `cpm_by_hand` is on, and
    /// until a speed is measured in the reader otherwise.
    pub cpm: f32,
    /// Use `cpm` instead of the Chinese speed measured in the reader.
    pub cpm_by_hand: bool,
    /// Japanese: kanji and kana per minute.
    pub ja_cpm: f32,
    /// Korean: words per minute (as counted: content words).
    pub wpm: f32,
    /// Keyboard shortcuts changed from their defaults, by action, as
    /// `Ctrl+Shift+K` (empty: none). Read by the app's `shortcuts` module.
    pub shortcuts: BTreeMap<String, String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            writing_style: false,
            style_weight: crate::style::DEFAULT_WEIGHT,
            skip_hapax: false,
            cpm: 300.0,
            cpm_by_hand: false,
            ja_cpm: 300.0,
            wpm: 100.0,
            shortcuts: BTreeMap::new(),
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        crate::models::app_dir().join("settings.json")
    }

    /// Missing → defaults; unreadable → defaults, the file kept aside
    /// ([`crate::saver::load_json`]).
    pub fn load() -> Self {
        crate::saver::load_json(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        crate::saver::write(&Self::path(), serde_json::to_vec_pretty(self).unwrap())
    }
}
