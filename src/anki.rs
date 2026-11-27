//! Known-vocabulary statistics from the user's local Anki collection.
//!
//! The collection is never opened in place: Anki keeps it open with an
//! exclusive lock while running, so `collection.anki2` (and its `-wal`, which
//! holds changes not yet checkpointed) are copied to a private folder under
//! [`copies_root`] and read from there. Nothing is ever written back. The copy
//! is kept between runs and brought up to date by writing only the blocks of
//! the file Anki changed, or nothing when the collection hasn't changed.
//!
//! Classification follows Anki's own Card Counts definitions: a review card is
//! *mature* when its interval is ≥ 21 days and *young* otherwise; learning and
//! relearning (lapsed) cards are *learning*; unstudied cards are *new*.
//! Optionally, a review card is mature when its FSRS stability (`s` in the
//! `cards.data` JSON) reaches a chosen number of days instead, which doesn't
//! shift with desired retention the way the interval does; cards without FSRS
//! data keep the interval rule. A note
//! takes the best status of its cards in the selected decks, a word the best
//! status of the notes it appears in, and a character the best status of the
//! words containing it.
//!
//! The user's choice of collection, decks, note types and fields is saved in
//! `%LOCALAPPDATA%\hanstat\anki.json`, in [`crate::models::app_dir`].

use crate::chars;
use crate::script::{self, Variant};
use crate::segment::{Segmenter, SegmenterKind};
use regex::Regex;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Anki's maturity threshold, in days.
pub const MATURE_IVL: i64 = 21;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Status {
    /// Not in any selected note.
    #[default]
    NotInAnki,
    New,
    Learning,
    /// Not young/mature in Anki, but in a ticked level (HSK, TOCFL) or an imported word
    /// list (see `crate::known`). Counts as known.
    Listed,
    Young,
    Mature,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Status::NotInAnki => "Not in Anki",
            Status::New => "New",
            Status::Learning => "Learning",
            Status::Listed => "Word list",
            Status::Young => "Young",
            Status::Mature => "Mature",
        }
    }

    /// Young or mature (what Anki considers successfully reviewed), or in a
    /// word list marked known.
    pub fn is_known(self) -> bool {
        self >= Status::Listed
    }
}

/// A count per [`Status`].
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct ByStatus {
    pub mature: u64,
    pub young: u64,
    pub learning: u64,
    pub new: u64,
    pub not_in_anki: u64,
    /// Always 0 in an Anki summary.
    pub listed: u64,
}

impl ByStatus {
    pub fn add(&mut self, s: Status, n: u64) {
        *match s {
            Status::NotInAnki => &mut self.not_in_anki,
            Status::New => &mut self.new,
            Status::Learning => &mut self.learning,
            Status::Listed => &mut self.listed,
            Status::Young => &mut self.young,
            Status::Mature => &mut self.mature,
        } += n;
    }

    pub fn get(&self, s: Status) -> u64 {
        match s {
            Status::NotInAnki => self.not_in_anki,
            Status::New => self.new,
            Status::Learning => self.learning,
            Status::Listed => self.listed,
            Status::Young => self.young,
            Status::Mature => self.mature,
        }
    }

    /// Mature + young (+ listed).
    pub fn known(&self) -> u64 {
        self.mature + self.young + self.listed
    }
}

// ---------------------------------------------------------------------------
// Profiles and saved settings
// ---------------------------------------------------------------------------

pub struct Profile {
    pub name: String,
    pub collection: PathBuf,
}

/// Anki's data folder: `ANKI_BASE` if set (as Anki itself honours), else
/// `%APPDATA%\Anki2`, on macOS `~/Library/Application Support/Anki2`, on
/// Linux the first of [`linux_anki_bases`] that exists.
pub fn anki_base() -> Option<PathBuf> {
    if let Some(b) = std::env::var_os("ANKI_BASE") {
        return Some(PathBuf::from(b));
    }
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Anki2"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/Anki2"))
    } else {
        let bases = linux_anki_bases();
        bases.iter().find(|b| b.is_dir()).or(bases.first()).cloned()
    }
}

/// Where Anki keeps its data on Linux: `$XDG_DATA_HOME/Anki2` (normally
/// `~/.local/share/Anki2`), then Anki's Flatpak (`net.ankiweb.Anki`). In
/// hanstat's own Flatpak `XDG_DATA_HOME` is hanstat's sandbox folder; the
/// host's is in `HOST_XDG_DATA_HOME` when it was set.
fn linux_anki_bases() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data = if std::env::var_os("FLATPAK_ID").is_some() {
        std::env::var_os("HOST_XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")))
    } else {
        crate::models::xdg_dir("XDG_DATA_HOME", ".local/share")
    };
    data.map(|d| d.join("Anki2"))
        .into_iter()
        .chain(home.map(|h| h.join(".var/app/net.ankiweb.Anki/data/Anki2")))
        .collect()
}

/// Every profile folder that contains a `collection.anki2`.
pub fn find_profiles() -> Vec<Profile> {
    let Some(base) = anki_base() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut v: Vec<Profile> = rd
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let c = e.path().join("collection.anki2");
            c.is_file().then(|| Profile {
                name: e.file_name().to_string_lossy().into_owned(),
                collection: c,
            })
        })
        .collect();
    v.sort_by_key(|p| p.name.to_lowercase());
    v
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeckRef {
    pub id: i64,
    /// Full name with `::` separators; used if the id no longer exists.
    pub name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoteTypeSel {
    pub id: i64,
    pub name: String,
    /// Field names (Anki has no stable field ids).
    pub fields: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub collection: Option<PathBuf>,
    /// Selecting a deck includes its subdecks.
    pub decks: Vec<DeckRef>,
    pub note_types: Vec<NoteTypeSel>,
    /// Classify suspended cards by their interval (true) or ignore them (false).
    pub include_suspended: bool,
    /// Fold Traditional to Simplified on both sides before matching.
    pub match_across_scripts: bool,
    /// Judge maturity by FSRS stability instead of the interval.
    pub mature_by_stability: bool,
    /// The stability, in days, at which a review card is mature.
    pub mature_stability_days: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: false,
            collection: None,
            decks: Vec::new(),
            note_types: Vec::new(),
            include_suspended: true,
            match_across_scripts: true,
            mature_by_stability: false,
            mature_stability_days: MATURE_IVL as u32,
        }
    }
}

impl Settings {
    /// The settings file (`anki.json`).
    pub fn path() -> PathBuf {
        crate::models::app_dir().join("anki.json")
    }

    /// Missing file → defaults. An unreadable one is kept aside and
    /// reported by the saver ([`crate::saver::load_json`]); defaults are used.
    pub fn load() -> (Settings, Option<String>) {
        (
            crate::saver::load_json(&Self::path()).unwrap_or_default(),
            None,
        )
    }

    pub fn save(&self) -> Result<(), String> {
        crate::saver::write(&Self::path(), serde_json::to_vec_pretty(self).unwrap())
    }

    /// The stability threshold in days, if maturity is judged by stability.
    pub fn stability_threshold(&self) -> Option<u32> {
        self.mature_by_stability
            .then_some(self.mature_stability_days.max(1))
    }

    pub fn has_selection(&self) -> bool {
        !self.decks.is_empty() && self.note_types.iter().any(|n| !n.fields.is_empty())
    }

    pub fn deck_selected(&self, d: &Deck) -> bool {
        self.decks.iter().any(|r| r.id == d.id)
    }

    pub fn set_deck(&mut self, d: &Deck, on: bool) {
        self.decks.retain(|r| r.id != d.id);
        if on {
            self.decks.push(DeckRef {
                id: d.id,
                name: d.name.clone(),
            });
        }
    }

    pub fn field_selected(&self, nt: &NoteType, field: &str) -> bool {
        self.note_types
            .iter()
            .any(|s| s.id == nt.id && s.fields.iter().any(|f| f == field))
    }

