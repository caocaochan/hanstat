//! Known-vocabulary statistics from the user's local Anki collection.
//!
//! The collection is never opened in place: Anki keeps it open with an
//! exclusive lock while running, so `collection.anki2` (and its `-wal`, which
//! holds changes not yet checkpointed) are copied to a private temp folder and
//! read from there. Nothing is ever written back.
//!
//! Classification follows Anki's own Card Counts definitions: a review card is
//! *mature* when its interval is ≥ 21 days and *young* otherwise; learning and
//! relearning (lapsed) cards are *learning*; unstudied cards are *new*. A note
//! takes the best status of its cards in the selected decks, a word the best
//! status of the notes it appears in, and a character the best status of the
//! words containing it.
//!
//! The user's choice of collection, decks, note types and fields is saved to
//! `%LOCALAPPDATA%\hanstat\anki.json`, next to the `models` folder.

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
    Young,
    Mature,
}

impl Status {
    pub const ALL: [Status; 5] = [
        Status::Mature,
        Status::Young,
        Status::Learning,
        Status::New,
        Status::NotInAnki,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Status::NotInAnki => "Not in Anki",
            Status::New => "New",
            Status::Learning => "Learning",
            Status::Young => "Young",
            Status::Mature => "Mature",
        }
    }

    /// Young or mature: what Anki considers successfully reviewed.
    pub fn is_known(self) -> bool {
        self >= Status::Young
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
}

impl ByStatus {
    pub fn add(&mut self, s: Status, n: u64) {
        *match s {
            Status::NotInAnki => &mut self.not_in_anki,
            Status::New => &mut self.new,
            Status::Learning => &mut self.learning,
            Status::Young => &mut self.young,
            Status::Mature => &mut self.mature,
        } += n;
    }

    pub fn get(&self, s: Status) -> u64 {
        match s {
            Status::NotInAnki => self.not_in_anki,
            Status::New => self.new,
            Status::Learning => self.learning,
            Status::Young => self.young,
            Status::Mature => self.mature,
        }
    }

    pub fn total(&self) -> u64 {
        self.mature + self.young + self.learning + self.new + self.not_in_anki
    }

    /// Mature + young.
    pub fn known(&self) -> u64 {
        self.mature + self.young
    }
}

// ---------------------------------------------------------------------------
// Profiles and saved settings
// ---------------------------------------------------------------------------

pub struct Profile {
    pub name: String,
    pub collection: PathBuf,
}

/// Anki's data folder: `ANKI_BASE` if set (as Anki itself honours), else `%APPDATA%\Anki2`.
pub fn anki_base() -> Option<PathBuf> {
    if let Some(b) = std::env::var_os("ANKI_BASE") {
        return Some(PathBuf::from(b));
    }
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Anki2"))
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
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        crate::models::app_dir().join("anki.json")
    }

    /// Missing file → defaults. A corrupt file is reported, and defaults are used.
    pub fn load() -> (Settings, Option<String>) {
        let p = Self::path();
        match std::fs::read(&p) {
            Err(_) => (Settings::default(), None),
            Ok(b) => match serde_json::from_slice(&b) {
                Ok(s) => (s, None),
                Err(e) => (
                    Settings::default(),
                    Some(format!("Ignoring unreadable {}: {e}", p.display())),
                ),
            },
        }
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path();
        let dir = p.parent().unwrap();
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let tmp = p.with_extension(format!("json.{}.tmp", std::process::id()));
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| format!("Cannot write {}: {e}", tmp.display()))?;
        std::fs::rename(&tmp, &p).map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            format!("Cannot write {}: {e}", p.display())
        })
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

/// Deletes the private copy when the collection is dropped.
struct TempCopy(PathBuf);

