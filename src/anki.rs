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
use crate::script;
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
        std::fs::create_dir_all(dir).map_err(|e| format!("Cannot create {}: {e}", dir.display()))?;
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
            let mut st = conn.prepare("SELECT id, name FROM decks").map_err(sql_err)?;
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
            let mut st = conn.prepare("SELECT id, name FROM notetypes").map_err(sql_err)?;
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

    /// Classify every word and character in the selected fields.
    pub fn knowledge(&self, s: &Settings) -> Result<Knowledge, String> {
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
                    for w in field_words(f, &mut norm) {
                        any = true;
                        let key = fold(&w, s.match_across_scripts);
                        let e = words.entry(key).or_default();
                        *e = (*e).max(ns);
                    }
                }
                if !any {
                    summary.notes_without_chinese += 1;
                }
            }
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
    let mut st = conn.prepare("SELECT id, name FROM decks").map_err(sql_err)?;
    st.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?.replace('\x1f', "::")))
    })
    .map_err(sql_err)?
    .collect::<Result<_, _>>()
    .map_err(sql_err)
}

/// Copies left behind by a crashed run, older than a day.
fn remove_stale_copies(tmp: &Path) {
    let Ok(rd) = std::fs::read_dir(tmp) else { return };
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

/// The Chinese words in one field: HTML, cloze markup, sound tags and
/// bracketed readings are removed, the text is NFC-normalised as book text is,
/// and every maximal run of Han characters counts as one word.
pub fn field_words(field: &str, norm: &mut chars::NormStats) -> Vec<String> {
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
        n.and_then(char::from_u32).map(String::from).unwrap_or_default()
    });
    let s = chars::normalize(&s, norm);
    s.split(|c: char| !chars::is_han(c))
        .filter(|w| !w.is_empty())
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

#[derive(Clone, Debug, Serialize)]
pub struct BookCoverage {
    pub anki: KnowledgeSummary,
    /// Running words of the book by the status of the word.
    pub word_tokens: ByStatus,
    /// Distinct words of the book.
    pub word_types: ByStatus,
    /// Running Chinese characters by the status of the character.
    pub char_tokens: ByStatus,
    pub char_types: ByStatus,
    /// Most frequent words that are neither young nor mature.
    pub top_unknown_words: Vec<(String, u32)>,
    /// Parallel to `Report::word_freq` / `char_freq`.
    #[serde(skip)]
    pub word_status: Vec<Status>,
    #[serde(skip)]
    pub char_status: Vec<Status>,
}

impl BookCoverage {
    pub fn compute(k: &Knowledge, char_freq: &[(char, u32)], word_freq: &[(String, u32)]) -> Self {
        let word_status: Vec<Status> = word_freq.iter().map(|(w, _)| k.word(w)).collect();
        let char_status: Vec<Status> = char_freq.iter().map(|(c, _)| k.char(*c)).collect();
        let (mut wt, mut wy, mut ct, mut cy) = (
            ByStatus::default(),
            ByStatus::default(),
            ByStatus::default(),
            ByStatus::default(),
        );
        for ((_, n), &s) in word_freq.iter().zip(&word_status) {
            wt.add(s, *n as u64);
            wy.add(s, 1);
        }
        for ((_, n), &s) in char_freq.iter().zip(&char_status) {
            ct.add(s, *n as u64);
            cy.add(s, 1);
        }
        let top_unknown_words = word_freq
            .iter()
            .zip(&word_status)
            .filter(|(_, s)| !s.is_known())
            .take(100)
            .map(|(w, _)| w.clone())
            .collect();
        BookCoverage {
            anki: k.summary.clone(),
            word_tokens: wt,
            word_types: wy,
            char_tokens: ct,
            char_types: cy,
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
        assert_eq!(field_words("{{c1::電腦::computer}}壞了", &mut n), ["電腦壞了"]);
        assert_eq!(
            field_words("<style>.x{content:'字'}</style>&#20320;好&nbsp;OK", &mut n),
            ["你好"]
        );
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
            chars: [(fold_char('響', true), Status::Mature)].into_iter().collect(),
            fold: true,
            summary: Default::default(),
        };
        assert_eq!(k.word("响彻"), Status::Mature);
        assert_eq!(k.word("響徹"), Status::Mature);
        assert_eq!(k.char('响'), Status::Mature);
    }
}