    pub fn set_field(&mut self, nt: &NoteType, field: &str, on: bool) {
        let idx = match self.note_types.iter().position(|s| s.id == nt.id) {
            Some(i) => i,
            None => {
                self.note_types.push(NoteTypeSel {
                    id: nt.id,
                    name: nt.name.clone(),
                    fields: Vec::new(),
                });
                self.note_types.len() - 1
            }
        };
        let s = &mut self.note_types[idx];
        s.name = nt.name.clone();
        s.fields.retain(|f| f != field);
        if on {
            s.fields.push(field.to_owned());
            // Keep the note type's own field order.
            s.fields
                .sort_by_key(|f| nt.fields.iter().position(|x| x == f).unwrap_or(usize::MAX));
        }
        self.note_types.retain(|s| !s.fields.is_empty());
    }

    /// Re-bind saved ids to the collection: a deck or note type that was
    /// deleted and re-imported keeps its name but gets a new id.
    pub fn rebind(&mut self, info: &CollectionInfo) {
        for r in &mut self.decks {
            if let Some(d) = info.decks.iter().find(|d| d.id == r.id) {
                r.name = d.name.clone();
            } else if let Some(d) = info.decks.iter().find(|d| d.name == r.name) {
                r.id = d.id;
            }
        }
        for s in &mut self.note_types {
            if let Some(n) = info.note_types.iter().find(|n| n.id == s.id) {
                s.name = n.name.clone();
            } else if let Some(n) = info.note_types.iter().find(|n| n.name == s.name) {
                s.id = n.id;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Collection
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Deck {
    pub id: i64,
    /// Full name with `::` separators.
    pub name: String,
    /// Cards whose home deck is this deck or one of its subdecks.
    pub cards: u64,
}

impl Deck {
    pub fn depth(&self) -> usize {
        self.name.matches("::").count()
    }
    pub fn leaf(&self) -> &str {
        self.name.rsplit("::").next().unwrap_or(&self.name)
    }
}

#[derive(Clone, Debug)]
pub struct NoteType {
    pub id: i64,
    pub name: String,
    pub fields: Vec<String>,
    pub notes: u64,
}

#[derive(Clone, Debug)]
pub struct CollectionInfo {
    pub path: PathBuf,
    /// Sorted as in Anki's deck list; decks with no cards in their subtree are omitted.
    pub decks: Vec<Deck>,
    /// Note types in use first, then alphabetical.
    pub note_types: Vec<NoteType>,
}

/// How sentence fields are split into words: the same segmenter and forced
/// words as the book, so that Anki words and book words share boundaries.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SegConfig {
    /// `Auto` resolves by the script of the Anki sentences.
    pub kind: SegmenterKind,
    pub user_words: Vec<String>,
}

impl SegConfig {
    /// The book's resolved segmenter if a book is analysed; otherwise the
    /// chosen one, with Auto as LTP so that no large model is fetched just to
    /// summarise a collection.
    pub fn new(chosen: SegmenterKind, book: Option<SegmenterKind>, user_words: &[String]) -> Self {
        SegConfig {
            kind: book.unwrap_or(match chosen {
                SegmenterKind::Auto => SegmenterKind::Ltp,
                k => k,
            }),
            user_words: user_words.to_vec(),
        }
    }
}

/// A private copy of a collection, in its own folder under [`copies_root`].
/// The folder's lock file stays locked for as long as the copy is in use, so
/// another hanstat (or a later run) can tell a live copy from an idle or
/// abandoned one. A kept copy stays on disk when dropped, to be brought up to
/// date next time; any other copy is deleted.
struct CopyGuard {
    dir: PathBuf,
    lock: Option<std::fs::File>,
    keep: bool,
}

impl Drop for CopyGuard {
    fn drop(&mut self) {
        // Closed first: Windows cannot delete a folder holding an open file.
        drop(self.lock.take());
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// An open, private, read-only-by-convention copy of a collection.
pub struct Collection {
    pub info: std::sync::Arc<CollectionInfo>,
    conn: Connection,
    // Declared after `conn` so the database is closed before the folder is
    // unlocked (and, for a temporary copy, removed).
    _tmp: CopyGuard,
}

static COPY_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Where private copies of collections are made: `anki-copy` in
/// [`crate::models::cache_dir`], on disk. Not the temp folder, which on many
/// Linux systems is a RAM-backed tmpfs capped at half the RAM: a large
/// collection didn't fit there ("No space left on device").
pub fn copies_root() -> PathBuf {
    crate::models::cache_dir().join("anki-copy")
}

const LOCK_FILE: &str = "lock";
/// In a kept copy: what it was last brought up to date from.
const SOURCE_FILE: &str = "source.txt";
const DB_FILE: &str = "collection.anki2";
/// Kept copies are `kept-<hash of the collection's path>`; every other
/// folder is a temporary copy, `<process id>-<n>`.
const KEPT_PREFIX: &str = "kept-";
/// A kept copy is compared with the collection in blocks this large, and only
/// blocks that differ are written.
const BLOCK: usize = 1 << 20;
/// Attempts at a copy that the collection didn't change in the middle of.
const SYNC_TRIES: usize = 3;

/// Headroom on top of the files themselves: SQLite's `-shm` and growth while
/// the copied WAL is replayed.
const COPY_SLACK: u64 = 16 << 20;

/// The error when `free` bytes (if known) cannot hold `need`.
fn check_space(need: u64, free: Option<u64>, dir: &Path) -> Result<(), String> {
    match free {
        Some(free) if free < need => Err(format!(
            "Not enough free space for a private copy of the Anki collection: it needs {}, but {} has only {} free.",
            human_bytes(need),
            dir.display(),
            human_bytes(free)
        )),
        _ => Ok(()),
    }
}

fn human_bytes(n: u64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    let mb = n as f64 / MB;
    if mb >= 1024.0 {
        format!("{:.1} GB", mb / 1024.0)
    } else {
        format!("{mb:.0} MB")
    }
}

fn wal_of(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-wal");
    PathBuf::from(s)
}

fn shm_of(db: &Path) -> PathBuf {
    let mut s = db.as_os_str().to_owned();
    s.push("-shm");
    PathBuf::from(s)
}

/// A file's size, modification time and first bytes (all zero for no
/// file): what tells whether it changed since a kept copy was made. The first
/// bytes catch a WAL that Anki restarted at the same size within the clock's
/// resolution (its header's salts change each time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Stamp {
    len: u64,
    mtime_ns: u128,
    head: u64,
}

fn stamp(p: &Path) -> std::io::Result<Stamp> {
    let m = match std::fs::metadata(p) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stamp::default()),
        Err(e) => return Err(e),
    };
    let mtime_ns = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let mut head = [0u8; 32];
    let n = match std::fs::File::open(p) {
        Ok(mut f) => read_full(&mut f, &mut head)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Stamp::default()),
        Err(e) => return Err(e),
    };
    Ok(Stamp {
        len: m.len(),
        mtime_ns,
        head: crate::dict::xxh64::xxh64(&head[..n], 0),
    })
}

/// What [`SOURCE_FILE`] holds for a copy of `path` made at these stamps.
fn source_record(path: &Path, db: Stamp, wal: Stamp) -> String {
    format!(
        "{}\n{} {} {:016x}\n{} {} {:016x}\n",
        path.display(),
        db.len,
        db.mtime_ns,
        db.head,
        wal.len,
        wal.mtime_ns,
        wal.head
    )
}

/// The kept copy's folder for the collection at `path`.
fn kept_dir(root: &Path, path: &Path) -> PathBuf {
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    root.join(format!(
        "{KEPT_PREFIX}{:016x}",
        crate::dict::xxh64::xxh64(key.as_os_str().as_encoded_bytes(), 0)
    ))
}

/// How a copy came about, for the tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CopyReport {
    /// The kept copy (false: a temporary one, as the kept one was in use).
    kept: bool,
    /// Nothing had changed since the kept copy was last brought up to date.
    unchanged: bool,
    /// Bytes of the database written.
    written: u64,
}

enum Side {
    Source,
    Copy,
}

