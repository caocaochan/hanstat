//! What is kept on disk: the imported stories ([`Records`], `duchinese.json`
//! in the app folder: worth keeping) and a cache of what was fetched (the
//! catalogue, chapter lists, chapter texts, covers and fit estimates, in
//! `duchinese/` in the cache folder: fetched again if lost).

use super::data::{Course, Lesson};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A chapter of an imported book, in the book's order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChapterRec {
    pub lesson: String,
    /// Its table of contents entry.
    pub label: String,
    /// The text's hash ([`Lesson::crd_hash`]): it changes when DuChinese edits it.
    pub crd: String,
    pub release: String,
}

/// A story imported as a book.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    /// The series as last seen (title, levels, chapters announced).
    pub course: Course,
    pub path: PathBuf,
    pub traditional: bool,
    pub translations: bool,
    pub chapters: Vec<ChapterRec>,
    /// Chapters released that the account couldn't read, left out.
    pub locked: u32,
    pub checked: u64,
    /// The last update that added chapters: how many, and when.
    pub added: u32,
    pub added_at: u64,
    /// New chapters fetched while the book was open in the reader: the
    /// chapter list to rebuild it from once it is closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending: Option<Vec<Lesson>>,
}

/// "New" lasts this long after chapters were added.
pub const NEW_FOR: u64 = 3 * 24 * 3600;
/// A series with a release this recent is checked even if it looks complete.
const RECENT: u64 = 8 * 7 * 24 * 3600;

impl Record {
    /// More chapters are announced than released.
    pub fn ongoing(&self) -> bool {
        (self.course.announced() as usize) > self.chapters.len() + self.locked as usize
    }

    /// Worth asking DuChinese about on its own: still coming out, or out lately.
    pub fn check_due(&self, now: u64) -> bool {
        let last = self
            .chapters
            .iter()
            .filter_map(|c| super::date_secs(&c.release))
            .max()
            .unwrap_or(0);
        self.ongoing() || self.locked > 0 || now.saturating_sub(last) < RECENT
    }

    /// Chapters added in the last [`NEW_FOR`].
    pub fn new_chapters(&self, now: u64) -> u32 {
        if now.saturating_sub(self.added_at) < NEW_FOR {
            self.added
        } else {
            0
        }
    }

