//! Each EPUB's chapter lengths, as the reader counts characters (its places
//! are offsets in them), so that how far through a book the reader is can
//! be shown without building every chapter again.
//!
//! Saved in `chapter-lengths.json` in the app folder. It is a cache: lost or
//! unreadable, the lengths are worked out again. Books are keyed by their
//! Library id (the file's SHA-256), so a changed file is a new book; the
//! whole file is dropped when the reader counts text differently
//! (`HANSTAT_READER_TEXT_FINGERPRINT`).

use crate::reader;
use crate::reading_stats::LeftAt;
use eframe::egui;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

const FILE: &str = "chapter-lengths.json";
const READER: &str = env!("HANSTAT_READER_TEXT_FINGERPRINT");

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    /// The reader's text fingerprint the lengths were counted with.
    reader: String,
    books: BTreeMap<String, Vec<usize>>,
}

#[derive(Default)]
struct Store {
    loaded: bool,
    books: HashMap<String, Arc<[usize]>>,
    /// Books waiting to be worked out, in order.
    todo: VecDeque<(String, PathBuf)>,
    /// Books waiting or being worked out.
    queued: HashSet<String>,
    /// Books that couldn't be opened, this run: not tried again.
    failed: HashSet<String>,
    running: bool,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(Mutex::default);

fn path() -> PathBuf {
    hanstat::models::app_dir().join(FILE)
}

fn store() -> MutexGuard<'static, Store> {
    let mut s = STORE.lock().unwrap_or_else(|e| e.into_inner());
    // Tests never read (or write) the user's file.
    if !s.loaded && !cfg!(test) {
        s.loaded = true;
        if let Some(saved) = hanstat::saver::load_json::<Saved>(&path())
            && saved.reader == READER
        {
            s.books = saved
                .books
                .into_iter()
                .map(|(id, l)| (id, Arc::from(l)))
                .collect();
        }
    }
    s
}

fn snapshot(s: &Store) -> Saved {
    Saved {
        reader: READER.to_owned(),
        books: s
            .books
            .iter()
            .map(|(id, l)| (id.clone(), l.to_vec()))
            .collect(),
    }
}

fn write_saved(saved: &Saved) {
    if cfg!(test) {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(saved) {
        // Errors reach the status bar through `saver::take_error`.
        let _ = hanstat::saver::write(&path(), bytes);
    }
}

fn save_locked(s: MutexGuard<'static, Store>) {
    if cfg!(test) {
        return;
    }
    let saved = snapshot(&s);
    drop(s);
    write_saved(&saved);
}

/// The book's chapter lengths, if worked out.
pub(crate) fn get(id: &str) -> Option<Arc<[usize]>> {
    store().books.get(id).cloned()
}

/// The chapter lengths of `epub` (the book `id`): saved ones, or worked out
/// now and saved. For a thread that has the book open already.
pub(crate) fn of_open(id: &str, epub: &reader::epub::Epub) -> Arc<[usize]> {
    compute_and_store(id, epub, true)
}

fn compute_and_store(id: &str, epub: &reader::epub::Epub, flush: bool) -> Arc<[usize]> {
    if let Some(l) = get(id) {
        return l;
    }
    let l: Arc<[usize]> = reader::chapter_text_lens(epub).into();
    let mut s = store();
    s.books.insert(id.to_owned(), l.clone());
    if flush {
        save_locked(s);
    }
    l
}

/// Work out the lengths of the EPUB `path` (the book `id`) in the
/// background, unless they are known, waiting, or it couldn't be opened.
pub(crate) fn want(id: &str, path: &std::path::Path, ctx: &egui::Context) {
    let mut s = store();
    if s.books.contains_key(id) || s.queued.contains(id) || s.failed.contains(id) {
        return;
    }
    s.queued.insert(id.to_owned());
    s.todo.push_back((id.to_owned(), path.to_owned()));
    if !s.running {
        s.running = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || run(ctx));
    }
}