/// Fills `buf` from `r` as far as the file goes; the number of bytes read.
fn read_full(r: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// Makes `dst` the same as `src`, writing only the [`BLOCK`]s that differ.
/// Returns the bytes written.
fn delta_copy(src: &Path, dst: &Path) -> Result<u64, (Side, std::io::Error)> {
    use std::io::{Seek, SeekFrom, Write};
    let mut s = std::fs::File::open(src).map_err(|e| (Side::Source, e))?;
    let mut d = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dst)
        .map_err(|e| (Side::Copy, e))?;
    let mut a = vec![0u8; BLOCK];
    let mut b = vec![0u8; BLOCK];
    let (mut pos, mut written) = (0u64, 0u64);
    loop {
        let n = read_full(&mut s, &mut a).map_err(|e| (Side::Source, e))?;
        if n == 0 {
            break;
        }
        let same = d
            .seek(SeekFrom::Start(pos))
            .and_then(|_| read_full(&mut d, &mut b[..n]))
            .map_err(|e| (Side::Copy, e))?
            == n
            && a[..n] == b[..n];
        if !same {
            d.seek(SeekFrom::Start(pos))
                .and_then(|_| d.write_all(&a[..n]))
                .map_err(|e| (Side::Copy, e))?;
            written += n as u64;
        }
        pos += n as u64;
        if n < BLOCK {
            break;
        }
    }
    d.set_len(pos).map_err(|e| (Side::Copy, e))?;
    Ok(written)
}

/// Brings the copy in `dir` up to date with the collection at `path` (and its
/// `-wal`): nothing at all when [`SOURCE_FILE`] says it already is, else only
/// the blocks that differ, so an empty `dir` gets a full copy.
fn sync_copy(path: &Path, dir: &Path) -> Result<CopyReport, String> {
    let read_err =
        |p: &Path, e: std::io::Error| format!("Cannot read Anki collection {}: {e}", p.display());
    let wal = wal_of(path);
    let db = dir.join(DB_FILE);
    let record = dir.join(SOURCE_FILE);
    let stamps = || -> Result<(Stamp, Stamp), String> {
        Ok((
            stamp(path).map_err(|e| read_err(path, e))?,
            stamp(&wal).map_err(|e| read_err(&wal, e))?,
        ))
    };
    let (mut s_db, mut s_wal) = stamps()?;
    if s_db.len == 0 {
        return Err(read_err(path, std::io::ErrorKind::NotFound.into()));
    }
    // A -shm from the last reader is never reused: SQLite builds it again.
    let _ = std::fs::remove_file(shm_of(&db));
    if db.exists()
        && std::fs::read_to_string(&record).is_ok_and(|r| r == source_record(path, s_db, s_wal))
    {
        return Ok(CopyReport {
            kept: true,
            unchanged: true,
            written: 0,
        });
    }
    // Gone until the copy is whole again, so a copy cut short is never taken
    // for an up-to-date one.
    let _ = std::fs::remove_file(&record);

    let have = std::fs::metadata(&db).map_or(0, |m| m.len());
    // The WAL is counted twice: once copied, once replayed into the database.
    let need = s_db.len.saturating_sub(have) + 2 * s_wal.len + COPY_SLACK;
    check_space(need, crate::dict::manage::free_space(dir), dir)?;
    let copy_err = |what: &Path, e: std::io::Error| {
        let mut msg = format!("Cannot copy {} into {}: {e}", what.display(), dir.display());
        if e.kind() == std::io::ErrorKind::StorageFull {
            msg.push_str(&format!(
                " (needs about {}; free up space there or set XDG_CACHE_HOME to a folder on a larger disk)",
                human_bytes(need)
            ));
        }
        msg
    };

    let mut written = 0;
    for attempt in 1..=SYNC_TRIES {
        written += delta_copy(path, &db).map_err(|(side, e)| match side {
            Side::Source => read_err(path, e),
            Side::Copy => copy_err(path, e),
        })?;
        let dst_wal = wal_of(&db);
        let _ = std::fs::remove_file(&dst_wal);
        if s_wal.len > 0 {
            std::fs::copy(&wal, &dst_wal).map_err(|e| copy_err(&wal, e))?;
        }
        let (now_db, now_wal) = stamps()?;
        if (now_db, now_wal) == (s_db, s_wal) {
            std::fs::write(&record, source_record(path, s_db, s_wal))
                .map_err(|e| copy_err(path, e))?;
            break;
        }
        // Anki wrote to the collection while it was being copied: once more,
        // which only writes what changed. After the last try the copy is used
        // as it is (as hanstat always did) but not recorded as up to date.
        if attempt == SYNC_TRIES {
            break;
        }
        (s_db, s_wal) = (now_db, now_wal);
    }
    Ok(CopyReport {
        kept: true,
        unchanged: false,
        written,
    })
}

/// Locks the kept copy's folder for this process; `None` if another copy of
/// hanstat is using it.
fn take_kept(dir: &Path) -> Result<Option<std::fs::File>, String> {
    // Twice, in case another hanstat clearing stale copies removed the folder
    // between creating it and locking it.
    for _ in 0..2 {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let lock_path = dir.join(LOCK_FILE);
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| format!("Cannot create {}: {e}", lock_path.display()))?;
        match f.try_lock() {
            Ok(()) if lock_path.exists() => return Ok(Some(f)),
            Ok(()) => continue,
            Err(std::fs::TryLockError::WouldBlock) => return Ok(None),
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(format!("Cannot lock {}: {e}", lock_path.display()));
            }
        }
    }
    Ok(None)
}