    /// Where it stands, for the Library: "4 of 10 · ongoing", "6 chapters".
    pub fn progress(&self) -> String {
        let have = self.chapters.len();
        let all = (self.course.announced() as usize).max(have + self.locked as usize);
        if all > have {
            let mut s = format!("{have} of {all}");
            if self.locked > 0 {
                s.push_str(&format!(" · {} locked", self.locked));
            }
            if self.ongoing() {
                s.push_str(" · ongoing");
            }
            s
        } else {
            format!("{have} chapter{}", if have == 1 { "" } else { "s" })
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct File {
    books: Vec<Record>,
}

/// The imported stories (`duchinese.json`).
#[derive(Default)]
pub struct Records {
    pub books: Vec<Record>,
    path: Option<PathBuf>,
}

impl Records {
    pub fn path() -> PathBuf {
        hanstat::models::app_dir().join("duchinese.json")
    }

    /// An unreadable file is set aside by the saver (and reported).
    pub fn load() -> Records {
        Self::load_from(Self::path())
    }

    pub fn load_from(path: PathBuf) -> Records {
        let f: File = hanstat::saver::load_json(&path).unwrap_or_default();
        Records {
            books: f.books,
            path: Some(path),
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let Some(p) = &self.path else { return Ok(()) };
        let f = File {
            books: self.books.clone(),
        };
        hanstat::saver::write(p, serde_json::to_vec_pretty(&f).unwrap())
    }

    pub fn get(&self, course: u64) -> Option<&Record> {
        self.books.iter().find(|r| r.course.id == course)
    }

    pub fn get_mut(&mut self, course: u64) -> Option<&mut Record> {
        self.books.iter_mut().find(|r| r.course.id == course)
    }

    /// The record of the book at `path`.
    pub fn by_path(&self, path: &Path) -> Option<&Record> {
        self.books.iter().find(|r| same_file(&r.path, path))
    }

    pub fn put(&mut self, r: Record) {
        match self.books.iter_mut().find(|x| x.course.id == r.course.id) {
            Some(x) => *x = r,
            None => self.books.push(r),
        }
    }
}

pub fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || (cfg!(windows)
            && a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase())
}

// The cache.

pub fn cache_root() -> PathBuf {
    hanstat::models::cache_dir().join("duchinese")
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Catalogue {
    /// When it was fetched (seconds since the Unix epoch).
    pub fetched: u64,
    pub courses: Vec<Course>,
}

impl Catalogue {
    fn path(root: &Path) -> PathBuf {
        root.join("catalogue.json")
    }

    pub fn load(root: &Path) -> Catalogue {
        std::fs::read(Self::path(root))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, root: &Path) -> Result<(), String> {
        write_atomic(&Self::path(root), &serde_json::to_vec(self).unwrap())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Chapters {
    pub fetched: u64,
    pub lessons: Vec<Lesson>,
}

impl Chapters {
    fn path(root: &Path, course: u64) -> PathBuf {
        root.join("lessons").join(format!("{course}.json"))
    }

    pub fn load(root: &Path, course: u64) -> Option<Chapters> {
        std::fs::read(Self::path(root, course))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
    }

    pub fn save(&self, root: &Path, course: u64) -> Result<(), String> {
        write_atomic(
            &Self::path(root, course),
            &serde_json::to_vec(self).unwrap(),
        )
    }
}

/// A chapter's text file, by lesson and text hash (a new hash: a new file).
pub fn crd_path(root: &Path, lesson: &str, hash: &str) -> PathBuf {
    let clean = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(80)
            .collect()
    };
    root.join("crd")
        .join(format!("{}-{}.json", clean(lesson), clean(hash)))
}

pub fn read_cached(path: &Path) -> Option<Vec<u8>> {
    std::fs::read(path).ok()
}

pub fn write_cached(path: &Path, bytes: &[u8]) -> Result<(), String> {
    write_atomic(path, bytes)
}

pub fn cover_path(root: &Path, course: u64) -> PathBuf {
    root.join("covers").join(format!("{course}.img"))
}

/// How much of a story's first chapter is known, worked out for a set of
/// known words (`key`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Estimate {
    /// The known words and settings it was worked out with, and the text's hash.
    pub key: String,
    /// Percent of running words known (names left out).
    pub percent: f64,
    /// The chapter it was worked out from (1-based).
    pub chapter: u32,
}

pub fn load_estimates(root: &Path) -> BTreeMap<u64, Estimate> {
    std::fs::read(root.join("estimates.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

pub fn save_estimates(root: &Path, e: &BTreeMap<u64, Estimate>) -> Result<(), String> {
    write_atomic(
        &root.join("estimates.json"),
        &serde_json::to_vec(e).unwrap(),
    )
}

/// A file name for a book titled `title`: no characters Windows refuses,
/// not too long.
pub fn file_name(title: &str, traditional: bool) -> String {
    let mut s: String = title
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') {
                ' '
            } else {
                c
            }
        })
        .collect();
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s = s.trim_matches(['.', ' ']).chars().take(100).collect();
    if s.is_empty() {
        s = "DuChinese story".to_owned();
    }
    if traditional {
        s.push_str(" (繁)");
    }
    format!("{s}.epub")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(announced: (u32, u32), have: usize, locked: u32, last: &str) -> Record {
        Record {
            course: Course {
                lesson_count: announced.0,
                placeholder_count: announced.1,
                ..Course::default()
            },
            chapters: (0..have)
                .map(|i| ChapterRec {
                    lesson: i.to_string(),
                    release: last.to_owned(),
                    ..ChapterRec::default()
                })
                .collect(),
            locked,
            ..Record::default()
        }
    }

    #[test]
    fn progress_and_checks() {
        let now = super::super::date_secs("2026-11-23").unwrap();
        let done = rec((6, 0), 6, 0, "2022-03-09T05:00:00.000+01:00");
        assert_eq!(done.progress(), "6 chapters");
        assert!(!done.ongoing() && !done.check_due(now));
        let growing = rec((2, 8), 2, 0, "2026-11-20T06:00:00.000+01:00");
        assert_eq!(growing.progress(), "2 of 10 · ongoing");
        assert!(growing.check_due(now));
        let partial = rec((22, 0), 1, 21, "2025-01-01");
        assert_eq!(partial.progress(), "1 of 22 · 21 locked");
        assert!(partial.check_due(now), "locked chapters may open up");
        let recent = rec((9, 0), 9, 0, "2026-11-13T07:00:00.000+01:00");
        assert!(recent.check_due(now), "a series out lately may grow");
    }

    #[test]
    fn new_chapters_fade() {
        let mut r = rec((3, 0), 3, 0, "");
        r.added = 2;
        r.added_at = 1000;
        assert_eq!(r.new_chapters(1000 + 3600), 2);
        assert_eq!(r.new_chapters(1000 + NEW_FOR + 1), 0);
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("Fan Li ", false), "Fan Li.epub");
        assert_eq!(file_name("A: B/C?", true), "A B C (繁).epub");
        assert_eq!(file_name("...", false), "DuChinese story.epub");
    }

    #[test]
    fn records_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("duchinese.json");
        let mut r = Records::load_from(p.clone());
        assert!(r.books.is_empty());
        let mut a = rec((2, 0), 2, 0, "2026-01-01");
        a.course.id = 5;
        a.path = dir.path().join("x.epub");
        r.put(a.clone());
        r.save().unwrap();
        hanstat::saver::flush();
        let back = Records::load_from(p);
        assert_eq!(back.books, vec![a.clone()]);
        assert!(back.by_path(&dir.path().join("x.epub")).is_some());
        assert_eq!(back.get(5).unwrap().chapters.len(), 2);
    }
}