impl Drop for TempCopy {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An open, private, read-only-by-convention copy of a collection.
pub struct Collection {
    pub info: std::sync::Arc<CollectionInfo>,
    conn: Connection,
    // Declared after `conn` so the database is closed before the folder is removed.
    _tmp: TempCopy,
}

static COPY_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn sql_err(e: rusqlite::Error) -> String {
    format!("Reading the Anki collection failed: {e}")
}

impl Collection {
    pub fn open(path: &Path) -> Result<Collection, String> {
        let tmp_root = std::env::temp_dir();
        remove_stale_copies(&tmp_root);
        let dir = tmp_root.join(format!(
            "hanstat-anki-{}-{}",
            std::process::id(),
            COPY_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
        let guard = TempCopy(dir.clone());
        let db = dir.join("collection.anki2");
        std::fs::copy(path, &db)
            .map_err(|e| format!("Cannot read Anki collection {}: {e}", path.display()))?;
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        if std::fs::metadata(&wal).is_ok_and(|m| m.len() > 0) {
            std::fs::copy(&wal, dir.join("collection.anki2-wal"))
                .map_err(|e| format!("Cannot read {}: {e}", wal.display()))?;
        }
        // Read-write only so SQLite can replay the copied WAL into its own index.
        let conn = Connection::open_with_flags(
            &db,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
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

        let mut summary = KnowledgeSummary::default();
        let mut note_status: HashMap<i64, Status> = HashMap::new();
        if !deck_ids.is_empty() && !ords.is_empty() {
            let mut st = self
                .conn
                .prepare(
                    "SELECT c.nid, n.mid, CASE WHEN c.odid != 0 THEN c.odid ELSE c.did END, c.type, c.queue, c.ivl \
                     FROM cards c JOIN notes n ON n.id = c.nid",
                )
                .map_err(sql_err)?;
            let rows = st
                .query_map([], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)?,
                        r.get::<_, i64>(5)?,
                    ))
                })
                .map_err(sql_err)?;
            for r in rows {
                let (nid, mid, did, ctype, queue, ivl) = r.map_err(sql_err)?;
                if !deck_ids.contains(&did) || !ords.contains_key(&mid) {
                    continue;
                }
                if queue == -1 && !s.include_suspended {
                    summary.suspended_excluded += 1;
                    continue;
                }
                let cs = card_status(ctype, ivl);
                summary.cards.add(cs, 1);
                let e = note_status.entry(nid).or_insert(Status::New);
                *e = (*e).max(cs);
            }
        }