/// A private, locked copy of the collection at `path` under `root`: the kept
/// copy, brought up to date, or, while another hanstat is using that, a
/// temporary full copy. Copies no running hanstat is using are cleared first.
fn make_copy(path: &Path, root: &Path) -> Result<(CopyGuard, PathBuf, CopyReport), String> {
    std::fs::metadata(path)
        .map_err(|e| format!("Cannot read Anki collection {}: {e}", path.display()))?;
    remove_stale_copies(root, Some(path));
    std::fs::create_dir_all(root).map_err(|e| format!("Cannot create {}: {e}", root.display()))?;

    let kept = kept_dir(root, path);
    if let Some(lock) = take_kept(&kept)? {
        let guard = CopyGuard {
            dir: kept.clone(),
            lock: Some(lock),
            keep: true,
        };
        let report = sync_copy(path, &kept)?;
        return Ok((guard, kept.join(DB_FILE), report));
    }

    let dir = root.join(format!(
        "{}-{}",
        std::process::id(),
        COPY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    // A folder left by an earlier process with the same id is stale.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir(&dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
    let mut guard = CopyGuard {
        dir: dir.clone(),
        lock: None,
        keep: false,
    };
    let lock = std::fs::File::create(dir.join(LOCK_FILE))
        .map_err(|e| format!("Cannot create {}: {e}", dir.join(LOCK_FILE).display()))?;
    lock.lock()
        .map_err(|e| format!("Cannot lock {}: {e}", dir.join(LOCK_FILE).display()))?;
    guard.lock = Some(lock);
    let report = sync_copy(path, &dir)?;
    Ok((
        guard,
        dir.join(DB_FILE),
        CopyReport {
            kept: false,
            ..report
        },
    ))
}

fn sql_err(e: rusqlite::Error) -> String {
    format!("Reading the Anki collection failed: {e}")
}

impl Collection {
    pub fn open(path: &Path) -> Result<Collection, String> {
        Self::open_in(path, &copies_root())
    }

    /// [`Collection::open`], with the private copy made under `root`.
    fn open_in(path: &Path, root: &Path) -> Result<Collection, String> {
        let (guard, db, _) = make_copy(path, root)?;
        // Read-write only so SQLite can replay the copied WAL into its own index.
        let conn = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(sql_err)?;
        // Closing must not fold the copied WAL into the database: the kept
        // copy's database then stays byte for byte Anki's, and the next update
        // writes only what Anki changed.
        conn.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .map_err(sql_err)?;
        // Anki's name columns use a custom collation; register an equivalent.
        conn.create_collation("unicase", |a, b| a.to_lowercase().cmp(&b.to_lowercase()))
            .map_err(sql_err)?;
        let ver: i64 = conn
            .query_row("SELECT ver FROM col", [], |r| r.get(0))
            .map_err(|e| format!("{} is not an Anki collection: {e}", path.display()))?;
        if ver < 15 {
            return Err(format!(
                "This collection uses schema {ver} (Anki older than 2.1.28). Open it once in a current Anki version to upgrade it."
            ));
        }

        // Cards per home deck (a card in a filtered deck belongs to `odid`).
        let mut per_deck: HashMap<i64, u64> = HashMap::new();
        {
            let mut st = conn
                .prepare("SELECT CASE WHEN odid != 0 THEN odid ELSE did END, count() FROM cards GROUP BY 1")
                .map_err(sql_err)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
                .map_err(sql_err)?;
            for r in rows {
                let (d, n) = r.map_err(sql_err)?;
                per_deck.insert(d, n as u64);
            }
        }
        let mut raw: Vec<(i64, String)> = {
            let mut st = conn
                .prepare("SELECT id, name FROM decks")
                .map_err(sql_err)?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql_err)?
                .collect::<Result<_, _>>()
                .map_err(sql_err)?
        };
        // Anki stores the hierarchy separator as U+001F; sorting on it puts
        // each deck directly before its children.
        raw.sort_by_key(|a| a.1.to_lowercase());
        let mut decks: Vec<Deck> = raw
            .iter()
            .map(|(id, name)| {
                let prefix = format!("{name}\x1f");
                let cards = raw
                    .iter()
                    .filter(|(_, n)| n == name || n.starts_with(&prefix))
                    .map(|(i, _)| per_deck.get(i).copied().unwrap_or(0))
                    .sum();
                Deck {
                    id: *id,
                    name: name.replace('\x1f', "::"),
                    cards,
                }
            })
            .collect();
        decks.retain(|d| d.cards > 0);

        let mut notes_per_type: HashMap<i64, u64> = HashMap::new();
        {
            let mut st = conn
                .prepare("SELECT mid, count() FROM notes GROUP BY mid")
                .map_err(sql_err)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))
                .map_err(sql_err)?;
            for r in rows {
                let (m, n) = r.map_err(sql_err)?;
                notes_per_type.insert(m, n as u64);
            }
        }
        let mut fields: HashMap<i64, Vec<String>> = HashMap::new();
        {
            let mut st = conn
                .prepare("SELECT ntid, name FROM fields ORDER BY ntid, ord")
                .map_err(sql_err)?;
            let rows = st
                .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(sql_err)?;
            for r in rows {
                let (nt, name) = r.map_err(sql_err)?;
                fields.entry(nt).or_default().push(name);
            }
        }
        let mut note_types: Vec<NoteType> = {
            let mut st = conn
                .prepare("SELECT id, name FROM notetypes")
                .map_err(sql_err)?;
            st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
                .map_err(sql_err)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_err)?
                .into_iter()
                .map(|(id, name)| NoteType {
                    id,
                    fields: fields.remove(&id).unwrap_or_default(),
                    notes: notes_per_type.get(&id).copied().unwrap_or(0),
                    name,
                })
                .collect()
        };
        note_types.sort_by(|a, b| {
            (a.notes == 0)
                .cmp(&(b.notes == 0))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });

        Ok(Collection {
            info: std::sync::Arc::new(CollectionInfo {
                path: path.to_owned(),
                decks,
                note_types,
            }),
            conn,
            _tmp: guard,
        })
    }

    /// Classify every word and character in the selected fields. Field values
    /// that look like sentences are split into words with `seg`, which should
    /// match the book's segmentation so that word boundaries agree.
    pub fn knowledge(&self, s: &Settings, seg: &SegConfig) -> Result<Knowledge, String> {
        let info = &self.info;
        // Selected decks, including subdecks.
        let roots: Vec<&str> = s
            .decks
            .iter()
            .filter_map(|r| {
                info.decks
                    .iter()
                    .find(|d| d.id == r.id)
                    .or_else(|| info.decks.iter().find(|d| d.name == r.name))
                    .map(|d| d.name.as_str())
            })
            .collect();
        let deck_ids: HashSet<i64> = all_deck_ids(&self.conn)?
            .into_iter()
            .filter(|(_, name)| {
                roots
                    .iter()
                    .any(|r| name == r || name.strip_prefix(r).is_some_and(|t| t.starts_with("::")))
            })
            .map(|(id, _)| id)
            .collect();
        // Note type id → field ordinals.
        let mut ords: HashMap<i64, Vec<usize>> = HashMap::new();
        for sel in &s.note_types {
            let Some(nt) = info
                .note_types
                .iter()
                .find(|n| n.id == sel.id)
                .or_else(|| info.note_types.iter().find(|n| n.name == sel.name))
            else {
                continue;
            };
            let o: Vec<usize> = sel
                .fields
                .iter()
                .filter_map(|f| nt.fields.iter().position(|x| x == f))
                .collect();
            if !o.is_empty() {
                ords.insert(nt.id, o);
            }
        }

        let threshold = s.stability_threshold();
        let mut summary = KnowledgeSummary {
            mature_stability_days: threshold,
            ..Default::default()
        };
        let mut note_status: HashMap<i64, Status> = HashMap::new();
        if !deck_ids.is_empty() && !ords.is_empty() {
            // FSRS stability is `s` in the card's data JSON (Anki 23.10+), present
            // only on cards with an FSRS memory state; `data` is often empty.
            let sql = if threshold.is_some() {
                "SELECT c.nid, n.mid, CASE WHEN c.odid != 0 THEN c.odid ELSE c.did END, c.type, c.queue, c.ivl, \
                 CASE WHEN json_valid(c.data) THEN json_extract(c.data, '$.s') END \
                 FROM cards c JOIN notes n ON n.id = c.nid"
            } else {
                "SELECT c.nid, n.mid, CASE WHEN c.odid != 0 THEN c.odid ELSE c.did END, c.type, c.queue, c.ivl, \
                 NULL \
                 FROM cards c JOIN notes n ON n.id = c.nid"
            };
            let mut st = self.conn.prepare(sql).map_err(sql_err)?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                        r.get::<_, Option<f64>>(6).unwrap_or(None),
                    ))
                })
                .map_err(sql_err)?;
            for r in rows {
                let (nid, mid, did, ctype, queue, ivl, stability) = r.map_err(sql_err)?;
                if !deck_ids.contains(&did) || !ords.contains_key(&mid) {
                    continue;
                }
                if queue == -1 && !s.include_suspended {
                    summary.suspended_excluded += 1;
                    continue;
                }
                let (mature, by_interval) = is_mature(ivl, stability, threshold);
                if by_interval && ctype == 2 {
                    summary.no_stability += 1;
                }
                let cs = card_status(ctype, mature);
                summary.cards.add(cs, 1);
                let e = note_status.entry(nid).or_insert(Status::New);
                *e = (*e).max(cs);
            }
        }

        // Sentences and long phrases, segmented together afterwards.
        let fold_on = s.match_across_scripts;
        let mut words: HashMap<String, Status> = HashMap::new();
        let mut add = |w: &str, st: Status| {
            let e = words.entry(fold(w, fold_on)).or_default();
            *e = (*e).max(st);
        };
        let mut to_segment: Vec<(String, Status)> = Vec::new();
        if !note_status.is_empty() {
            let mids: Vec<String> = ords.keys().map(|m| m.to_string()).collect();
            let mut st = self
                .conn
                .prepare(&format!(
                    "SELECT id, mid, flds FROM notes WHERE mid IN ({})",
                    mids.join(",")
                ))
                .map_err(sql_err)?;
            let mut rows = st.query([]).map_err(sql_err)?;
            let mut norm = chars::NormStats::default();
            while let Some(r) = rows.next().map_err(sql_err)? {
                let nid: i64 = r.get(0).map_err(sql_err)?;
                let Some(&ns) = note_status.get(&nid) else {
                    continue;
                };
                let mid: i64 = r.get(1).map_err(sql_err)?;
                let flds: String = r.get(2).map_err(sql_err)?;
                let parts: Vec<&str> = flds.split('\x1f').collect();
                summary.notes += 1;
                let mut any = false;
                for &o in &ords[&mid] {
                    let Some(f) = parts.get(o) else { continue };
                    let text = clean_field(f, &mut norm);
                    if han_runs(&text).next().is_none() {
                        continue;
                    }
                    any = true;
                    if looks_like_sentence(&text) {
                        summary.sentences += 1;
                        to_segment.push((text, ns));
                    } else {
                        for w in han_runs(&text) {
                            add(w, ns);
                            // An idiom or set phrase (睁一只眼闭一只眼): known as a
                            // whole, and its parts are what the book's segmenter
                            // will produce.
                            if w.chars().count() > MAX_WORD_RUN {
                                to_segment.push((w.to_owned(), ns));
                            }
                        }
                    }
                }
                if !any {
                    summary.notes_without_chinese += 1;
                }
            }
        }
        if !to_segment.is_empty() {
            let t = &*script::TABLES;
            let (mut tr, mut si) = (0u64, 0u64);
            for c in to_segment.iter().flat_map(|(x, _)| x.chars()) {
                tr += t.trad_only.contains(&c) as u64;
                si += t.simp_only.contains(&c) as u64;
            }
            // Small samples are "Unknown" to `classify`; the majority still tells
            // the segmenter whether to fold Traditional for its model.
            let variant = match script::classify(tr, si) {
                Variant::Unknown if tr > si => Variant::Traditional,
                Variant::Unknown if si > tr => Variant::Simplified,
                v => v,
            };
            let mut nd = chars::NormStats::default();
            let forced: Vec<String> = seg
                .user_words
                .iter()
                .map(|w| chars::normalize(w.trim(), &mut nd))
                .filter(|w| !w.is_empty())
                .collect();
            let segmenter = Segmenter::new(seg.kind, &forced, variant)?;
            let mut chunks: Vec<&str> = Vec::new();
            let mut chunk_status: Vec<Status> = Vec::new();
            for (text, st) in &to_segment {
                for r in han_runs(text) {
                    chunks.push(r);
                    chunk_status.push(*st);
                }
            }
            use rayon::prelude::*;
            let segmented: Vec<Vec<Vec<&str>>> = chunks
                .par_chunks(128)
                .map(|group| segmenter.cut_many(group))
                .collect();
            for (toks, st) in segmented.into_iter().flatten().zip(chunk_status) {
                toks.into_iter().for_each(|w| add(w, st));
            }
            summary.segmenter = Some(segmenter.kind);
        }
        let mut char_status: HashMap<char, Status> = HashMap::new();
        let mut add_chars = |w: &str, st: Status| {
            for c in w.chars() {
                let e = char_status.entry(c).or_default();
                *e = (*e).max(st);
            }
        };
        for (w, &st) in &words {
            summary.words.add(st, 1);
            add_chars(w, st);
        }
        for &st in char_status.values() {
            summary.chars.add(st, 1);
        }
        Ok(Knowledge {
            words,
            chars: char_status,
            fold: fold_on,
            summary,
            from_anki: true,
            listed: ListedSummary::default(),
        })
    }
}

