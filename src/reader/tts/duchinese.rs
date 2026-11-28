//! Native DuChinese chapter audio synced to reader text via `.crd` `syllable_times`.
//!
//! When a DuChinese book has downloaded chapter MP3s and cached `.crd` files,
//! [`DuBook`] locates them per spine index, [`align_chapter`] maps each spoken
//! sentence of [`Chapter::chars`] to its start time in seconds, and
//! [`DecodedChapter`] decodes the chapter MP3 once and partitions its PCM
//! samples cleanly at quiet boundaries between sentences so the existing
//! read-aloud player (highlighting, click-to-jump, WSOLA speed, auto page-turn)
//! plays native audio seamlessly.

use super::audio;
use crate::duchinese::data::{Crd, timing};
use crate::duchinese::store::{self, Records};
use crate::reader::tree::Chapter;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};

/// A sentence in [`Chapter::chars`] paired with its start time in the chapter's
/// native DuChinese recording.
#[derive(Clone, Debug, PartialEq)]
pub struct TimedSentence {
    pub range: Range<u32>,
    pub start_secs: f64,
}

/// Extract the DuChinese lesson id from a spine entry (`OEBPS/l1442.xhtml` → `"1442"`).
pub fn lesson_from_spine(spine_path: &str) -> Option<String> {
    let file = spine_path.rsplit('/').next()?;
    let id = file.strip_prefix('l')?.strip_suffix(".xhtml")?;
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Some(id.to_owned())
    } else {
        None
    }
}

/// Per-book resolver for downloaded DuChinese chapter audio and `.crd` files.
#[derive(Clone, Debug)]
pub struct DuBook {
    cache_root: PathBuf,
    pub traditional: bool,
    /// Lesson id per spine index (`None` for non-lesson spine items).
    lessons: Vec<Option<String>>,
    /// Known `(crd_hash, audio_hash)` per lesson from `duchinese.json`.
    hashes: HashMap<String, (String, String)>,
}

impl DuBook {
    /// Open a resolver for a DuChinese EPUB at `book_path` with `spine`.
    pub fn open(book_path: &Path, spine: &[String], traditional: bool) -> DuBook {
        let cache_root = store::cache_root();
        let lessons: Vec<Option<String>> = spine.iter().map(|s| lesson_from_spine(s)).collect();
        let mut hashes = HashMap::new();
        let records = Records::load();
        if let Some(rec) = records.by_path(book_path) {
            for ch in &rec.chapters {
                hashes.insert(ch.lesson.clone(), (ch.crd.clone(), ch.audio.clone()));
            }
        }
        DuBook {
            cache_root,
            traditional,
            lessons,
            hashes,
        }
    }

    #[cfg(test)]
    pub fn for_test(
        cache_root: PathBuf,
        lessons: Vec<Option<String>>,
        hashes: HashMap<String, (String, String)>,
        traditional: bool,
    ) -> DuBook {
        DuBook {
            cache_root,
            traditional,
            lessons,
            hashes,
        }
    }

    /// The `(crd_path, audio_path)` for chapter `spine`, if both files exist on disk.
    pub fn chapter_files(&self, spine: usize) -> Option<(PathBuf, PathBuf)> {
        let lesson = self.lessons.get(spine)?.as_deref()?;
        let (crd_hash, audio_hash) = self
            .hashes
            .get(lesson)
            .map(|(c, a)| (c.as_str(), a.as_str()))
            .unwrap_or(("", ""));
        let audio = (!audio_hash.is_empty())
            .then(|| store::audio_path(&self.cache_root, lesson, audio_hash))
            .filter(|p| p.is_file())
            .or_else(|| store::find_lesson_file(&self.cache_root.join("audio"), lesson, "mp3"))?;
        let crd = (!crd_hash.is_empty())
            .then(|| store::crd_path(&self.cache_root, lesson, crd_hash))
            .filter(|p| p.is_file())
            .or_else(|| store::find_lesson_file(&self.cache_root.join("crd"), lesson, "json"))?;
        Some((crd, audio))
    }

    /// `(chapters_with_audio, total_chapters)`.
    pub fn coverage(&self) -> (usize, usize) {
        let have = (0..self.lessons.len())
            .filter(|&i| self.chapter_files(i).is_some())
            .count();
        (have, self.lessons.len())
    }

    pub fn chapter_has_audio(&self, spine: usize) -> bool {
        self.chapter_files(spine).is_some()
    }
}

