//! What a Library row shows of the known words' coverage of its book. Part
//! of `HANSTAT_COVERAGE_FINGERPRINT` (build.rs), with
//! [`crate::anki::BookCoverage`]: a change here updates the rows from their
//! saved word data, without analysing the books again.

use crate::anki::Difficulty;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// "Anki", "Anki + HSK 1–5", "HSK 1–5": what counted as known.
    pub source: String,
    /// Percent of running words (names left out) that are known.
    pub running_words: f64,
    pub unique_words: f64,
    pub unique_chars: f64,
    pub unknown_words: u64,
}

impl Known {
    pub fn difficulty(&self) -> Difficulty {
        Difficulty::from_percent(self.running_words)
    }

    /// What a row shows of a book's coverage by the known words.
    pub fn from_coverage(c: &crate::anki::BookCoverage) -> Known {
        let lists = &c.listed.source;
        Known {
            source: match (c.from_anki, lists.is_empty()) {
                (true, true) => "Anki".to_owned(),
                (true, false) => format!("Anki + {lists}"),
                (false, _) => lists.clone(),
            },
            running_words: c.running_words.percent(),
            unique_words: c.unique_words.percent(),
            unique_chars: c.unique_chars.percent(),
            unknown_words: c.unique_words.not_known(),
        }
    }
}