        let mut words: HashMap<String, Status> = HashMap::new();
        let mut add = |w: &str, st: Status| {
            let e = words.entry(fold(w, s.match_across_scripts)).or_default();
            *e = (*e).max(st);
        };
        // Sentences and long phrases, segmented together afterwards.
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
            for (toks, st) in segmenter.cut_many(&chunks).into_iter().zip(chunk_status) {
                toks.into_iter().for_each(|w| add(w, st));
            }
            summary.segmenter = Some(segmenter.kind);
        }
        let mut char_status: HashMap<char, Status> = HashMap::new();
        for (w, &st) in &words {
            summary.words.add(st, 1);
            for c in w.chars() {
                let e = char_status.entry(c).or_default();
                *e = (*e).max(st);
            }
        }
        for &st in char_status.values() {
            summary.chars.add(st, 1);
        }
        Ok(Knowledge {
            words,
            chars: char_status,
            fold: s.match_across_scripts,
            summary,
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
fn remove_stale_copies(tmp: &Path) {
    let Ok(rd) = std::fs::read_dir(tmp) else {
        return;
    };
    let day = std::time::Duration::from_secs(86_400);
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with("hanstat-anki-")
            && e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > day))
        {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// Anki card `type`: 0 new, 1 learning, 2 review, 3 relearning.
fn card_status(ctype: i64, ivl: i64) -> Status {
    match ctype {
        2 if ivl >= MATURE_IVL => Status::Mature,
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

/// One field's text with HTML, cloze markup, sound tags and bracketed readings
/// removed, NFC-normalised as book text is.
pub fn clean_field(field: &str, norm: &mut chars::NormStats) -> String {
    let s = RE_CLOZE.replace_all(field, "$1");
    let s = RE_STYLE.replace_all(&s, " ");
    let s = RE_BLOCK.replace_all(&s, " ");
    // Inline tags (<b>, <span>) must not split a word.
    let s = RE_TAG.replace_all(&s, "");
    let s = RE_BRACKET.replace_all(&s, " ");
    let s = RE_NUM_ENTITY.replace_all(&s, |c: &regex::Captures| {
        let n = match (c.get(1), c.get(2)) {
            (Some(h), _) => u32::from_str_radix(h.as_str(), 16).ok(),
            (_, Some(d)) => d.as_str().parse().ok(),
            _ => None,
        };
        n.and_then(char::from_u32)
            .map(String::from)
            .unwrap_or_default()
    });
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
pub fn field_words(field: &str, norm: &mut chars::NormStats) -> Vec<String> {
    han_runs(&clean_field(field, norm))
        .map(str::to_owned)
        .collect()
}

fn fold(s: &str, on: bool) -> String {
    if !on {
        return s.to_owned();
    }
    let t = &script::TABLES.t2s;
    s.chars().map(|c| t.get(&c).copied().unwrap_or(c)).collect()
}

fn fold_char(c: char, on: bool) -> char {
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
    /// Distinct words (Han runs) by status.
    pub words: ByStatus,
    /// Distinct characters by status.
    pub chars: ByStatus,
}

pub struct Knowledge {
    /// Keyed by the (optionally T→S folded) word.
    pub words: HashMap<String, Status>,
    pub chars: HashMap<char, Status>,
    pub fold: bool,
    pub summary: KnowledgeSummary,
}

impl Knowledge {
    pub fn word(&self, w: &str) -> Status {
        self.words
            .get(&fold(w, self.fold))
            .copied()
            .unwrap_or_default()
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
    /// The book's distinct words that are known.
    pub unique_words: KnownCount,
    /// The book's distinct Chinese characters that are known.
    pub unique_chars: KnownCount,
    /// Word occurrences in the running text whose word is known, leaving out
    /// names (words that contain one of the `names` passed to `compute`).
    pub running_words: KnownCount,
    /// Word occurrences left out of `running_words` as names.
    pub name_tokens: u64,
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
    pub fn compute(
        k: &Knowledge,
        char_freq: &[(char, u32)],
        word_freq: &[(String, u32)],
        names: &HashSet<String>,
    ) -> Self {
        let word_status: Vec<Status> = word_freq.iter().map(|(w, _)| k.word(w)).collect();
        let char_status: Vec<Status> = char_freq.iter().map(|(c, _)| k.char(*c)).collect();
        let count = |v: &[Status]| KnownCount {
            known: v.iter().filter(|s| s.is_known()).count() as u64,
            total: v.len() as u64,
        };
        let top_unknown_words = word_freq
            .iter()
            .zip(&word_status)
            .filter(|(_, s)| !s.is_known())
            .take(100)
            .map(|(w, _)| w.clone())
            .collect();
        // Single characters would match far too much.
        let names: Vec<&str> = names
            .iter()
            .filter(|n| n.chars().count() >= 2)
            .map(String::as_str)
            .collect();
        let mut name_tokens = 0;
        let mut running_words = KnownCount::default();
        for ((w, n), s) in word_freq.iter().zip(&word_status) {
            let n = *n as u64;
            if !s.is_known() && names.iter().any(|name| w.contains(name)) {
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
            unique_words: count(&word_status),
            unique_chars: count(&char_status),
            running_words,
            name_tokens,
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
        let c = BookCoverage::compute(&k, &char_freq, &word_freq, &names);
        // 顾飞 and 顾飞说 are names; the one-character "name" 飞 is ignored.
        assert_eq!(c.name_tokens, 15);
        assert_eq!((c.running_words.known, c.running_words.total), (80, 85));
        // 80 / 85 = 94.1%
        assert_eq!(c.difficulty, Difficulty::Challenging);
        assert_eq!((c.running_chars.known, c.running_chars.total), (80, 110));
        assert_eq!((c.unique_words.known, c.unique_words.total), (2, 5));
    }

    #[test]
    fn statuses() {
        assert_eq!(card_status(2, 21), Status::Mature);
        assert_eq!(card_status(2, 20), Status::Young);
        assert_eq!(card_status(3, 30), Status::Learning);
        assert_eq!(card_status(1, 0), Status::Learning);
        assert_eq!(card_status(0, 0), Status::New);
        assert!(Status::Young.is_known() && !Status::Learning.is_known());
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
        };
        assert_eq!(k.word("响彻"), Status::Mature);
        assert_eq!(k.word("響徹"), Status::Mature);
        assert_eq!(k.char('响'), Status::Mature);
    }
}