/// Align `crd`'s syllable timestamps to the sentences of `ch` (from
/// [`super::sentences_of`]). Any synthetic `<h2>` heading not present in the
/// `.crd` is omitted so playback starts directly on spoken text.
pub fn align_chapter(ch: &Chapter, crd: &Crd, traditional: bool) -> Option<Vec<TimedSentence>> {
    let t = timing(crd, traditional)?;

    // Extract non-spoiler blocks from `ch.chars` (separated by '\0').
    let mut ch_blocks: Vec<(usize, usize)> = Vec::new();
    let mut start = 0usize;
    let n = ch.chars.len();
    for i in 0..=n {
        if i == n || ch.chars[i] == '\0' {
            if i > start {
                let a = start as u32;
                let b = i as u32;
                let is_spoiler = ch.spoiler_ranges.iter().any(|s| a < s.end && s.start < b);
                if !is_spoiler && ch.chars[start..i].iter().any(|c| !c.is_whitespace()) {
                    ch_blocks.push((start, i));
                }
            }
            start = i + 1;
        }
    }
    if ch_blocks.is_empty() {
        return None;
    }

    // If the EPUB has a synthetic `<h2>` heading that did not come from `.crd`,
    // skip that leading block.
    let mut cursor = if !t.has_heading && ch_blocks.len() == t.blocks.len() + 1 {
        1
    } else {
        0
    };

    let mut char_times: Vec<Option<f64>> = vec![None; ch.chars.len()];
    let mut matched = 0usize;
    for (block_text, block_times) in &t.blocks {
        let want: Vec<(char, Option<f64>)> = block_text
            .chars()
            .zip(block_times.iter().copied())
            .filter(|(c, _)| !c.is_whitespace())
            .collect();
        if want.is_empty() {
            continue;
        }
        let found = ch_blocks[cursor..].iter().position(|&(a, b)| {
            let mut it = ch.chars[a..b]
                .iter()
                .copied()
                .filter(|c| !c.is_whitespace());
            want.iter().all(|(wc, _)| it.next() == Some(*wc)) && it.next().is_none()
        });
        let Some(rel) = found else {
            continue;
        };
        let (a, b) = ch_blocks[cursor + rel];
        let mut wi = 0usize;
        for pos in a..b {
            if !ch.chars[pos].is_whitespace() {
                char_times[pos] = want[wi].1;
                wi += 1;
            }
        }
        cursor += rel + 1;
        matched += 1;
    }
    if matched == 0 {
        return None;
    }

    let mut out = Vec::new();
    let mut prev_start = 0.0f64;
    for range in super::sentences_of(ch) {
        let a = range.start as usize;
        let b = (range.end as usize).min(char_times.len());
        let Some(s) = char_times[a..b].iter().flatten().copied().next() else {
            continue;
        };
        let start_secs = s.max(prev_start);
        prev_start = start_secs;
        out.push(TimedSentence { range, start_secs });
    }
    (!out.is_empty()).then_some(out)
}

/// A decoded DuChinese chapter MP3 partitioned into exact per-sentence sample
/// ranges.
pub struct DecodedChapter {
    pub samples: Vec<f32>,
    pub rate: u32,
    cuts: Vec<Range<usize>>,
}

impl DecodedChapter {
    /// Decode `audio_path` (MP3) and compute per-sentence cuts from `sentences`.
    pub fn load(audio_path: &Path, sentences: &[TimedSentence]) -> Result<DecodedChapter, String> {
        let bytes =
            std::fs::read(audio_path).map_err(|e| format!("Cannot read DuChinese audio: {e}"))?;
        let (samples, rate) = audio::decode_mp3(bytes)?;
        Ok(Self::from_pcm(samples, rate, sentences))
    }

    pub fn from_pcm(samples: Vec<f32>, rate: u32, sentences: &[TimedSentence]) -> DecodedChapter {
        let cuts = partition_samples(&samples, rate, sentences);
        DecodedChapter {
            samples,
            rate,
            cuts,
        }
    }

    /// PCM samples for sentence `idx`.
    pub fn slice(&self, idx: usize) -> Vec<f32> {
        self.cuts
            .get(idx)
            .map(|r| self.samples[r.clone()].to_vec())
            .unwrap_or_default()
    }
}