fn run(ctx: egui::Context) {
    const SAVE_EVERY: usize = 25;
    let mut unsaved = 0usize;
    loop {
        let Some((id, path)) = ({
            let mut s = store();
            let next = s.todo.pop_front();
            if next.is_none() {
                s.running = false;
                if unsaved > 0 {
                    unsaved = 0;
                    save_locked(s);
                }
            }
            next
        }) else {
            return;
        };
        match reader::epub::Epub::open(&path) {
            Ok(epub) => {
                compute_and_store(&id, &epub, false);
                unsaved += 1;
                if unsaved >= SAVE_EVERY {
                    unsaved = 0;
                    save_locked(store());
                }
            }
            Err(_) => {
                store().failed.insert(id.clone());
            }
        }
        store().queued.remove(&id);
        ctx.request_repaint();
    }
}

/// Forget a book's lengths (it was removed from the Library).
pub(crate) fn forget(id: &str) {
    let mut s = store();
    let removed = s.books.remove(id).is_some();
    s.failed.remove(id);
    if removed {
        save_locked(s);
    }
}

/// How far through the book the place `at` is, 0..=1.
pub(crate) fn through(chapters: &[usize], at: LeftAt) -> f32 {
    let total: usize = chapters.iter().sum();
    if total == 0 {
        return 0.0;
    }
    let spine = (at.spine as usize).min(chapters.len());
    let before: usize = chapters[..spine].iter().sum();
    let here = chapters
        .get(spine)
        .map_or(0, |&n| (at.offset as usize).min(n));
    ((before + here) as f32 / total as f32).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn through_counts_chapters_before_and_the_offset() {
        let ch = [100, 300, 600];
        let at = |spine, offset| LeftAt {
            spine,
            offset,
            saved: 0,
        };
        assert_eq!(through(&ch, at(0, 0)), 0.0);
        assert!((through(&ch, at(1, 150)) - 0.25).abs() < 1e-6);
        assert_eq!(through(&ch, at(2, 600)), 1.0);
        // A place past the end (the book changed) stays within the book.
        assert_eq!(through(&ch, at(7, 5)), 1.0);
        assert_eq!(through(&ch, at(1, 10_000)), 0.4);
        assert_eq!(through(&[], at(0, 0)), 0.0);
    }

    #[test]
    fn saved_with_another_reader_is_dropped() {
        let saved: Saved = serde_json::from_str(r#"{"reader":"x","books":{"a":[1,2]}}"#).unwrap();
        assert_ne!(saved.reader, READER);
        let round = Saved {
            reader: READER.into(),
            books: [("a".to_owned(), vec![3, 4])].into(),
        };
        let back: Saved = serde_json::from_slice(&serde_json::to_vec(&round).unwrap()).unwrap();
        assert_eq!(back.reader, READER);
        assert_eq!(back.books["a"], vec![3, 4]);
    }

    #[test]
    fn forget_drops_lengths() {
        store().books.insert("t1".into(), Arc::from(vec![5usize]));
        assert!(get("t1").is_some());
        forget("t1");
        assert!(get("t1").is_none());
    }

    /// The saved lengths agree with the reader's own count, on a real book
    /// from `HANSTAT_TEST_EPUBS` (a folder of EPUBs); skipped without one.
    #[test]
    fn lengths_match_the_reader_on_a_real_epub() {
        let Some(dir) = std::env::var_os("HANSTAT_TEST_EPUBS") else {
            return;
        };
        let Some(p) = std::fs::read_dir(&dir).ok().and_then(|d| {
            d.filter_map(|e| e.ok().map(|e| e.path()))
                .find(|p| p.extension().is_some_and(|x| x == "epub"))
        }) else {
            return;
        };
        let epub = reader::epub::Epub::open(&p).unwrap();
        let l = of_open("real-epub-test", &epub);
        assert_eq!(l.len(), epub.spine.len());
        eprintln!(
            "{}: {} chapters, {} characters",
            p.display(),
            l.len(),
            l.iter().sum::<usize>()
        );
        for (n, href) in l.iter().zip(&epub.spine) {
            assert_eq!(*n, reader::chapter_text_len(&epub, href));
        }
        forget("real-epub-test");
    }
}