fn all_deck_ids(conn: &Connection) -> Result<Vec<(i64, String)>, String> {
    let mut st = conn
        .prepare("SELECT id, name FROM decks")
        .map_err(sql_err)?;
    st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?.replace('\x1f', "::"),
        ))
    })
    .map_err(sql_err)?
    .collect::<Result<_, _>>()
    .map_err(sql_err)
}

/// Copies left behind by a crashed run, older than a day.
/// Deletes private copies no running hanstat is using: under `root`, every
/// temporary copy whose lock file can be locked (its owner has exited or
/// crashed), however old, and every kept copy not in use except the one for
/// `keep` (so with `None`, all of them: Anki statistics are off); and every
/// copy from before 2026.11.25, which went into the temp folder as
/// `hanstat-anki-*` with no lock file.
pub fn remove_stale_copies(root: &Path, keep: Option<&Path>) {
    let keep = keep.map(|p| kept_dir(root, p));
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let dir = e.path();
            if !dir.is_dir() || keep.as_ref() == Some(&dir) {
                continue;
            }
            let lock = dir.join(LOCK_FILE);
            let stale = match std::fs::OpenOptions::new().write(true).open(&lock) {
                // Locked here means nobody else holds it. Kept locked until the
                // folder is gone, then closed (Windows deletes the closed file
                // on a second pass).
                Ok(f) => match f.try_lock() {
                    Ok(()) => {
                        let _ = std::fs::remove_dir_all(&dir);
                        drop(f);
                        true
                    }
                    Err(_) => false,
                },
                // A folder whose lock file isn't made yet is being set up by
                // someone right now, unless it has sat like that for a while.
                Err(_) => e
                    .metadata()
                    .and_then(|m| m.modified())
                    .is_ok_and(|t| t.elapsed().is_ok_and(|age| age.as_secs() > 3600)),
            };
            if stale {
                let _ = std::fs::remove_dir_all(&dir);
            }
        }
    }
    if let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) {
        for e in rd.flatten() {
            if e.file_name().to_string_lossy().starts_with("hanstat-anki-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Whether a review card counts as mature: by FSRS stability when `threshold`
/// is set and the card has one, otherwise by Anki's interval rule. The second
/// value is true when a stability threshold was set but the card had none.
fn is_mature(ivl: i64, stability: Option<f64>, threshold: Option<u32>) -> (bool, bool) {
    match (threshold, stability) {
        (Some(t), Some(s)) => (s >= t as f64, false),
        (Some(_), None) => (ivl >= MATURE_IVL, true),
        (None, _) => (ivl >= MATURE_IVL, false),
    }
}

/// Anki card `type`: 0 new, 1 learning, 2 review, 3 relearning. `mature`
/// only matters for review cards.
fn card_status(ctype: i64, mature: bool) -> Status {
    match ctype {
        2 if mature => Status::Mature,
        2 => Status::Young,
        1 | 3 => Status::Learning,
        _ => Status::New,
    }
}

static RE_CLOZE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)\{\{c\d+::(.*?)(?:::[^}]*)?\}\}").unwrap());
static RE_STYLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<(?:style|script)\b.*?</(?:style|script)\s*>").unwrap());
static RE_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)<\s*/?\s*(?:br|div|p|li|ul|ol|tr|td|th|h[1-6]|hr|table)\b[^>]*>").unwrap()
});
static RE_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
/// `[sound:…]`, and readings in Anki's `漢字[かんじ]` / `你好[nǐhǎo]` style.
static RE_BRACKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[^\]]*\]").unwrap());
static RE_NUM_ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&#(?:[xX]([0-9a-fA-F]+)|([0-9]+));").unwrap());

fn replace_if(s: &mut std::borrow::Cow<'_, str>, needle: &str, re: &Regex, rep: &str) {
    if s.contains(needle) {
        let next = match re.replace_all(s, rep) {
            std::borrow::Cow::Owned(r) => Some(r),
            std::borrow::Cow::Borrowed(_) => None,
        };
        if let Some(r) = next {
            *s = std::borrow::Cow::Owned(r);
        }
    }
}

/// One field's text with HTML, cloze markup, sound tags and bracketed readings
/// removed, NFC-normalised as book text is.
pub fn clean_field(field: &str, norm: &mut chars::NormStats) -> String {
    let mut s = std::borrow::Cow::Borrowed(field);
    replace_if(&mut s, "{{", &RE_CLOZE, "$1");
    if s.contains('<') {
        replace_if(&mut s, "<", &RE_STYLE, " ");
        replace_if(&mut s, "<", &RE_BLOCK, " ");
        // Inline tags (<b>, <span>) must not split a word.
        replace_if(&mut s, "<", &RE_TAG, "");
    }
    replace_if(&mut s, "[", &RE_BRACKET, " ");
    if s.contains("&#") {
        let next = match RE_NUM_ENTITY.replace_all(&s, |c: &regex::Captures| {
            let n = match (c.get(1), c.get(2)) {
                (Some(h), _) => u32::from_str_radix(h.as_str(), 16).ok(),
                (_, Some(d)) => d.as_str().parse().ok(),
                _ => None,
            };
            n.and_then(char::from_u32)
                .map(String::from)
                .unwrap_or_default()
        }) {
            std::borrow::Cow::Owned(r) => Some(r),
            std::borrow::Cow::Borrowed(_) => None,
        };
        if let Some(r) = next {
            s = std::borrow::Cow::Owned(r);
        }
    }
    chars::normalize(&s, norm)
}

/// Maximal runs of Han characters.
pub fn han_runs(s: &str) -> impl Iterator<Item = &str> {
    s.split(|c: char| !chars::is_han(c))
        .filter(|w| !w.is_empty())
}

/// Longest run that is only a word. Longer runs without punctuation (idioms,
/// sayings) count as a word *and* are segmented, because the book's segmenter
/// usually splits them too. Chengyu are 4 characters.
const MAX_WORD_RUN: usize = 4;

