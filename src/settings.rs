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
    /// Characters per minute. Used when `cpm_by_hand` is on, and until a
    /// speed is measured in the reader otherwise.
    pub cpm: f32,
    /// Use `cpm` instead of the speed measured in the reader.
    pub cpm_by_hand: bool,
    /// Keyboard shortcuts changed from their defaults, by action, as
    /// `Ctrl+Shift+K` (empty: none). Read by the app's `shortcuts` module.
    pub shortcuts: BTreeMap<String, String>,
    /// Importing DuChinese stories (the app's `duchinese` module).
    pub duchinese: DuChinese,
}

/// How DuChinese stories are imported and kept up to date.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuChinese {
    /// Where imported books are saved; `None`: `DuChinese books` in the app folder
    /// (`duchinese` there is the cache).
    pub folder: Option<PathBuf>,
    /// New imports in Traditional characters (DuChinese's own `tc_hanzi`).
    pub traditional: bool,
    /// Each paragraph followed by DuChinese's English translation of it.
    pub translations: bool,
    /// Check imported stories for new chapters and add them on their own.
    pub auto_update: bool,
}

impl Default for DuChinese {
    fn default() -> Self {
        DuChinese {
            folder: None,
            traditional: false,
            translations: false,
            auto_update: true,
        }
    }
}

impl DuChinese {
    /// The folder imported books go to.
    pub fn folder(&self) -> PathBuf {
        self.folder
            .clone()
            .unwrap_or_else(|| crate::models::app_dir().join("DuChinese books"))
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            writing_style: false,
            style_weight: crate::style::DEFAULT_WEIGHT,
            skip_hapax: false,
            cpm: 300.0,
            cpm_by_hand: false,
            shortcuts: BTreeMap::new(),
            duchinese: DuChinese::default(),
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
