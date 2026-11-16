//! The books analysed before, with their headline statistics, for the
//! History view. Kept in `%LOCALAPPDATA%\hanstat\history.json` (on macOS
//! `~/Library/Application Support/hanstat/history.json`).
//!
//! A book is identified by the SHA-256 of its file, so a renamed or moved
//! copy is still the same book; a file changed in place is recognised by its
//! path. Recording a book replaces any entry that matches either way.
//!
//! An entry holds what the Overview showed when it was recorded: the
//! sections included then, the segmenter used, and the known-word coverage
//! from the Anki cards and word lists in use at the time.

use crate::analyze::Report;
use crate::anki::Difficulty;
use crate::lang::Language;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Layout of the file; a file of a later layout is not overwritten.
const FORMAT: u32 = 1;

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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Known {
    /// "Anki", "Anki + HSK 1–5", "HSK 1–5": what counted as known.
    pub source: String,
    /// Percent of running words (names left out) that are known.
    pub running_words: f64,
    pub unique_words: f64,
    /// Zero for Korean.
    pub unique_chars: f64,
    pub unknown_words: u64,
}

impl Known {
    pub fn difficulty(&self) -> Difficulty {
        Difficulty::from_percent(self.running_words)
    }
}

impl Stats {
    /// From the report the Overview shows.
    pub fn from_report(r: &Report) -> Stats {
        let known = r.anki.as_ref().map(|c| {
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
        });
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

#[derive(Serialize, Deserialize)]
struct File {
    format: u32,
    books: Vec<Entry>,
}

/// The saved list, newest first.
#[derive(Default)]
pub struct History {
    pub entries: Vec<Entry>,
    path: PathBuf,
    /// The file is of a later layout, or unreadable: it is left alone.
    read_only: bool,
}

impl History {
    pub fn path() -> PathBuf {
        crate::models::app_dir().join("history.json")
    }

    /// Missing file → empty. An unreadable one is reported and never overwritten.
    pub fn load() -> (History, Option<String>) {
        Self::load_from(Self::path())
    }

    pub fn load_from(path: PathBuf) -> (History, Option<String>) {
        let mut h = History {
            entries: Vec::new(),
            path,
            read_only: false,
        };
        let warn = match std::fs::read(&h.path) {
            Err(_) => None,
            Ok(b) => match serde_json::from_slice::<File>(&b) {
                Ok(f) if f.format <= FORMAT => {
                    h.entries = f.books;
                    h.entries.sort_by_key(|e| std::cmp::Reverse(e.analysed));
                    None
                }
                Ok(f) => {
                    h.read_only = true;
                    Some(format!(
                        "{} is from a newer hanstat (format {}); history is not saved",
                        h.path.display(),
                        f.format
                    ))
                }
                Err(e) => {
                    h.read_only = true;
                    Some(format!(
                        "Ignoring unreadable {}: {e}; history is not saved",
                        h.path.display()
                    ))
                }
            },
        };
        (h, warn)
    }

    /// Add `entry`, replacing any entry for the same file or the same path,
    /// and save. Nothing is written if the book's entry already has these
    /// statistics, unless `touch` (a fresh analysis, which moves it to the top).
    pub fn record(&mut self, entry: Entry, touch: bool) -> Result<bool, String> {
        let same = |e: &Entry| e.id == entry.id || same_path(&e.path, &entry.path);
        if !touch
            && let Some(e) = self.entries.iter().find(|e| same(e))
            && e.id == entry.id
            && e.stats == entry.stats
            && e.title == entry.title
            && e.path == entry.path
        {
            return Ok(false);
        }
        // A replaced entry of another file (same path, changed contents) takes its cover with it.
        for old in self.entries.iter().filter(|e| same(e) && e.id != entry.id) {
            let _ = std::fs::remove_file(self.cover(&old.id));
        }
        self.entries.retain(|e| !same(e));
        self.entries.insert(0, entry);
        self.save().map(|_| true)
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        self.entries.retain(|e| e.id != id);
        if !self.read_only {
            let _ = std::fs::remove_file(self.cover(id));
        }
        self.save()
    }

    /// Where the cover thumbnail of book `id` is kept (it may not exist).
    pub fn cover(&self, id: &str) -> PathBuf {
        cover_in(&self.path, id)
    }

    /// Written to a temporary file and renamed, so a crash never leaves half a file.
    fn save(&self) -> Result<(), String> {
        if self.read_only {
            return Ok(());
        }
        let p = &self.path;
        let dir = p.parent().unwrap();
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let file = File {
            format: FORMAT,
            books: self.entries.clone(),
        };
        let tmp = p.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(&file).unwrap())
            .map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, p).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Cannot write {}: {e}", p.display())
        })
    }
}

/// Cover thumbnails are no larger than this (pixels), about four times the
/// size they're drawn at, for high-DPI screens and the larger hover view.
pub const COVER_MAX: (u32, u32) = (120, 168);

/// `covers/<id>.png` next to the history file.
fn cover_in(history: &Path, id: &str) -> PathBuf {
    history.with_file_name("covers").join(format!("{id}.png"))
}

/// Where the cover thumbnail of book `id` is kept, in the default folder.
pub fn cover_path(id: &str) -> PathBuf {
    cover_in(&History::path(), id)
}

