//! Settings that apply everywhere, from the ⚙ menu: `settings.json` in
//! [`crate::models::app_dir`].

use serde::{Deserialize, Serialize};
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
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            writing_style: false,
            style_weight: crate::style::DEFAULT_WEIGHT,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        crate::models::app_dir().join("settings.json")
    }

    pub fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&p, serde_json::to_vec_pretty(self).unwrap())
            .map_err(|e| format!("Cannot save {}: {e}", p.display()))
    }
}
