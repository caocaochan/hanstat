//! What duchinese.net serves, as far as importing needs it: a story or
//! course ([`Course`]), its chapters ([`Lesson`]) and a chapter's text
//! ([`Crd`], DuChinese's own segmented words with Simplified and Traditional
//! forms), and the chapter turned into paragraphs ([`text`]).
//!
//! None of this is a published API: the shapes were read from the site's
//! own requests on 2026-11-23. Fields are optional or defaulted wherever a
//! missing one can be lived with, so a small change on their side doesn't
//! stop everything.

use serde::{Deserialize, Deserializer, Serialize};

/// The chapter format this code reads (`"version"` in a `.crd` file).
pub const CRD_VERSION: &str = "2";

/// A multi-chapter story or a course, from `/lessons/courses.json` or a
/// lesson's `course`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Course {
    pub id: u64,
    pub title: String,
    pub description: String,
    /// `short_stories`, `courses`, `article_collections`.
    pub group: String,
    /// `newbie`, `elementary`, `intermediate`, `upper intermediate`,
    /// `advanced`, `master`.
    #[serde(deserialize_with = "null_default")]
    pub levels: Vec<String>,
    /// Chapters released (or about to be: see [`Course::announced`]).
    pub lesson_count: u32,
    /// Chapters announced and not released yet.
    pub placeholder_count: u32,
    /// `/lessons/courses/<id>-<slug>/lessons.json`.
    pub lessons_url: String,
    pub medium_image_url: Option<String>,
    pub large_image_url: Option<String>,
    pub release_at: Option<String>,
}

impl Course {
    /// Chapters announced in all: released and to come.
    pub fn announced(&self) -> u32 {
        self.lesson_count + self.placeholder_count
    }

    /// A story (one narrative), not a course of separate readings.
    pub fn is_story(&self) -> bool {
        self.group == "short_stories"
    }

    /// The course's page on the site.
    pub fn page(&self) -> String {
        let path = self.lessons_url.trim_end_matches(".json");
        let path = path.strip_suffix("/lessons").unwrap_or(path);
        format!("{}{path}", super::api::BASE)
    }

    /// Its lowest level, as the site names them.
    pub fn level(&self) -> Level {
        self.levels
            .iter()
            .filter_map(|l| Level::parse(l))
            .min()
            .unwrap_or(Level::Unknown)
    }
}

/// DuChinese's six levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Level {
    Newbie,
    Elementary,
    Intermediate,
    UpperIntermediate,
    Advanced,
    Master,
    Unknown,
}

impl Level {
    pub const ALL: [Level; 6] = [
        Level::Newbie,
        Level::Elementary,
        Level::Intermediate,
        Level::UpperIntermediate,
        Level::Advanced,
        Level::Master,
    ];

    pub fn parse(s: &str) -> Option<Level> {
        Some(
            match s
                .trim()
                .to_ascii_lowercase()
                .replace(['_', '-'], " ")
                .as_str()
            {
                "newbie" => Level::Newbie,
                "elementary" => Level::Elementary,
                "intermediate" => Level::Intermediate,
                "upper intermediate" => Level::UpperIntermediate,
                "advanced" => Level::Advanced,
                "master" => Level::Master,
                _ => return None,
            },
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Level::Newbie => "Newbie",
            Level::Elementary => "Elementary",
            Level::Intermediate => "Intermediate",
            Level::UpperIntermediate => "Upper intermediate",
            Level::Advanced => "Advanced",
            Level::Master => "Master",
            Level::Unknown => "Unknown level",
        }
    }
}

/// A chapter, from a course's `lessons.json` (or `/lessons/latest.json`,
/// with its `course`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Lesson {
    #[serde(deserialize_with = "string_or_number")]
    pub id: String,
    pub title: String,
    pub course_position: Option<u32>,
    pub free: bool,
    /// Missing for a chapter the account can't read.
    pub crd_url: Option<String>,
    /// Native narration MP3 on `static.duchinese.net`, if the account can read it.
    pub audio_url: Option<String>,
    pub release_at: Option<String>,
    pub course: Option<Course>,
}

impl Lesson {
    /// The account can read it: its text is linked.
    pub fn readable(&self) -> bool {
        self.crd_url.as_deref().is_some_and(|u| !u.is_empty())
    }

    /// The hash that names its text file (it changes when the text is
    /// edited), or `""`.
    pub fn crd_hash(&self) -> &str {
        self.crd_url.as_deref().map_or("", crd_hash)
    }