/// A value is a sentence (segmented only, never a word itself) if it has clause
/// or sentence punctuation. List separators (、 ; / ·) don't count:
/// "你好、再見" is two words.
pub fn looks_like_sentence(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '。' | '，'
                | '！'
                | '？'
                | '!'
                | '?'
                | '…'
                | '：'
                | '「'
                | '」'
                | '『'
                | '』'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '《'
                | '》'
        )
    })
}

/// The Chinese words in one word-like field (each run of Han characters is one word).
#[cfg(test)]
fn field_words(field: &str, norm: &mut chars::NormStats) -> Vec<String> {
    han_runs(&clean_field(field, norm))
        .map(str::to_owned)
        .collect()
}

/// Traditional → Simplified, character by character, when `on`: how
/// [`Knowledge`] keys its words.
pub fn fold(s: &str, on: bool) -> String {
    if !on {
        return s.to_owned();
    }
    let t = &script::TABLES.t2s;
    s.chars().map(|c| t.get(&c).copied().unwrap_or(c)).collect()
}

/// [`fold`] for one character.
pub fn fold_char(c: char, on: bool) -> char {
    if on {
        script::TABLES.t2s.get(&c).copied().unwrap_or(c)
    } else {
        c
    }
}

// ---------------------------------------------------------------------------
// Knowledge and book coverage
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, Serialize)]
pub struct KnowledgeSummary {
    /// Selected notes with at least one card in the selected decks.
    pub notes: u64,
    /// Of those, notes whose selected fields contain no Chinese characters.
    pub notes_without_chinese: u64,
    /// Field values that looked like sentences and were split into words.
    pub sentences: u64,
    /// The segmenter used for them.
    pub segmenter: Option<SegmenterKind>,
    pub cards: ByStatus,
    pub suspended_excluded: u64,
    /// Mature at this FSRS stability (days); `None`: at an interval of 21+ days.
    pub mature_stability_days: Option<u32>,
    /// Review cards judged by interval because they have no FSRS stability
    /// (only counted with a stability threshold).
    pub no_stability: u64,
    /// Distinct words (Han runs) by status.
    pub words: ByStatus,
    /// Distinct characters by status.
    pub chars: ByStatus,
}

/// Words marked known from HSK bands and word lists (`crate::known`).
#[derive(Clone, Debug, Default, Serialize)]
pub struct ListedSummary {
    /// What was marked known, e.g. "HSK 1–3 + 2 word lists"; empty if nothing.
    pub source: String,
    /// Distinct words in those lists (after Traditional → Simplified folding, if on).
    pub words: u64,
    /// Of those, words that were not young/mature in Anki.
    pub new_words: u64,
    /// Characters known only through those words.
    pub new_chars: u64,
}

#[derive(Clone)]
pub struct Knowledge {
    /// Keyed by the (optionally T→S folded) word.
    pub words: HashMap<String, Status>,
    pub chars: HashMap<char, Status>,
    pub fold: bool,
    /// Anki's own counts (empty if Anki is off).
    pub summary: KnowledgeSummary,
    /// Built from an Anki collection, not from word lists alone.
    pub from_anki: bool,
    pub listed: ListedSummary,
}

impl Knowledge {
    /// Nothing known (for word lists without Anki).
    pub fn empty(fold: bool) -> Self {
        Knowledge {
            words: HashMap::new(),
            chars: HashMap::new(),
            fold,
            summary: KnowledgeSummary::default(),
            from_anki: false,
            listed: ListedSummary::default(),
        }
    }

    /// Mark `words` known as [`Status::Listed`], keeping a better (young or
    /// mature) Anki status. Their characters become known too.
    pub fn add_listed<'a>(&mut self, words: impl IntoIterator<Item = &'a str>, source: String) {
        let mut seen: HashSet<String> = HashSet::new();
        for w in words {
            let key = fold(w, self.fold);
            if key.is_empty() {
                continue;
            }
            let first = seen.insert(key.clone());
            for c in key.chars() {
                let e = self.chars.entry(c).or_default();
                if !e.is_known() {
                    self.listed.new_chars += 1;
                    *e = Status::Listed;
                }
            }
            if !first {
                continue;
            }
            let e = self.words.entry(key).or_default();
            if !e.is_known() {
                self.listed.new_words += 1;
                *e = Status::Listed;
            }
        }
        self.listed.words += seen.len() as u64;
        self.listed.source = source;
    }

    pub fn word(&self, w: &str) -> Status {
        if !self.fold {
            self.words.get(w).copied().unwrap_or_default()
        } else {
            let t = &script::TABLES.t2s;
            if w.chars().any(|c| t.contains_key(&c)) {
                self.words.get(&fold(w, true)).copied().unwrap_or_default()
            } else {
                self.words.get(w).copied().unwrap_or_default()
            }
        }
    }

    pub fn char(&self, c: char) -> Status {
        self.chars
            .get(&fold_char(c, self.fold))
            .copied()
            .unwrap_or_default()
    }
}

/// How many of a book's distinct items are known (young or mature).
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct KnownCount {
    pub known: u64,
    pub total: u64,
}

impl KnownCount {
    pub fn not_known(&self) -> u64 {
        self.total - self.known
    }
    pub fn percent(&self) -> f64 {
        self.known as f64 * 100.0 / self.total.max(1) as f64
    }
}

/// How hard the book should be to read with the known words, from the share
/// of running words (every occurrence) that are known. The thresholds follow
/// L2 reading research on lexical coverage: 98% for comfortable unassisted
/// reading of fiction (Hu & Nation 2000), 95% as the minimum for adequate
/// comprehension (Laufer 1989), and comprehension falling off steeply below
/// 90% and failing at 80%.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Difficulty {
    Easy,
    Moderate,
    Challenging,
    Hard,
    VeryHard,
}

impl Difficulty {
    /// Lower bounds (percent of running words known) of each band but the last.
    pub const BANDS: [(f64, Difficulty); 4] = [
        (98.0, Difficulty::Easy),
        (95.0, Difficulty::Moderate),
        (90.0, Difficulty::Challenging),
        (80.0, Difficulty::Hard),
    ];

    pub fn from_percent(p: f64) -> Self {
        Self::BANDS
            .iter()
            .find(|(min, _)| p >= *min)
            .map(|(_, d)| *d)
            .unwrap_or(Difficulty::VeryHard)
    }