/// Partition `samples` into one contiguous range per timed sentence.
/// Adjacent ranges share an exact boundary snapped to the quietest point in a
/// short window just before each sentence's onset, so sequential playback at
/// 1.0× reproduces the original waveform continuously while random jumps start
/// cleanly in the pause before the sentence.
fn partition_samples(samples: &[f32], rate: u32, sentences: &[TimedSentence]) -> Vec<Range<usize>> {
    if sentences.is_empty() {
        return Vec::new();
    }
    let total = samples.len();
    let to_sample =
        |secs: f64| -> usize { ((secs.max(0.0) * rate as f64).round() as usize).min(total) };

    let mut boundaries = Vec::with_capacity(sentences.len() + 1);
    let first_start = if sentences[0].start_secs <= 2.0 {
        0
    } else {
        snap_quiet(samples, rate, to_sample(sentences[0].start_secs), 0)
    };
    boundaries.push(first_start);

    let min_gap = (rate as usize / 50).max(1); // 20 ms
    for s in &sentences[1..] {
        let prev = *boundaries.last().unwrap();
        let nom = to_sample(s.start_secs).max(prev);
        let floor = (prev + min_gap).min(nom);
        let cut = snap_quiet(samples, rate, nom, floor);
        boundaries.push(cut);
    }
    boundaries.push(total);

    boundaries.windows(2).map(|w| w[0]..w[1]).collect()
}