    /// Native audio is linked for this chapter.
    pub fn has_audio(&self) -> bool {
        self.audio_url.as_deref().is_some_and(|u| !u.is_empty())
    }

    /// The hash that names its audio file (it changes when re-recorded), or `""`.
    pub fn audio_hash(&self) -> &str {
        self.audio_url.as_deref().map_or("", audio_hash)
    }
}

/// `https://static.duchinese.net/documents/1442/5b4b…bed9.crd?17937` → `5b4b…bed9`.
pub fn crd_hash(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or("");
    name.strip_suffix(".crd").unwrap_or(name)
}

/// `https://static.duchinese.net/documents/1442/9f7d…6853.mp3?16366` → `9f7d…6853`.
pub fn audio_hash(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or("");
    let name = path.rsplit('/').next().unwrap_or("");
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

/// Lessons in reading order: by `course_position`, the order served otherwise.
pub fn in_order(mut lessons: Vec<Lesson>) -> Vec<Lesson> {
    lessons.sort_by_key(|l| l.course_position.unwrap_or(u32::MAX));
    lessons
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Crd {
    /// `"2"` or `2`: both are served.
    #[serde(deserialize_with = "string_or_number")]
    pub version: String,
    pub words: Vec<Word>,
    /// Start time (in seconds) of each spoken syllable (counting whitespace-separated
    /// tokens in each word's `pinyin`), with an optional final end timestamp.
    #[serde(deserialize_with = "times")]
    pub syllable_times: Vec<f64>,
    /// Where each sentence after the first begins, as an index into `words`.
    #[serde(deserialize_with = "indices")]
    pub sentence_indices: Vec<usize>,
    /// One per sentence (the first included); empty for blank ones.
    #[serde(deserialize_with = "null_default")]
    pub sentence_translations: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Word {
    #[serde(deserialize_with = "string_or_number")]
    pub hanzi: String,
    pub tc_hanzi: Option<String>,
    #[serde(deserialize_with = "string_or_number")]
    pub pinyin: String,
}

impl Crd {
    pub fn parse(bytes: &[u8]) -> Result<Crd, String> {
        let crd: Crd = serde_json::from_slice(bytes)
            .map_err(|e| format!("DuChinese sent a chapter hanstat can't read ({e})"))?;
        if crd.version != CRD_VERSION {
            return Err(format!(
                "DuChinese changed its chapter format (version {:?}, hanstat reads {CRD_VERSION}): \
                 a newer hanstat is needed",
                crd.version
            ));
        }
        if crd.words.is_empty() {
            return Err("DuChinese sent an empty chapter".to_owned());
        }
        Ok(crd)
    }
}

/// A chapter as paragraphs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Text {
    /// The chapter's own heading (its first line), if it has one.
    pub heading: Option<String>,
    pub paragraphs: Vec<Paragraph>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Paragraph {
    pub text: String,
    /// DuChinese's English for the sentences that begin in it.
    pub translation: String,
}

/// The longest first line taken as a heading.
const MAX_HEADING: usize = 30;

/// The chapter's paragraphs, in Traditional characters if `traditional`.
/// A short first line without sentence punctuation is its heading.
pub fn text(crd: &Crd, traditional: bool) -> Text {
    // Sentence of each word: how many sentence starts come at or before it.
    let mut starts = crd.sentence_indices.clone();
    starts.sort_unstable();
    starts.dedup();
    let mut paras: Vec<(String, Vec<usize>)> = vec![(String::new(), Vec::new())];
    let mut sentence = 0;
    let mut seen = vec![false; starts.len() + 1];
    for (i, w) in crd.words.iter().enumerate() {
        while sentence < starts.len() && starts[sentence] <= i {
            sentence += 1;
        }
        let s = if traditional {
            w.tc_hanzi
                .as_deref()
                .filter(|t| !t.is_empty())
                .unwrap_or(&w.hanzi)
        } else {
            &w.hanzi
        };
        if s.contains('\n') {
            for (k, part) in s.split('\n').enumerate() {
                if k > 0 {
                    paras.push((String::new(), Vec::new()));
                }
                paras.last_mut().unwrap().0.push_str(part);
            }
            continue;
        }
        let p = paras.last_mut().unwrap();
        p.0.push_str(s);
        // A sentence's translation goes with the paragraph it begins in.
        if !s.trim().is_empty() && !seen[sentence] {
            seen[sentence] = true;
            p.1.push(sentence);
        }
    }
    let mut paragraphs: Vec<Paragraph> = paras
        .into_iter()
        .filter_map(|(t, sentences)| {
            let text = t.trim().trim_start_matches('\u{3000}').trim().to_owned();
            (!text.is_empty()).then(|| Paragraph {
                translation: sentences
                    .iter()
                    .filter_map(|&s| crd.sentence_translations.get(s))
                    .map(|t| t.trim())
                    .filter(|t| !t.is_empty())
                    .collect::<Vec<_>>()
                    .join(" "),
                text,
            })
        })
        .collect();
    let heading = paragraphs
        .first()
        .filter(|p| {
            paragraphs.len() > 1
                && p.text.chars().count() <= MAX_HEADING
                && !p.text.contains(['。', '！', '？', '!', '?', '…', '；'])
        })
        .map(|p| p.text.clone());
    if heading.is_some() {
        paragraphs.remove(0);
    }
    Text {
        heading,
        paragraphs,
    }
}

/// Per-block character timestamps extracted from a `.crd` file, in the exact
/// block order produced by [`text`] (heading if present, then paragraphs).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ChapterTiming {
    /// True when the `.crd`'s first line became the EPUB's `<h2>` heading.
    /// When false, the EPUB's `<h2>` is a synthetic label not present in audio.
    pub has_heading: bool,
    /// `(text, char_times)` for each spoken block (heading if `has_heading`,
    /// followed by each paragraph). `char_times.len() == text.chars().count()`.
    pub blocks: Vec<(String, Vec<Option<f64>>)>,
    /// End timestamp of the final spoken syllable, if provided.
    pub end_time: Option<f64>,
}

/// Extract per-character syllable start times from `crd`, matching the exact
/// text transformation of [`text`]. Returns `None` if `crd.syllable_times` is
/// missing or inconsistent with the words' pinyin syllable count.
pub fn timing(crd: &Crd, traditional: bool) -> Option<ChapterTiming> {
    let total_syllables: usize = crd
        .words
        .iter()
        .map(|w| w.pinyin.split_whitespace().count())
        .sum();
    if total_syllables == 0 || crd.syllable_times.is_empty() {
        return None;
    }
    let n_times = crd.syllable_times.len();
    // DuChinese serves `total_syllables + 1` (start of each syllable plus end
    // of the last), or occasionally `total_syllables`. Reject mismatched arrays
    // (more than 5% off) rather than producing drifting highlights.
    let excess = if n_times < total_syllables {
        total_syllables - n_times
    } else {
        n_times.saturating_sub(total_syllables + 1)
    };
    if excess * 20 > total_syllables {
        return None;
    }
    let time_at = |syl: usize| -> Option<f64> {
        let idx = if n_times >= total_syllables {
            syl.min(n_times - 1)
        } else {
            (syl * (n_times - 1)) / total_syllables.max(1)
        };
        crd.syllable_times
            .get(idx)
            .copied()
            .filter(|t| t.is_finite() && *t >= 0.0)
    };

    let mut paras: Vec<(Vec<char>, Vec<Option<f64>>)> = vec![(Vec::new(), Vec::new())];
    let mut syl = 0usize;
    for w in &crd.words {
        let s = if traditional {
            w.tc_hanzi
                .as_deref()
                .filter(|t| !t.is_empty())
                .unwrap_or(&w.hanzi)
        } else {
            &w.hanzi
        };
        let k = w.pinyin.split_whitespace().count();
        if s.contains('\n') {
            for (part_idx, part) in s.split('\n').enumerate() {
                if part_idx > 0 {
                    paras.push((Vec::new(), Vec::new()));
                }
                let p = paras.last_mut().unwrap();
                for c in part.chars() {
                    p.0.push(c);
                    p.1.push(None);
                }
            }
            syl += k;
            continue;
        }
        let chars: Vec<char> = s.chars().collect();
        let m = chars.len();
        let p = paras.last_mut().unwrap();
        for (j, c) in chars.into_iter().enumerate() {
            let t = if k == 0 {
                None
            } else {
                let offset = if k <= m {
                    j.min(k - 1)
                } else {
                    (j * k) / m.max(1)
                };
                time_at(syl + offset)
            };
            p.0.push(c);
            p.1.push(t);
        }
        syl += k;
    }

    let blocks: Vec<(String, Vec<Option<f64>>)> = paras
        .into_iter()
        .filter_map(|(chars, times)| {
            // Mirror `t.trim().trim_start_matches('\u{3000}').trim()` exactly.
            let mut a = 0;
            let mut b = chars.len();
            while a < b && (chars[a].is_whitespace() || chars[a] == '\u{3000}') {
                a += 1;
            }
            while b > a && chars[b - 1].is_whitespace() {
                b -= 1;
            }
            if a == b {
                return None;
            }
            let text: String = chars[a..b].iter().collect();
            Some((text, times[a..b].to_vec()))
        })
        .collect();
    if blocks.is_empty() {
        return None;
    }
    let has_heading = blocks.len() > 1
        && blocks[0].0.chars().count() <= MAX_HEADING
        && !blocks[0]
            .0
            .contains(['。', '！', '？', '!', '?', '…', '；']);
    let end_time = (n_times > total_syllables)
        .then(|| crd.syllable_times.get(total_syllables).copied())
        .flatten()
        .filter(|t| t.is_finite() && *t >= 0.0);
    Some(ChapterTiming {
        has_heading,
        blocks,
        end_time,
    })
}

fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn string_or_number<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::String(s) => s,
        serde_json::Value::Number(n) => n.to_string(),
        _ => String::new(),
    })
}