    pub fn label(self) -> &'static str {
        match self {
            Difficulty::Easy => "Easy",
            Difficulty::Moderate => "Moderate",
            Difficulty::Challenging => "Challenging",
            Difficulty::Hard => "Hard",
            Difficulty::VeryHard => "Very hard",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Difficulty::Easy => "read without a dictionary",
            Difficulty::Moderate => "occasional look-ups",
            Difficulty::Challenging => "frequent look-ups",
            Difficulty::Hard => "look-ups in most sentences",
            Difficulty::VeryHard => "several unknowns per sentence",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct BookCoverage {
    pub anki: KnowledgeSummary,
    /// Anki was on (otherwise only word lists count).
    pub from_anki: bool,
    /// Words marked known from HSK bands and word lists.
    pub listed: ListedSummary,
    /// The book's distinct words that are known.
    pub unique_words: KnownCount,
    /// The book's distinct Chinese characters that are known.
    pub unique_chars: KnownCount,
    /// Word occurrences in the running text whose word is known, leaving out
    /// names (words that contain one of the `names` passed to `compute`).
    pub running_words: KnownCount,
    /// Word occurrences left out of `running_words` as names.
    pub name_tokens: u64,
    /// Distinct words left out of `unique_words` and `running_words` for
    /// occurring only once (0 unless `skip_hapax`).
    pub hapax_left_out: u64,
    /// Chinese character occurrences in the running text that are known.
    pub running_chars: KnownCount,
    /// From `running_words`.
    pub difficulty: Difficulty,
    /// Most frequent words that are not known.
    pub top_unknown_words: Vec<(String, u32)>,
    /// Parallel to `Report::word_freq` / `char_freq`.
    #[serde(skip)]
    pub word_status: Vec<Status>,
    #[serde(skip)]
    pub char_status: Vec<Status>,
}

impl BookCoverage {
    /// `names`: names and terms (the user dictionary and the recommended
    /// Suggested words). Readers don't need to know a name to follow a text,
    /// so any word containing one is left out of the running-word coverage
    /// that sets the difficulty (as coverage studies treat proper nouns).
    ///
    /// `skip_hapax`: words that occur only once are left out of the unique
    /// and running words (known or not), and of the top unknown words.
    pub fn compute(
        k: &Knowledge,
        char_freq: &[(char, u32)],
        word_freq: &[(String, u32)],
        names: &HashSet<String>,
        skip_hapax: bool,
    ) -> Self {
        let counted = |n: u32| !skip_hapax || n > 1;
        let word_status: Vec<Status> = word_freq.iter().map(|(w, _)| k.word(w)).collect();
        let char_status: Vec<Status> = char_freq.iter().map(|(c, _)| k.char(*c)).collect();
        let count = |v: &[Status]| KnownCount {
            known: v.iter().filter(|s| s.is_known()).count() as u64,
            total: v.len() as u64,
        };
        let unique_words =
            word_freq
                .iter()
                .zip(&word_status)
                .fold(KnownCount::default(), |mut c, ((_, n), s)| {
                    if counted(*n) {
                        c.total += 1;
                        c.known += u64::from(s.is_known());
                    }
                    c
                });
        let hapax_left_out = word_freq.len() as u64 - unique_words.total;
        let top_unknown_words = word_freq
            .iter()
            .zip(&word_status)
            .filter(|((_, n), s)| counted(*n) && !s.is_known())
            .take(100)
            .map(|(w, _)| w.clone())
            .collect();
        // Single characters would match far too much.
        let names: Vec<&str> = names
            .iter()
            .filter(|n| n.chars().count() >= 2)
            .map(String::as_str)
            .collect();
        let ac = (!names.is_empty())
            .then(|| aho_corasick::AhoCorasick::new(&names).ok())
            .flatten();
        let mut name_tokens = 0;
        let mut running_words = KnownCount::default();
        for ((w, n), s) in word_freq.iter().zip(&word_status) {
            if !counted(*n) {
                continue;
            }
            let n = *n as u64;
            if !s.is_known() && ac.as_ref().is_some_and(|a| a.is_match(w.as_str())) {
                name_tokens += n;
                continue;
            }
            running_words.total += n;
            if s.is_known() {
                running_words.known += n;
            }
        }
        BookCoverage {
            anki: k.summary.clone(),
            from_anki: k.from_anki,
            listed: k.listed.clone(),
            unique_words,
            unique_chars: count(&char_status),
            running_words,
            name_tokens,
            hapax_left_out,
            running_chars: char_freq.iter().zip(&char_status).fold(
                KnownCount::default(),
                |mut c, ((_, n), s)| {
                    c.total += *n as u64;
                    if s.is_known() {
                        c.known += *n as u64;
                    }
                    c
                },
            ),
            difficulty: Difficulty::from_percent(running_words.percent()),
            top_unknown_words,
            word_status,
            char_status,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_cleaning() {
        let mut n = chars::NormStats::default();
        assert_eq!(field_words("<b>跫</b>音", &mut n), ["跫音"]);
        assert_eq!(
            field_words("跫音[sound:a.ogg]<div>喀</div>", &mut n),
            ["跫音", "喀"]
        );
        assert_eq!(field_words("你好[nǐ hǎo]、再見", &mut n), ["你好", "再見"]);
        assert_eq!(
            field_words("{{c1::電腦::computer}}壞了", &mut n),
            ["電腦壞了"]
        );
        assert_eq!(
            field_words("<style>.x{content:'字'}</style>&#20320;好&nbsp;OK", &mut n),
            ["你好"]
        );
    }

    #[test]
    fn sentence_detection() {
        let mut n = chars::NormStats::default();
        let s = |f: &str, n: &mut chars::NormStats| looks_like_sentence(&clean_field(f, n));
        assert!(s("喀，喀，<b>跫音</b>響徹迴廊。", &mut n));
        assert!(s("我很好。", &mut n));
        assert!(!s("睁一只眼闭一只眼", &mut n));
        assert!(!s("跫音", &mut n));
        assert!(!s("<b>一路順風</b>", &mut n));
        assert!(!s("你好、再見", &mut n));
        assert!(!s("你好[nǐ hǎo]", &mut n));
    }

    #[test]
    fn difficulty_bands() {
        let d = Difficulty::from_percent;
        assert_eq!(d(100.0), Difficulty::Easy);
        assert_eq!(d(98.0), Difficulty::Easy);
        assert_eq!(d(97.9), Difficulty::Moderate);
        assert_eq!(d(95.0), Difficulty::Moderate);
        assert_eq!(d(90.0), Difficulty::Challenging);
        assert_eq!(d(85.0), Difficulty::Hard);
        assert_eq!(d(79.9), Difficulty::VeryHard);
        assert_eq!(d(0.0), Difficulty::VeryHard);
    }

    #[test]
    fn running_coverage_leaves_out_names() {
        let k = Knowledge {
            words: [
                ("我".to_string(), Status::Mature),
                ("看".to_string(), Status::Young),
            ]
            .into(),
            chars: [('我', Status::Mature), ('看', Status::Young)].into(),
            fold: false,
            summary: KnowledgeSummary::default(),
            from_anki: true,
            listed: ListedSummary::default(),
        };
        let word_freq = vec![
            ("我".to_string(), 50),
            ("看".to_string(), 30),
            ("顾飞".to_string(), 10),
            ("顾飞说".to_string(), 5),
            ("滑板".to_string(), 5),
        ];
        let char_freq = vec![('我', 50), ('看', 30), ('顾', 15), ('飞', 15)];
        let names: HashSet<String> = ["顾飞".to_string(), "飞".to_string()].into();
        let c = BookCoverage::compute(&k, &char_freq, &word_freq, &names, false);
        // 顾飞 and 顾飞说 are names; the one-character "name" 飞 is ignored.
        assert_eq!(c.name_tokens, 15);
        assert_eq!((c.running_words.known, c.running_words.total), (80, 85));
        // 80 / 85 = 94.1%
        assert_eq!(c.difficulty, Difficulty::Challenging);
        assert_eq!((c.running_chars.known, c.running_chars.total), (80, 110));
        assert_eq!((c.unique_words.known, c.unique_words.total), (2, 5));
        assert_eq!(c.hapax_left_out, 0);
    }

    #[test]
    fn running_coverage_skips_hapax() {
        let k = Knowledge {
            words: [
                ("我".to_string(), Status::Mature),
                ("一".to_string(), Status::Mature),
            ]
            .into(),
            chars: HashMap::new(),
            fold: false,
            summary: KnowledgeSummary::default(),
            from_anki: true,
            listed: ListedSummary::default(),
        };
        let word_freq = vec![
            ("我".to_string(), 90),
            ("看".to_string(), 8),
            ("一".to_string(), 1),
            ("滑板".to_string(), 1),
        ];
        let off = BookCoverage::compute(&k, &[], &word_freq, &HashSet::new(), false);
        assert_eq!(
            (off.running_words.known, off.running_words.total),
            (91, 100)
        );
        assert_eq!((off.unique_words.known, off.unique_words.total), (2, 4));
        // Known (一) and unknown (滑板) words that occur once both go.
        let on = BookCoverage::compute(&k, &[], &word_freq, &HashSet::new(), true);
        assert_eq!((on.running_words.known, on.running_words.total), (90, 98));
        assert_eq!((on.unique_words.known, on.unique_words.total), (1, 2));
        assert_eq!(on.hapax_left_out, 2);
        assert_eq!(on.top_unknown_words, [("看".to_string(), 8)]);
        // Statuses stay parallel to the word list.
        assert_eq!(on.word_status.len(), 4);
    }

    #[test]
    fn statuses() {
        let by_ivl = |t, ivl| card_status(t, is_mature(ivl, None, None).0);
        assert_eq!(by_ivl(2, 21), Status::Mature);
        assert_eq!(by_ivl(2, 20), Status::Young);
        assert_eq!(by_ivl(3, 30), Status::Learning);
        assert_eq!(by_ivl(1, 0), Status::Learning);
        assert_eq!(by_ivl(0, 0), Status::New);
        assert!(Status::Young.is_known() && !Status::Learning.is_known());
    }

    #[test]
    fn maturity_by_stability() {
        // Interval rule: stability is ignored.
        assert_eq!(is_mature(21, Some(5.0), None), (true, false));
        // Stability rule: a long interval at low desired retention isn't enough…
        assert_eq!(is_mature(40, Some(17.0), Some(21)), (false, false));
        // …and a short one at high retention can be.
        assert_eq!(is_mature(10, Some(21.0), Some(21)), (true, false));
        // No FSRS data: the interval decides, and the fallback is reported.
        assert_eq!(is_mature(30, None, Some(21)), (true, true));
        assert_eq!(is_mature(5, None, Some(21)), (false, true));
    }

    #[test]
    fn stability_settings_default_and_old_files() {
        // Files saved before the option existed load with the interval rule.
        let s: Settings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(!s.mature_by_stability);
        assert_eq!(s.mature_stability_days, 21);
        assert_eq!(s.stability_threshold(), None);
        let s = Settings {
            mature_by_stability: true,
            mature_stability_days: 0,
            ..Default::default()
        };
        assert_eq!(s.stability_threshold(), Some(1));
    }

    /// `json_extract` on Anki's `cards.data`: empty for non-FSRS cards.
    #[test]
    fn stability_sql() {
        let c = Connection::open_in_memory().unwrap();
        let q = |data: &str| -> Option<f64> {
            c.query_row(
                "SELECT CASE WHEN json_valid(?1) THEN json_extract(?1, '$.s') END",
                [data],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(q(r#"{"s":23.5,"d":5.1,"dr":0.9}"#), Some(23.5));
        assert_eq!(q(r#"{"pos":12}"#), None);
        assert_eq!(q(""), None);
        assert_eq!(q(r#"{"s":30}"#), Some(30.0));
    }

    #[test]
    fn folding_matches_across_scripts() {
        let k = Knowledge {
            words: [(fold("響徹", true), Status::Mature)].into_iter().collect(),
            chars: [(fold_char('響', true), Status::Mature)]
                .into_iter()
                .collect(),
            fold: true,
            summary: Default::default(),
            from_anki: true,
            listed: Default::default(),
        };
        assert_eq!(k.word("响彻"), Status::Mature);
        assert_eq!(k.word("響徹"), Status::Mature);
        assert_eq!(k.char('响'), Status::Mature);
    }

    fn copy_root(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("hanstat-copytest-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn space_check() {
        let dir = Path::new("/c");
        assert!(check_space(900 << 20, None, dir).is_ok());
        assert!(check_space(900 << 20, Some(1 << 30), dir).is_ok());
        let e = check_space(900 << 20, Some(300 << 20), dir).unwrap_err();
        assert!(
            e.contains("needs 900 MB") && e.contains("only 300 MB free"),
            "{e}"
        );
    }

    /// Writes `bytes` to `p` and moves its modified time on, so a change is
    /// seen even within the clock's resolution.
    fn write_at(p: &Path, bytes: &[u8], secs: u64) {
        std::fs::write(p, bytes).unwrap();
        let f = std::fs::OpenOptions::new().write(true).open(p).unwrap();
        f.set_modified(
            std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + secs),
        )
        .unwrap();
    }

    /// 3.5 blocks of bytes that differ from block to block.
    fn sample_db() -> Vec<u8> {
        (0..BLOCK * 7 / 2)
            .map(|i| (i % 251) as u8 ^ (i / BLOCK) as u8)
            .collect()
    }

    #[test]
    fn kept_copy_writes_only_what_changed() {
        let base = copy_root("kept");
        let src = base.join("collection.anki2");
        let src_wal = wal_of(&src);
        let mut bytes = sample_db();
        write_at(&src, &bytes, 1);
        write_at(&src_wal, b"wal one", 1);
        let root = base.join("copies");

        let (g, db, r) = make_copy(&src, &root).unwrap();
        assert_eq!(
            r,
            CopyReport {
                kept: true,
                unchanged: false,
                written: bytes.len() as u64
            }
        );
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
        assert_eq!(std::fs::read(wal_of(&db)).unwrap(), b"wal one");
        let dir = g.dir.clone();
        drop(g);
        assert!(dir.exists(), "a kept copy stays when dropped");

        // Nothing changed: nothing is read or written.
        let (g, _, r) = make_copy(&src, &root).unwrap();
        assert_eq!(
            r,
            CopyReport {
                kept: true,
                unchanged: true,
                written: 0
            }
        );
        drop(g);

        // One byte in the third block and the WAL: one block written.
        bytes[2 * BLOCK + 5] ^= 0xff;
        write_at(&src, &bytes, 2);
        write_at(&src_wal, b"wal two!", 2);
        let (g, db, r) = make_copy(&src, &root).unwrap();
        assert_eq!(
            r,
            CopyReport {
                kept: true,
                unchanged: false,
                written: BLOCK as u64
            }
        );
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
        assert_eq!(std::fs::read(wal_of(&db)).unwrap(), b"wal two!");
        drop(g);

        // Shorter (Anki's Check Database), and the WAL checkpointed away.
        bytes.truncate(BLOCK + 10);
        write_at(&src, &bytes, 3);
        std::fs::remove_file(&src_wal).unwrap();
        let (g, db, r) = make_copy(&src, &root).unwrap();
        assert_eq!(r.written, 0, "the first blocks were already the same");
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
        assert!(!wal_of(&db).exists());
        drop(g);

        // A copy cut short (no record) is compared again, not trusted.
        std::fs::remove_file(dir.join(SOURCE_FILE)).unwrap();
        std::fs::write(&db, b"half").unwrap();
        let (_g, db, r) = make_copy(&src, &root).unwrap();
        assert!(!r.unchanged);
        assert_eq!(std::fs::read(&db).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn kept_copy_in_use_gives_a_temporary_one() {
        let base = copy_root("busy");
        let src = base.join("collection.anki2");
        write_at(&src, b"db", 1);
        let root = base.join("copies");

        let (kept, _, r) = make_copy(&src, &root).unwrap();
        assert!(r.kept);
        // As on Reload while a job still holds the first copy.
        let (temp, db, r) = make_copy(&src, &root).unwrap();
        assert!(!r.kept);
        assert_eq!(std::fs::read(&db).unwrap(), b"db");
        assert!(
            kept.dir.exists(),
            "clearing stale copies left the one in use"
        );
        let temp_dir = temp.dir.clone();
        drop(temp);
        assert!(!temp_dir.exists());
        let kept_dir = kept.dir.clone();
        drop(kept);
        assert!(kept_dir.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stale_copies_cleared() {
        let base = copy_root("stale");
        let src = base.join("collection.anki2");
        write_at(&src, b"db", 1);
        let root = base.join("copies");

        // Left behind by a crash: a lock file nobody holds.
        let crashed = root.join("1-0");
        std::fs::create_dir_all(&crashed).unwrap();
        std::fs::write(crashed.join(LOCK_FILE), b"").unwrap();
        // Being set up by another process: no lock file yet, just made.
        let starting = root.join("2-0");
        std::fs::create_dir_all(&starting).unwrap();
        // Another collection's kept copy, not in use.
        let other = root.join(format!("{KEPT_PREFIX}0000000000000001"));
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(LOCK_FILE), b"").unwrap();

        let (g, _, _) = make_copy(&src, &root).unwrap();
        assert!(
            !crashed.exists(),
            "an unlocked copy is stale whatever its age"
        );
        assert!(starting.exists(), "a folder being set up is left alone");
        assert!(!other.exists(), "only the open collection's copy is kept");
        let kept = g.dir.clone();

        // In use: kept even when no collection is to be kept.
        remove_stale_copies(&root, None);
        assert!(kept.exists());
        drop(g);
        // Statistics off: idle kept copies go.
        remove_stale_copies(&root, None);
        assert!(!kept.exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn missing_collection_is_a_read_error() {
        let base = copy_root("missing");
        let e = make_copy(&base.join("collection.anki2"), &base.join("copies"))
            .err()
            .unwrap();
        assert!(e.starts_with("Cannot read Anki collection"), "{e}");
        let _ = std::fs::remove_dir_all(&base);
    }
}