/// Find the quietest sample index in `[nom - 35ms, nom]` (clamped to `floor`),
/// using a 4 ms energy window.
fn snap_quiet(samples: &[f32], rate: u32, nom: usize, floor: usize) -> usize {
    let nom = nom.min(samples.len());
    let floor = floor.min(nom);
    let lookback = (rate as usize * 35) / 1000;
    let lo = nom.saturating_sub(lookback).max(floor);
    let win = ((rate as usize * 4) / 1000).max(1);
    if nom.saturating_sub(lo) <= win {
        return nom;
    }
    let step = (win / 2).max(1);
    let mut best_pos = nom;
    let mut best_energy = f32::INFINITY;
    let mut p = lo;
    while p + win <= nom {
        let energy: f32 = samples[p..p + win].iter().map(|s| s * s).sum();
        // Prefer the latest quiet point (closest to speech onset) on near-ties.
        if energy <= best_energy {
            best_energy = energy;
            best_pos = p + win / 2;
        }
        p += step;
    }
    best_pos.clamp(floor, nom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reader::css::RuleSet;
    use crate::reader::tree::{Source as TreeSource, build_doc, ua_rules};

    fn chapter_from_html(html: &str) -> Chapter {
        let doc = scraper::Html::parse_document(html);
        build_doc(
            &doc,
            &ua_rules(),
            &RuleSet::default(),
            TreeSource::Epub {
                base: String::new(),
                path: String::new(),
                duchinese: true,
            },
        )
    }

    #[test]
    fn extracts_lesson_id_from_spine() {
        assert_eq!(
            lesson_from_spine("OEBPS/l1442.xhtml").as_deref(),
            Some("1442")
        );
        assert_eq!(lesson_from_spine("l99-a.xhtml").as_deref(), Some("99-a"));
        assert_eq!(lesson_from_spine("OEBPS/nav.xhtml"), None);
        assert_eq!(lesson_from_spine("text/00001.xhtml"), None);
    }

    #[test]
    fn aligns_heading_and_skips_translations() {
        let crd_json = r#"{
            "version": "2",
            "words": [
                {"hanzi": "小猫", "tc_hanzi": "小貓", "pinyin": "xiǎo māo"},
                {"hanzi": "\n\n", "tc_hanzi": "\n\n"},
                {"hanzi": "这", "tc_hanzi": "這", "pinyin": "zhè"},
                {"hanzi": "是", "tc_hanzi": "是", "pinyin": "shì"},
                {"hanzi": "猫", "tc_hanzi": "貓", "pinyin": "māo"},
                {"hanzi": "。", "tc_hanzi": "。"},
                {"hanzi": "它", "tc_hanzi": "它", "pinyin": "tā"},
                {"hanzi": "很", "tc_hanzi": "很", "pinyin": "hěn"},
                {"hanzi": "可爱", "tc_hanzi": "可愛", "pinyin": "kě ài"},
                {"hanzi": "。", "tc_hanzi": "。"}
            ],
            "syllable_times": [0.5, 0.9, 2.0, 2.4, 2.8, 4.0, 4.3, 4.6, 5.0, 5.8],
            "sentence_indices": [1, 2, 6],
            "sentence_translations": ["Kitten", "", "This is a cat.", "It is cute."]
        }"#;
        let crd = Crd::parse(crd_json.as_bytes()).unwrap();
        let html = r#"<html><body>
            <h2>小猫</h2>
            <p>这是猫。它很可爱。</p>
            <p class="tr" lang="en">This is a cat. It is cute.</p>
        </body></html>"#;
        let ch = chapter_from_html(html);
        let timed = align_chapter(&ch, &crd, false).unwrap();
        let got: Vec<(String, f64)> = timed
            .iter()
            .map(|t| {
                (
                    ch.chars[t.range.start as usize..t.range.end as usize]
                        .iter()
                        .collect(),
                    t.start_secs,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("小猫".to_owned(), 0.5),
                ("这是猫。".to_owned(), 2.0),
                ("它很可爱。".to_owned(), 4.0),
            ]
        );
    }

    #[test]
    fn skips_synthetic_heading_not_in_crd() {
        let crd_json = r#"{
            "version": "2",
            "words": [
                {"hanzi": "今天", "pinyin": "jīn tiān"},
                {"hanzi": "天气", "pinyin": "tiān qì"},
                {"hanzi": "好", "pinyin": "hǎo"},
                {"hanzi": "。", "pinyin": ""},
                {"hanzi": "\n"},
                {"hanzi": "我们", "pinyin": "wǒ men"},
                {"hanzi": "去", "pinyin": "qù"},
                {"hanzi": "玩", "pinyin": "wán"},
                {"hanzi": "。", "pinyin": ""}
            ],
            "syllable_times": [0.3, 0.6, 0.9, 1.2, 1.5, 2.8, 3.1, 3.4, 3.7, 4.2]
        }"#;
        let crd = Crd::parse(crd_json.as_bytes()).unwrap();
        // When `text.heading` is None, `duchinese/epub.rs` synthesizes `<h2>第1章</h2>`.
        let html = r#"<html><body>
            <h2>第1章</h2>
            <p>今天天气好。</p>
            <p>我们去玩。</p>
        </body></html>"#;
        let ch = chapter_from_html(html);
        let timed = align_chapter(&ch, &crd, false).unwrap();
        let got: Vec<(String, f64)> = timed
            .iter()
            .map(|t| {
                (
                    ch.chars[t.range.start as usize..t.range.end as usize]
                        .iter()
                        .collect(),
                    t.start_secs,
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("今天天气好。".to_owned(), 0.3),
                ("我们去玩。".to_owned(), 2.8),
            ]
        );
    }

    #[test]
    fn partition_covers_whole_audio_without_gaps_or_overlaps() {
        let rate = 1000;
        let mut pcm = vec![0.4f32; 5000];
        // Quiet pause just before 2.0 s.
        for s in &mut pcm[1970..1995] {
            *s = 0.0;
        }
        let sents = vec![
            TimedSentence {
                range: 0..2,
                start_secs: 0.4,
            },
            TimedSentence {
                range: 3..8,
                start_secs: 2.0,
            },
            TimedSentence {
                range: 8..12,
                start_secs: 3.5,
            },
        ];
        let dec = DecodedChapter::from_pcm(pcm.clone(), rate, &sents);
        let mut rebuilt = Vec::new();
        for i in 0..sents.len() {
            rebuilt.extend(dec.slice(i));
        }
        assert_eq!(rebuilt, pcm, "exact partition preserves waveform");
        // Cut 1 landed inside the quiet gap [1970..1995].
        assert!((1970..=1995).contains(&dec.cuts[0].end), "{:?}", dec.cuts);
    }

    #[test]
    fn dubook_resolves_cached_files_by_hash_and_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let crd0 = store::crd_path(root, "101", "c0");
        let aud0 = store::audio_path(root, "101", "a0");
        let crd1 = store::crd_path(root, "102", "c1");
        let aud1 = store::audio_path(root, "102", "a1");
        store::write_cached(&crd0, b"{}").unwrap();
        store::write_cached(&aud0, b"mp3").unwrap();
        store::write_cached(&crd1, b"{}").unwrap();
        store::write_cached(&aud1, b"mp3").unwrap();

        let mut hashes = HashMap::new();
        hashes.insert("101".to_owned(), ("c0".to_owned(), "a0".to_owned()));
        // Lesson 102 intentionally has empty hashes to exercise directory fallback.
        let book = DuBook::for_test(
            root.to_path_buf(),
            vec![
                Some("101".to_owned()),
                Some("102".to_owned()),
                Some("103".to_owned()),
            ],
            hashes,
            false,
        );
        assert_eq!(book.chapter_files(0), Some((crd0, aud0)));
        assert_eq!(book.chapter_files(1), Some((crd1, aud1)));
        assert_eq!(book.chapter_files(2), None);
        assert_eq!(book.coverage(), (2, 3));
    }
}