fn indices<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<usize>, D::Error> {
    let v = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(v.iter()
        .filter_map(|x| match x {
            serde_json::Value::Number(n) => n.as_u64().map(|n| n as usize),
            serde_json::Value::String(s) => s.trim().parse().ok(),
            _ => None,
        })
        .collect())
}

fn times<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    let v = Option::<Vec<serde_json::Value>>::deserialize(d)?.unwrap_or_default();
    Ok(v.iter()
        .filter_map(|x| match x {
            serde_json::Value::Number(n) => n.as_f64().filter(|f| f.is_finite()),
            serde_json::Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
            _ => None,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made up, in the shape DuChinese serves (no text of theirs).
    pub(crate) const CRD: &str = r#"{
        "version": "2",
        "words": [
            {"hanzi": "小猫", "tc_hanzi": "小貓", "pinyin": "xiǎo māo"},
            {"hanzi": "\n\n", "tc_hanzi": "\n\n"},
            {"hanzi": "    ", "tc_hanzi": "    "},
            {"hanzi": "这", "tc_hanzi": "這"},
            {"hanzi": "是", "tc_hanzi": "是"},
            {"hanzi": "一只", "tc_hanzi": "一隻"},
            {"hanzi": "猫", "tc_hanzi": "貓"},
            {"hanzi": "。", "tc_hanzi": "。"},
            {"hanzi": "它", "tc_hanzi": "它"},
            {"hanzi": "很", "tc_hanzi": "很"},
            {"hanzi": "小", "tc_hanzi": "小"},
            {"hanzi": "，", "tc_hanzi": "，"},
            {"hanzi": "\n", "tc_hanzi": "\n"},
            {"hanzi": "    ", "tc_hanzi": "    "},
            {"hanzi": "也", "tc_hanzi": "也"},
            {"hanzi": "很", "tc_hanzi": "很"},
            {"hanzi": "可爱", "tc_hanzi": "可愛"},
            {"hanzi": "。", "tc_hanzi": "。"}
        ],
        "syllable_times": [0.5],
        "sentence_indices": [1, 3, 8],
        "sentence_translations": ["Kitten", "", "This is a cat.", "It is small, and cute."]
    }"#;

    #[test]
    fn paragraphs_heading_and_translations() {
        let crd = Crd::parse(CRD.as_bytes()).unwrap();
        let t = text(&crd, false);
        assert_eq!(t.heading.as_deref(), Some("小猫"));
        let p: Vec<(&str, &str)> = t
            .paragraphs
            .iter()
            .map(|p| (p.text.as_str(), p.translation.as_str()))
            .collect();
        // A sentence running over two paragraphs is translated once, with the first.
        assert_eq!(
            p,
            [
                (
                    "这是一只猫。它很小，",
                    "This is a cat. It is small, and cute."
                ),
                ("也很可爱。", "")
            ]
        );
        let t = text(&crd, true);
        assert_eq!(t.heading.as_deref(), Some("小貓"));
        assert_eq!(t.paragraphs[0].text, "這是一隻貓。它很小，");
    }

    #[test]
    fn a_long_first_line_is_text_not_a_heading() {
        let crd = Crd::parse(
            r#"{"version":"2","words":[{"hanzi":"今天天气很好。"},{"hanzi":"\n"},{"hanzi":"我们去公园。"}]}"#
                .as_bytes(),
        )
        .unwrap();
        let t = text(&crd, false);
        assert_eq!(t.heading, None);
        assert_eq!(t.paragraphs.len(), 2);
    }

    #[test]
    fn other_versions_and_junk_are_refused() {
        assert!(Crd::parse(br#"{"version":"3","words":[{"hanzi":"x"}]}"#).is_err());
        assert!(Crd::parse(b"<html>").is_err());
        assert!(
            Crd::parse(r#"{"version":2,"words":[{"hanzi":2000},{"hanzi":"年"}]}"#.as_bytes())
                .is_ok()
        );
        assert!(Crd::parse(br#"{"version":"2","words":[]}"#).is_err());
    }

    #[test]
    fn lessons_and_courses_parse_leniently() {
        let l: Vec<Lesson> = serde_json::from_str(
            r#"[{"id":"2562","free":false,"locked":true,"course_position":1,"title":"T"},
                {"id":2561,"free":true,"locked":false,"course_position":0,
                 "crd_url":"https://static.duchinese.net/documents/2561/abc123.crd?1",
                 "audio_url":"https://static.duchinese.net/documents/2561/def456.mp3?9",
                 "course":{"id":124,"levels":null,"lesson_count":2,"placeholder_count":3,
                           "group":"short_stories","lessons_url":"/lessons/courses/124-x/lessons.json"}}]"#,
        )
        .unwrap();
        let l = in_order(l);
        assert_eq!(l[0].id, "2561");
        assert!(l[0].readable() && !l[1].readable());
        assert_eq!(l[0].crd_hash(), "abc123");
        assert!(l[0].has_audio() && !l[1].has_audio());
        assert_eq!(l[0].audio_hash(), "def456");
        let c = l[0].course.as_ref().unwrap();
        assert_eq!(c.announced(), 5);
        assert!(c.is_story());
        assert_eq!(c.page(), "https://duchinese.net/lessons/courses/124-x");
        assert_eq!(
            Level::parse("upper intermediate"),
            Some(Level::UpperIntermediate)
        );
        assert_eq!(
            Level::parse("upper_intermediate"),
            Some(Level::UpperIntermediate)
        );
    }

    #[test]
    fn timing_maps_syllables_including_numbers_and_erhua() {
        let raw = r#"{
            "version": "2",
            "words": [
                {"hanzi": "刘备", "tc_hanzi": "劉備", "pinyin": "liú bèi"},
                {"hanzi": "\n\n"},
                {"hanzi": "这", "tc_hanzi": "這", "pinyin": "zhè"},
                {"hanzi": "2000", "pinyin": "2"},
                {"hanzi": "年", "pinyin": "nián"},
                {"hanzi": "有点儿", "tc_hanzi": "有點兒", "pinyin": "yǒu diǎn'r"},
                {"hanzi": "冷", "pinyin": "lěng"},
                {"hanzi": "。"}
            ],
            "syllable_times": [0.5, 1.0, 2.0, 2.5, 3.0, 3.4, 3.8, 4.2, 5.0]
        }"#;
        let crd = Crd::parse(raw.as_bytes()).unwrap();
        let t = timing(&crd, false).unwrap();
        assert!(t.has_heading);
        assert_eq!(t.end_time, Some(5.0));
        assert_eq!(t.blocks.len(), 2);
        assert_eq!(t.blocks[0].0, "刘备");
        assert_eq!(t.blocks[0].1, vec![Some(0.5), Some(1.0)]);
        assert_eq!(t.blocks[1].0, "这2000年有点儿冷。");
        assert_eq!(
            t.blocks[1].1,
            vec![
                Some(2.0), // 这
                Some(2.5), // 2
                Some(2.5), // 0
                Some(2.5), // 0
                Some(2.5), // 0
                Some(3.0), // 年
                Some(3.4), // 有
                Some(3.8), // 点
                Some(3.8), // 儿
                Some(4.2), // 冷
                None,      // 。
            ]
        );
        // Traditional text has the same timestamps.
        let tc = timing(&crd, true).unwrap();
        assert_eq!(tc.blocks[0].0, "劉備");
        assert_eq!(tc.blocks[1].0, "這2000年有點兒冷。");
        assert_eq!(tc.blocks[1].1, t.blocks[1].1);

        // Missing or wildly mismatched syllable_times returns None.
        let bad = Crd::parse(CRD.as_bytes()).unwrap();
        assert!(timing(&bad, false).is_none());
    }
}