/// Shrink a book's cover (JPEG, PNG, GIF or WebP) to fit [`COVER_MAX`] and
/// save it as PNG at `dest`, under a temporary name first.
pub fn save_cover(image: &[u8], dest: &Path) -> Result<(), String> {
    let img = image::load_from_memory(image).map_err(|e| format!("Cannot read the cover: {e}"))?;
    let thumb = img.thumbnail(COVER_MAX.0, COVER_MAX.1).into_rgba8();
    let dir = dest.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let tmp = dest.with_extension(format!("png.{}.tmp", std::process::id()));
    thumb
        .save_with_format(&tmp, image::ImageFormat::Png)
        .map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dest).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("Cannot write {}: {e}", dest.display())
    })
}

/// Windows paths compare without regard to case.
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// SHA-256 of a file, lowercase hex: what identifies a book in the history.
pub fn file_id(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// "just now", "5 min ago", "3 h ago", "yesterday", "12 days ago", "2025-03-14".
pub fn age(then: u64, now: u64) -> String {
    let s = now.saturating_sub(then);
    match s {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "yesterday".to_owned(),
        _ if s < 60 * 86_400 => format!("{} days ago", s / 86_400),
        _ => date(then),
    }
}

/// UTC date and time, "2025-03-14 18:05 UTC".
pub fn date_time(t: u64) -> String {
    let (h, m) = ((t % 86_400) / 3600, (t % 3600) / 60);
    format!("{} {h:02}:{m:02} UTC", date(t))
}

/// UTC calendar date of a Unix time (Hinnant's days-to-civil).
fn date(t: u64) -> String {
    let z = (t / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, path: &str, words: u64, at: u64) -> Entry {
        Entry {
            id: id.into(),
            path: path.into(),
            title: "t".into(),
            analysed: at,
            stats: Stats {
                format: "EPUB".into(),
                language: Language::Chinese,
                variant: "Simplified".into(),
                segmenter: "LTP perceptron".into(),
                sections_included: 1,
                sections_total: 1,
                han_total: 1,
                han_unique: 1,
                kana_total: 0,
                words_total: words,
                words_unique: 1,
                words_for_98: 1,
                known: None,
            },
        }
    }

    fn temp() -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "hanstat-history-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        d.join("history.json")
    }

    #[test]
    fn replaces_same_file_or_path() {
        let p = temp();
        let (mut h, w) = History::load_from(p.clone());
        assert!(w.is_none() && h.entries.is_empty());
        h.record(entry("a", "C:/books/a.epub", 10, 1), true)
            .unwrap();
        h.record(entry("b", "C:/books/b.epub", 20, 2), true)
            .unwrap();
        // Same file, moved: replaced.
        h.record(entry("a", "D:/a.epub", 11, 3), true).unwrap();
        // Same path, file changed: replaced (on Windows, whatever the case).
        let b = if cfg!(windows) {
            "c:/BOOKS/b.epub"
        } else {
            "C:/books/b.epub"
        };
        h.record(entry("c", b, 21, 4), true).unwrap();
        let ids: Vec<&str> = h.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["c", "a"]);
        // Unchanged statistics are not rewritten unless touched.
        assert!(!h.record(entry("a", "D:/a.epub", 11, 9), false).unwrap());
        assert!(h.record(entry("a", "D:/a.epub", 12, 9), false).unwrap());

        let (h2, w) = History::load_from(p.clone());
        assert!(w.is_none());
        let got: Vec<(&str, u64)> = h2
            .entries
            .iter()
            .map(|e| (e.id.as_str(), e.stats.words_total))
            .collect();
        assert_eq!(got, [("a", 12), ("c", 21)]);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn covers_are_shrunk_and_go_with_their_entry() {
        let p = temp();
        let (mut h, _) = History::load_from(p.clone());
        let mut big = Vec::new();
        image::RgbImage::from_pixel(600, 840, image::Rgb([200, 30, 30]))
            .write_to(
                &mut std::io::Cursor::new(&mut big),
                image::ImageFormat::Jpeg,
            )
            .unwrap();
        for id in ["a", "b"] {
            save_cover(&big, &h.cover(id)).unwrap();
        }
        let t = image::open(h.cover("a")).unwrap();
        assert_eq!((t.width(), t.height()), COVER_MAX);
        assert!(save_cover(b"not an image", &h.cover("x")).is_err());

        h.record(entry("a", "a.epub", 1, 1), true).unwrap();
        h.record(entry("b", "b.epub", 1, 2), true).unwrap();
        // b.epub changed in place: the old file's cover goes.
        h.record(entry("b2", "b.epub", 1, 3), true).unwrap();
        assert!(!h.cover("b").exists());
        h.remove("a").unwrap();
        assert!(!h.cover("a").exists());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn unreadable_file_is_left_alone() {
        let p = temp();
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "not json").unwrap();
        let (mut h, w) = History::load_from(p.clone());
        assert!(w.is_some());
        h.record(entry("a", "a", 1, 1), true).unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "not json");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(date_time(1_763_312_400), "2025-11-16 17:00 UTC");
        assert_eq!(age(100, 130), "just now");
        assert_eq!(age(0, 7200), "2 h ago");
        assert_eq!(age(0, 86_400 * 3), "3 days ago");
    }
}
