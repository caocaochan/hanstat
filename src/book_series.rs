//! The series each EPUB in the Library names in its metadata
//! ([`hanstat::series::read_epub`]), read once per book in the background
//! and saved in `book-series.json` in the app folder. A cache: lost or
//! unreadable, the books are read again (a few milliseconds each).
//! `None` (the book names no series) is saved too, so a book is read once.

use eframe::egui;
use hanstat::series::Meta;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex, MutexGuard};

const FILE: &str = "book-series.json";
/// Bump when the reading rule changes: every book is read again.
const VERSION: u32 = 1;

static REVISION: AtomicU64 = AtomicU64::new(0);

pub(crate) fn revision() -> u64 {
    REVISION.load(Ordering::Relaxed)
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    version: u32,
    books: BTreeMap<String, Option<Meta>>,
}

#[derive(Default)]
struct Store {
    loaded: bool,
    books: BTreeMap<String, Option<Meta>>,
    todo: VecDeque<(String, PathBuf)>,
    queued: HashSet<String>,
    /// Books whose file couldn't be read, this run.
    failed: HashSet<String>,
    running: bool,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(Mutex::default);

fn path() -> PathBuf {
    hanstat::models::app_dir().join(FILE)
}

fn store() -> MutexGuard<'static, Store> {
    let mut s = STORE.lock().unwrap_or_else(|e| e.into_inner());
    if !s.loaded && !cfg!(test) {
        s.loaded = true;
        if let Some(saved) = hanstat::saver::load_json::<Saved>(&path())
            && saved.version == VERSION
        {
            s.books = saved.books;
            REVISION.fetch_add(1, Ordering::Relaxed);
        }
    }
    s
}

fn save(s: &Store) {
    if cfg!(test) {
        return;
    }
    let saved = Saved {
        version: VERSION,
        books: s.books.clone(),
    };
    if let Ok(bytes) = serde_json::to_vec(&saved) {
        let _ = hanstat::saver::write(&path(), bytes);
    }
}

/// The series the book `id` names in its metadata; `None` also while it
/// hasn't been read yet.
pub(crate) fn meta(id: &str) -> Option<Meta> {
    store().books.get(id).cloned().flatten()
}

/// Every book's series known now, by id (books naming none left out).
pub(crate) fn all() -> BTreeMap<String, Meta> {
    store()
        .books
        .iter()
        .filter_map(|(id, m)| Some((id.clone(), m.clone()?)))
        .collect()
}

/// Read the metadata of the EPUBs among `books` (id, file) not read yet,
/// in the background.
pub(crate) fn want<'a>(
    books: impl Iterator<Item = (&'a str, &'a std::path::Path)>,
    ctx: &egui::Context,
) {
    let mut s = store();
    let mut added = false;
    for (id, p) in books {
        if s.books.contains_key(id) || s.queued.contains(id) || s.failed.contains(id) {
            continue;
        }
        s.queued.insert(id.to_owned());
        s.todo.push_back((id.to_owned(), p.to_owned()));
        added = true;
    }
    if added && !s.running {
        s.running = true;
        let ctx = ctx.clone();
        std::thread::spawn(move || run(ctx));
    }
}

fn run(ctx: egui::Context) {
    let mut since_save = 0;
    loop {
        let next = {
            let mut s = store();
            let next = s.todo.pop_front();
            if next.is_none() {
                s.running = false;
                if since_save > 0 {
                    save(&s);
                }
            }
            next
        };
        let Some((id, path)) = next else {
            ctx.request_repaint();
            return;
        };
        let read = hanstat::series::read_epub(&path);
        let mut s = store();
        s.queued.remove(&id);
        match read {
            Ok(m) => {
                s.books.insert(id, m);
                REVISION.fetch_add(1, Ordering::Relaxed);
                since_save += 1;
                // Every so often on a large Library, not after each book.
                if since_save >= 50 {
                    since_save = 0;
                    save(&s);
                }
            }
            Err(_) => {
                s.failed.insert(id);
            }
        }
    }
}

/// Forget a book (removed from the Library, or its file replaced).
pub(crate) fn forget(id: &str) {
    let mut s = store();
    if s.books.remove(id).is_some() {
        REVISION.fetch_add(1, Ordering::Relaxed);
        save(&s);
    }
    s.failed.remove(id);
}
