//! Reading stats: time and Chinese characters read in the reader, per book
//! and per local day, kept in `reading-stats.sqlite` in the app's folder.
//!
//! The reader's tracker (`reader/tracker.rs`) writes them; the reader's
//! stats panel, the Library's book view and Library Stats read them. One
//! copy lives in memory ([`with`], [`edit`]); [`revision`] changes with
//! every change, so views build their summaries again only then. [`save`]
//! writes the books that changed, in one transaction, off the UI thread.
//!
//! Days are local days that start at [`TrackerSettings::day_start_hour`]:
//! the local offset comes from the OS, through functions declared here, so
//! no dependency (and no feature of one) is added.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{LazyLock, Mutex, MutexGuard};

const DB_FILE: &str = "reading-stats.sqlite";
/// What builds before the database saved; read once if there is no database.
const OLD_JSON: &str = "reading-stats.json";
/// Days whose reading counts toward the measured speed.
pub const SPEED_DAYS: i32 = 30;
/// Reading time a speed needs before it is shown or used.
pub const MIN_SPEED_SECS: f64 = 600.0;
/// Reading a day needs to count as a day read.
pub const DAY_READ_SECS: f64 = 60.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrackerSettings {
    /// Start timing when a book opens; otherwise wait for P.
    pub auto_start: bool,
    /// Pause after this many minutes with no input.
    pub idle_mins: u32,
    /// The local hour a new day begins.
    pub day_start_hour: u32,
    /// Show the stats line in the reader's bottom margin.
    pub live_line: bool,
}

impl Default for TrackerSettings {
    fn default() -> Self {
        TrackerSettings {
            auto_start: true,
            idle_mins: 5,
            day_start_hour: 4,
            live_line: true,
        }
    }
}

/// Reading and listening (read aloud playing) in one day, or one session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Day {
    pub read_secs: f64,
    pub read_chars: u64,
    pub listen_secs: f64,
    pub listen_chars: u64,
}

impl Day {
    pub fn add(&mut self, o: &Day) {
        self.read_secs += o.read_secs;
        self.read_chars += o.read_chars;
        self.listen_secs += o.listen_secs;
        self.listen_chars += o.listen_chars;
    }

    pub fn is_empty(&self) -> bool {
        self.read_secs < 1.0
            && self.read_chars == 0
            && self.listen_secs < 1.0
            && self.listen_chars == 0
    }
}

/// The reader open on a book, from opening to closing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    /// Unix times.
    pub start: u64,
    pub end: u64,
    #[serde(flatten)]
    pub totals: Day,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BookStats {
    pub title: String,
    /// The book's Chinese characters, as the reader counts them.
    pub han_total: u64,
    /// Unix times.
    pub first_read: Option<u64>,
    pub finished: Option<u64>,
    /// Characters paged back over beyond what today had: paged forward
    /// again, they pay this off before they count.
    pub debt: u64,
    /// By local day number ([`local_day`]).
    pub days: BTreeMap<i32, Day>,
    pub sessions: Vec<Session>,
}

impl BookStats {
    pub fn totals(&self) -> Day {
        let mut t = Day::default();
        for d in self.days.values() {
            t.add(d);
        }
        t
    }

    /// Characters per minute of reading, once there is enough of it.
    pub fn speed(&self) -> Option<f64> {
        let t = self.totals();
        speed_of(t.read_chars, t.read_secs)
    }
}

/// A daily goal, in force from `from_day` until the next one. 0: no goal
/// of that kind.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Goal {
    pub from_day: i32,
    pub minutes: u32,
    pub chars: u64,
}

impl Goal {
    pub fn is_set(&self) -> bool {
        self.minutes > 0 || self.chars > 0
    }

    pub fn met(&self, d: &Day) -> bool {
        self.is_set() && d.read_secs >= self.minutes as f64 * 60.0 && d.read_chars >= self.chars
    }

    /// How far `d` is toward the goal, 0..=1 (the lesser part when both are set).
    pub fn progress(&self, d: &Day) -> f32 {
        let mut p: f32 = 1.0;
        if self.minutes > 0 {
            p = p.min((d.read_secs / (self.minutes as f64 * 60.0)) as f32);
        }
        if self.chars > 0 {
            p = p.min(d.read_chars as f32 / self.chars as f32);
        }
        p.clamp(0.0, 1.0)
    }
}

/// Where the reader left a book: a chapter (spine index) and the text
/// offset in it, and when (Unix time).
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LeftAt {
    pub spine: u32,
    pub offset: u64,
    pub saved: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReadingStats {
    pub version: u32,
    pub settings: TrackerSettings,
    /// Dated goal changes, oldest first.
    pub goals: Vec<Goal>,
    /// By the reader's book id (the Library id, or the file's path).
    pub books: BTreeMap<String, BookStats>,
    /// Where each book was left, by the same id; kept when a book's stats
    /// are removed.
    pub places: BTreeMap<String, LeftAt>,
}

impl ReadingStats {
    /// The goal in force on `day`.
    pub fn goal_on(&self, day: i32) -> Option<Goal> {
        goal_in(&self.goals, day)
    }

    /// Change the goal from `today` on; days before keep theirs.
    pub fn set_goal(&mut self, today: i32, minutes: u32, chars: u64) {
        self.goals.retain(|g| g.from_day < today);
        let g = Goal {
            from_day: today,
            minutes,
            chars,
        };
        if self.goals.last().map(|l| (l.minutes, l.chars)) != Some((minutes, chars))
            || self.goals.is_empty()
        {
            self.goals.push(g);
        }
    }

    /// Every book's days added up.
    pub fn days(&self) -> BTreeMap<i32, Day> {
        let mut out: BTreeMap<i32, Day> = BTreeMap::new();
        for b in self.books.values() {
            for (d, v) in &b.days {
                out.entry(*d).or_default().add(v);
            }
        }
        out
    }

    /// Characters per minute over the last [`SPEED_DAYS`] days up to `today`
    /// (listening left out), once there is enough reading.
    pub fn measured_speed(&self, today: i32) -> Option<f64> {
        let (mut chars, mut secs) = (0u64, 0.0);
        for b in self.books.values() {
            for (_, d) in b.days.range(today - SPEED_DAYS + 1..=today) {
                chars += d.read_chars;
                secs += d.read_secs;
            }
        }
        speed_of(chars, secs)
    }
}

/// The goal of `goals` (dated changes, oldest first) in force on `day`.
pub fn goal_in(goals: &[Goal], day: i32) -> Option<Goal> {
    goals
        .iter()
        .rev()
        .find(|g| g.from_day <= day)
        .copied()
        .filter(Goal::is_set)
}

/// Every day of `days` in `a..b` added up.
pub fn sum_range(days: &BTreeMap<i32, Day>, a: i32, b: i32) -> Day {
    let mut t = Day::default();
    if a < b {
        for (_, d) in days.range(a..b) {
            t.add(d);
        }
    }
    t
}

pub fn speed_of(chars: u64, secs: f64) -> Option<f64> {
    (secs >= MIN_SPEED_SECS && chars > 0).then(|| chars as f64 / (secs / 60.0))
}

/// Current and longest runs of days that pass `ok`, ending today. Today
/// doesn't break the current run until it is over.
pub fn streaks(
    days: &BTreeMap<i32, Day>,
    today: i32,
    ok: impl Fn(i32, &Day) -> bool,
) -> (u32, u32) {
    let pass = |d: i32| days.get(&d).is_some_and(|v| ok(d, v));
    let mut longest = 0u32;
    let mut run = 0u32;
    let mut prev: Option<i32> = None;
    for (&d, v) in days.range(..=today) {
        if !ok(d, v) {
            run = 0;
            prev = None;
            continue;
        }
        run = if prev == Some(d - 1) { run + 1 } else { 1 };
        prev = Some(d);
        longest = longest.max(run);
    }
    let mut current = 0u32;
    let mut d = if pass(today) { today } else { today - 1 };
    while pass(d) {
        current += 1;
        d -= 1;
    }
    (current, longest)
}

/// What Library Stats and the stats panel show, made once per [`revision`].
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub today: i32,
    pub days: BTreeMap<i32, Day>,
    pub totals: Day,
    pub books_finished: usize,
    pub days_read: usize,
    pub read_streak: (u32, u32),
    pub goal_streak: (u32, u32),
    pub goal: Option<Goal>,
    /// Every goal change, for [`goal_in`].
    pub goals: Vec<Goal>,
    pub goal_days: std::collections::HashSet<i32>,
    pub speed: Option<f64>,
    /// Per book: id, stats without the session lists.
    pub books: Vec<(String, BookRow)>,
}

#[derive(Clone, Debug, Default)]
pub struct BookRow {
    pub title: String,
    pub totals: Day,
    pub han_total: u64,
    pub first_read: Option<u64>,
    pub finished: Option<u64>,
    pub last_day: Option<i32>,
    /// By local day, for totals over a stretch of days.
    pub days: BTreeMap<i32, Day>,
}

impl ReadingStats {
    pub fn summary(&self, today: i32) -> Summary {
        let days = self.days();
        let mut totals = Day::default();
        for d in days.values() {
            totals.add(d);
        }
        let read = |_: i32, d: &Day| d.read_secs >= DAY_READ_SECS;
        let goal_ok = |day: i32, d: &Day| self.goal_on(day).is_some_and(|g| g.met(d));
        let goal_days = days
            .iter()
            .filter(|(k, v)| goal_ok(**k, v))
            .map(|(k, _)| *k)
            .collect();
        let mut books: Vec<(String, BookRow)> = self
            .books
            .iter()
            .filter(|(_, b)| !b.totals().is_empty() || b.finished.is_some())
            .map(|(id, b)| {
                (
                    id.clone(),
                    BookRow {
                        title: b.title.clone(),
                        totals: b.totals(),
                        han_total: b.han_total,
                        first_read: b.first_read,
                        finished: b.finished,
                        last_day: b.days.keys().next_back().copied(),
                        days: b.days.clone(),
                    },
                )
            })
            .collect();
        books.sort_by_key(|b| std::cmp::Reverse(b.1.last_day));
        Summary {
            today,
            totals,
            books_finished: self.books.values().filter(|b| b.finished.is_some()).count(),
            days_read: days
                .values()
                .filter(|d| d.read_secs >= DAY_READ_SECS)
                .count(),
            read_streak: streaks(&days, today, read),
            goal_streak: streaks(&days, today, goal_ok),
            goal: self.goal_on(today),
            goals: self.goals.clone(),
            goal_days,
            speed: self.measured_speed(today),
            days,
            books,
        }
    }
}

// The copy in memory, kept in SQLite (`reading-stats.sqlite`): read once,
// then each changed book written again in one transaction, off the UI
// thread. A database that can't be read is never written to.

struct Store {
    stats: ReadingStats,
    loaded: bool,
    revision: u64,
    /// Changed since the last save: books (rewritten whole), books
    /// removed, and the settings and goals.
    dirty: BTreeSet<String>,
    removed: BTreeSet<String>,
    meta_dirty: bool,
    places_dirty: BTreeSet<String>,
    /// Why the database couldn't be read; nothing is saved then. `bool`:
    /// said once.
    broken: Option<(String, bool)>,
    /// [`measured_now`] for (revision, day).
    speed: Option<(u64, i32, Option<f64>)>,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| {
    Mutex::new(Store {
        stats: ReadingStats::default(),
        loaded: false,
        revision: 1,
        dirty: BTreeSet::new(),
        removed: BTreeSet::new(),
        meta_dirty: false,
        places_dirty: BTreeSet::new(),
        broken: None,
        speed: None,
    })
});

fn path() -> std::path::PathBuf {
    hanstat::models::app_dir().join(DB_FILE)
}

fn store() -> MutexGuard<'static, Store> {
    let mut s = STORE.lock().unwrap_or_else(|e| e.into_inner());
    // Tests never read (or write) the user's stats.
    if !s.loaded && !cfg!(test) {
        s.loaded = true;
        let p = path();
        if p.exists() {
            match db::open(&p).and_then(|c| db::read(&c)) {
                Ok(v) => s.stats = v,
                Err(e) => {
                    s.broken = Some((
                        format!(
                            "The reading stats in {} can't be read ({e}). They are left as they are, \
                             and nothing new is saved until hanstat can read them.",
                            p.display()
                        ),
                        false,
                    ))
                }
            }
        } else if let Ok(b) = std::fs::read(hanstat::models::app_dir().join(OLD_JSON))
            && let Ok(old) = serde_json::from_slice::<ReadingStats>(&b)
        {
            // Saved by a build before the database: written to it at the
            // next save (the JSON file is left as it is).
            s.dirty = old.books.keys().cloned().collect();
            s.meta_dirty = true;
            s.stats = old;
        }
    }
    s
}

pub fn with<R>(f: impl FnOnce(&ReadingStats) -> R) -> R {
    f(&store().stats)
}

/// Change the stats in place without marking anything changed: the
/// reader's tracker, once a frame, says with [`book_changed`] when it did.
pub fn edit<R>(f: impl FnOnce(&mut ReadingStats) -> R) -> R {
    f(&mut store().stats)
}

/// Book `id`'s stats changed; [`save`] writes them.
pub fn book_changed(id: &str) {
    let mut s = store();
    s.revision += 1;
    s.removed.remove(id);
    s.dirty.insert(id.to_owned());
}

/// Change the tracker settings or the goals; [`save`] writes them.
pub fn update_meta<R>(f: impl FnOnce(&mut ReadingStats) -> R) -> R {
    let mut s = store();
    let r = f(&mut s.stats);
    s.revision += 1;
    s.meta_dirty = true;
    r
}

/// Remove book `id`'s stats; [`save`] deletes them.
pub fn remove_book(id: &str) {
    let mut s = store();
    s.stats.books.remove(id);
    s.revision += 1;
    s.dirty.remove(id);
    s.removed.insert(id.to_owned());
}

/// Where the reader left book `id`, if it is saved here.
pub fn left_at(id: &str) -> Option<LeftAt> {
    store().stats.places.get(id).copied()
}

/// The reader left book `id` at `spine`, `offset`; [`save`] writes it.
pub fn set_left_at(id: &str, spine: usize, offset: usize) {
    let mut s = store();
    let at = LeftAt {
        spine: spine as u32,
        offset: offset as u64,
        saved: now() as u64,
    };
    if s.stats
        .places
        .get(id)
        .is_none_or(|p| (p.spine, p.offset) != (at.spine, at.offset))
    {
        s.stats.places.insert(id.to_owned(), at);
        s.places_dirty.insert(id.to_owned());
    }
}

/// The Chinese reading speed measured in the reader (characters a minute
/// over the last [`SPEED_DAYS`] days), once there is enough reading; kept
/// until the stats or the day change, so it is cheap to ask every frame.
pub fn measured_now() -> Option<f64> {
    let now = now();
    let mut s = store();
    let today = local_day(now, s.stats.settings.day_start_hour);
    if let Some((rev, day, v)) = s.speed
        && rev == s.revision
        && day == today
    {
        return v;
    }
    let v = s.stats.measured_speed(today);
    s.speed = Some((s.revision, today, v));
    v
}

/// Changes with every change to the stats.
pub fn revision() -> u64 {
    store().revision
}

/// Queue what changed for the writer thread. An error: the last write
/// failed, or the database couldn't be read (said once).
pub fn save() -> Result<(), String> {
    let mut s = store();
    if let Some((msg, told)) = &mut s.broken {
        let first = !*told;
        *told = true;
        return if first { Err(msg.clone()) } else { Ok(()) };
    }
    if cfg!(test)
        || (s.dirty.is_empty()
            && s.removed.is_empty()
            && !s.meta_dirty
            && s.places_dirty.is_empty())
    {
        return writer::take_error().map_or(Ok(()), Err);
    }
    let dirty = std::mem::take(&mut s.dirty);
    let batch = db::Batch {
        books: dirty
            .into_iter()
            .filter_map(|id| s.stats.books.get(&id).cloned().map(|b| (id, b)))
            .collect(),
        removed: std::mem::take(&mut s.removed).into_iter().collect(),
        meta: std::mem::take(&mut s.meta_dirty)
            .then(|| (s.stats.settings.clone(), s.stats.goals.clone())),
        places: std::mem::take(&mut s.places_dirty)
            .into_iter()
            .filter_map(|id| s.stats.places.get(&id).map(|p| (id, *p)))
            .collect(),
    };
    drop(s);
    writer::queue(path(), batch);
    writer::take_error().map_or(Ok(()), Err)
}

/// Why the saved stats couldn't be read, if they couldn't.
pub fn problem() -> Option<String> {
    store().broken.as_ref().map(|b| b.0.clone())
}

/// Wait until everything queued is written (on exit).
pub fn flush() {
    writer::flush();
}

/// The database: one row per book, per book and day, per session and per
/// goal change, and the settings as JSON (new fields need no migration).
mod db {
    use super::{BookStats, Day, Goal, LeftAt, ReadingStats, Session, TrackerSettings};
    use rusqlite::{Connection, params};
    use std::path::Path;

    /// `PRAGMA user_version`: a database from a newer build isn't touched.
    const VERSION: i32 = 1;

    pub struct Batch {
        pub books: Vec<(String, BookStats)>,
        pub removed: Vec<String>,
        pub meta: Option<(TrackerSettings, Vec<Goal>)>,
        pub places: Vec<(String, LeftAt)>,
    }

    pub fn open(path: &Path) -> Result<Connection, String> {
        let c = Connection::open(path).map_err(|e| e.to_string())?;
        c.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        // WAL: a crash or power cut loses at most the transaction being
        // written, never the file; FULL syncs every commit.
        c.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| e.to_string())?;
        c.pragma_update(None, "synchronous", "FULL")
            .map_err(|e| e.to_string())?;
        let v: i32 = c
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if v > VERSION {
            return Err(format!(
                "they were saved by a newer hanstat (format {v}, this one reads {VERSION})"
            ));
        }
        c.execute_batch(
            "BEGIN;
             CREATE TABLE IF NOT EXISTS books (
                 id TEXT PRIMARY KEY, title TEXT NOT NULL, han_total INTEGER NOT NULL,
                 first_read INTEGER, finished INTEGER, debt INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS days (
                 book TEXT NOT NULL, day INTEGER NOT NULL,
                 read_secs REAL NOT NULL, read_chars INTEGER NOT NULL,
                 listen_secs REAL NOT NULL, listen_chars INTEGER NOT NULL,
                 PRIMARY KEY (book, day)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS sessions (
                 book TEXT NOT NULL, seq INTEGER NOT NULL, start INTEGER NOT NULL, end INTEGER NOT NULL,
                 read_secs REAL NOT NULL, read_chars INTEGER NOT NULL,
                 listen_secs REAL NOT NULL, listen_chars INTEGER NOT NULL,
                 PRIMARY KEY (book, seq)) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS goals (
                 from_day INTEGER PRIMARY KEY, minutes INTEGER NOT NULL, chars INTEGER NOT NULL);
             CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS places (
                 book TEXT PRIMARY KEY, spine INTEGER NOT NULL, offset INTEGER NOT NULL,
                 saved INTEGER NOT NULL);
             COMMIT;",
        )
        .map_err(|e| e.to_string())?;
        if v < VERSION {
            c.pragma_update(None, "user_version", VERSION)
                .map_err(|e| e.to_string())?;
        }
        Ok(c)
    }

    pub fn read(c: &Connection) -> Result<ReadingStats, String> {
        let e = |e: rusqlite::Error| e.to_string();
        let mut s = ReadingStats::default();
        if let Some(json) = c
            .query_row("SELECT value FROM meta WHERE key = 'settings'", [], |r| {
                r.get::<_, String>(0)
            })
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                e => Err(e),
            })
            .map_err(e)?
        {
            s.settings = serde_json::from_str(&json).map_err(|e| e.to_string())?;
        }
        let mut q = c
            .prepare("SELECT from_day, minutes, chars FROM goals ORDER BY from_day")
            .map_err(e)?;
        s.goals = q
            .query_map([], |r| {
                Ok(Goal {
                    from_day: r.get(0)?,
                    minutes: r.get(1)?,
                    chars: r.get::<_, i64>(2)? as u64,
                })
            })
            .map_err(e)?
            .collect::<Result<_, _>>()
            .map_err(e)?;
        let mut q = c
            .prepare("SELECT id, title, han_total, first_read, finished, debt FROM books")
            .map_err(e)?;
        let books = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    BookStats {
                        title: r.get(1)?,
                        han_total: r.get::<_, i64>(2)? as u64,
                        first_read: r.get::<_, Option<i64>>(3)?.map(|v| v as u64),
                        finished: r.get::<_, Option<i64>>(4)?.map(|v| v as u64),
                        debt: r.get::<_, i64>(5)? as u64,
                        ..BookStats::default()
                    },
                ))
            })
            .map_err(e)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(e)?;
        s.books = books.into_iter().collect();
        let mut q = c
            .prepare("SELECT book, day, read_secs, read_chars, listen_secs, listen_chars FROM days")
            .map_err(e)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i32>(1)?,
                    day_of_row(r, 2)?,
                ))
            })
            .map_err(e)?;
        for row in rows {
            let (book, d, v) = row.map_err(e)?;
            s.books.entry(book).or_default().days.insert(d, v);
        }
        let mut q = c
            .prepare(
                "SELECT book, start, end, read_secs, read_chars, listen_secs, listen_chars
                 FROM sessions ORDER BY book, seq",
            )
            .map_err(e)?;
        let rows = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    Session {
                        start: r.get::<_, i64>(1)? as u64,
                        end: r.get::<_, i64>(2)? as u64,
                        totals: day_of_row(r, 3)?,
                    },
                ))
            })
            .map_err(e)?;
        for row in rows {
            let (book, sess) = row.map_err(e)?;
            s.books.entry(book).or_default().sessions.push(sess);
        }
        let mut q = c
            .prepare("SELECT book, spine, offset, saved FROM places")
            .map_err(e)?;
        s.places = q
            .query_map([], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    LeftAt {
                        spine: r.get(1)?,
                        offset: r.get::<_, i64>(2)? as u64,
                        saved: r.get::<_, i64>(3)? as u64,
                    },
                ))
            })
            .map_err(e)?
            .collect::<Result<_, _>>()
            .map_err(e)?;
        Ok(s)
    }

    fn day_of_row(r: &rusqlite::Row, i: usize) -> rusqlite::Result<Day> {
        Ok(Day {
            read_secs: r.get(i)?,
            read_chars: r.get::<_, i64>(i + 1)? as u64,
            listen_secs: r.get(i + 2)?,
            listen_chars: r.get::<_, i64>(i + 3)? as u64,
        })
    }

    /// Write a batch in one transaction: each book's rows replaced whole.
    pub fn write(c: &mut Connection, b: &Batch) -> rusqlite::Result<()> {
        let t = c.transaction()?;
        for id in &b.removed {
            for sql in [
                "DELETE FROM books WHERE id = ?1",
                "DELETE FROM days WHERE book = ?1",
                "DELETE FROM sessions WHERE book = ?1",
            ] {
                t.execute(sql, [id])?;
            }
        }
        for (id, book) in &b.books {
            t.execute(
                "INSERT OR REPLACE INTO books (id, title, han_total, first_read, finished, debt)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    id,
                    book.title,
                    book.han_total as i64,
                    book.first_read.map(|v| v as i64),
                    book.finished.map(|v| v as i64),
                    book.debt as i64
                ],
            )?;
            t.execute("DELETE FROM days WHERE book = ?1", [id])?;
            t.execute("DELETE FROM sessions WHERE book = ?1", [id])?;
            let mut q = t.prepare(
                "INSERT INTO days (book, day, read_secs, read_chars, listen_secs, listen_chars)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )?;
            for (d, v) in &book.days {
                q.execute(params![
                    id,
                    d,
                    v.read_secs,
                    v.read_chars as i64,
                    v.listen_secs,
                    v.listen_chars as i64
                ])?;
            }
            let mut q = t.prepare(
                "INSERT INTO sessions (book, seq, start, end, read_secs, read_chars, listen_secs, listen_chars)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for (i, s) in book.sessions.iter().enumerate() {
                let v = &s.totals;
                q.execute(params![
                    id,
                    i as i64,
                    s.start as i64,
                    s.end as i64,
                    v.read_secs,
                    v.read_chars as i64,
                    v.listen_secs,
                    v.listen_chars as i64
                ])?;
            }
        }
        for (id, p) in &b.places {
            t.execute(
                "INSERT OR REPLACE INTO places (book, spine, offset, saved) VALUES (?1, ?2, ?3, ?4)",
                params![id, p.spine, p.offset as i64, p.saved as i64],
            )?;
        }
        if let Some((settings, goals)) = &b.meta {
            let json = serde_json::to_string(settings).unwrap_or_default();
            t.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('settings', ?1)",
                [json],
            )?;
            t.execute("DELETE FROM goals", [])?;
            for g in goals {
                t.execute(
                    "INSERT OR REPLACE INTO goals (from_day, minutes, chars) VALUES (?1, ?2, ?3)",
                    params![g.from_day, g.minutes, g.chars as i64],
                )?;
            }
        }
        t.commit()
    }
}

/// The thread that writes batches, one after another.
mod writer {
    use super::db;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::{Condvar, LazyLock, Mutex, Once};

    #[derive(Default)]
    struct State {
        queue: VecDeque<(PathBuf, db::Batch)>,
        busy: bool,
        error: Option<String>,
    }

    static STATE: LazyLock<(Mutex<State>, Condvar)> =
        LazyLock::new(|| (Mutex::new(State::default()), Condvar::new()));
    static START: Once = Once::new();

    pub fn queue(path: PathBuf, batch: db::Batch) {
        START.call_once(|| {
            let _ = std::thread::Builder::new()
                .name("reading-stats".into())
                .spawn(run);
        });
        let (m, cv) = &*STATE;
        m.lock()
            .unwrap_or_else(|e| e.into_inner())
            .queue
            .push_back((path, batch));
        cv.notify_all();
    }

    pub fn take_error() -> Option<String> {
        STATE
            .0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .error
            .take()
    }

    pub fn flush() {
        let (m, cv) = &*STATE;
        let mut st = m.lock().unwrap_or_else(|e| e.into_inner());
        while !st.queue.is_empty() || st.busy {
            st = cv.wait(st).unwrap_or_else(|e| e.into_inner());
        }
    }

    fn run() {
        let (m, cv) = &*STATE;
        let mut conn: Option<(PathBuf, rusqlite::Connection)> = None;
        loop {
            let (path, batch) = {
                let mut st = m.lock().unwrap_or_else(|e| e.into_inner());
                while st.queue.is_empty() {
                    st = cv.wait(st).unwrap_or_else(|e| e.into_inner());
                }
                st.busy = true;
                st.queue.pop_front().unwrap()
            };
            if conn.as_ref().is_none_or(|(p, _)| *p != path) {
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                conn = db::open(&path).ok().map(|c| (path.clone(), c));
            }
            let result = match &mut conn {
                Some((_, c)) => db::write(c, &batch).map_err(|e| e.to_string()),
                None => Err(format!("Cannot open {}", path.display())),
            };
            let mut st = m.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(e) = result {
                st.error = Some(format!("Cannot save the reading stats: {e}"));
                conn = None;
            }
            st.busy = false;
            cv.notify_all();
        }
    }
}

// Local time.

/// Seconds east of UTC at Unix time `t`, as the OS's time zone has it
/// (0 where unknown). Cached for the hour.
pub fn utc_offset(t: i64) -> i64 {
    static CACHE: Mutex<(i64, i64)> = Mutex::new((i64::MIN, 0));
    let hour = t.div_euclid(3600);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if c.0 != hour {
        *c = (hour, os_offset(hour * 3600));
    }
    c.1
}

#[cfg(windows)]
fn os_offset(t: i64) -> i64 {
    #[repr(C)]
    #[derive(Default)]
    struct SystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        ms: u16,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SystemTimeToTzSpecificLocalTime(
            zone: *const std::ffi::c_void,
            utc: *const SystemTime,
            local: *mut SystemTime,
        ) -> i32;
    }
    let days = t.div_euclid(86_400);
    let (y, m, d) = civil(days);
    let secs = t.rem_euclid(86_400);
    let utc = SystemTime {
        year: y as u16,
        month: m as u16,
        day: d as u16,
        hour: (secs / 3600) as u16,
        minute: (secs / 60 % 60) as u16,
        second: (secs % 60) as u16,
        ..Default::default()
    };
    let mut local = SystemTime::default();
    // SAFETY: both structs are SYSTEMTIME-shaped and live for the call; a
    // null zone means the current one.
    let ok = unsafe { SystemTimeToTzSpecificLocalTime(std::ptr::null(), &utc, &mut local) };
    if ok == 0 {
        return 0;
    }
    let local_t = days_from_civil(local.year as i64, local.month as i64, local.day as i64) * 86_400
        + local.hour as i64 * 3600
        + local.minute as i64 * 60
        + local.second as i64;
    local_t - t
}

#[cfg(target_os = "linux")]
fn os_offset(t: i64) -> i64 {
    let tt = t as libc::time_t;
    // SAFETY: `tm` is plain data, written by localtime_r.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let r = unsafe { libc::localtime_r(&tt, &mut tm) };
    if r.is_null() { 0 } else { tm.tm_gmtoff as i64 }
}

#[cfg(target_os = "macos")]
fn os_offset(t: i64) -> i64 {
    use std::ffi::{c_char, c_int, c_long};
    #[repr(C)]
    struct Tm {
        tm_sec: c_int,
        tm_min: c_int,
        tm_hour: c_int,
        tm_mday: c_int,
        tm_mon: c_int,
        tm_year: c_int,
        tm_wday: c_int,
        tm_yday: c_int,
        tm_isdst: c_int,
        tm_gmtoff: c_long,
        tm_zone: *mut c_char,
    }
    unsafe extern "C" {
        fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm;
    }
    // SAFETY: `Tm` matches <time.h>'s struct tm on macOS; plain data.
    let mut tm: Tm = unsafe { std::mem::zeroed() };
    let r = unsafe { localtime_r(&t, &mut tm) };
    if r.is_null() { 0 } else { tm.tm_gmtoff as i64 }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn os_offset(_: i64) -> i64 {
    0
}

/// (year, month, day) of a day number (days since 1970-01-01), Hinnant's
/// days-to-civil.
pub fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The reading day of Unix time `t`: local days that begin at `start_hour`.
pub fn local_day(t: f64, start_hour: u32) -> i32 {
    day_with(t, start_hour, &utc_offset)
}

fn day_with(t: f64, start_hour: u32, off: &dyn Fn(i64) -> i64) -> i32 {
    let ti = t.floor() as i64;
    (ti + off(ti) - start_hour as i64 * 3600).div_euclid(86_400) as i32
}

/// When reading day `day` begins, in Unix time.
fn day_begins(day: i32, start_hour: u32, off: &dyn Fn(i64) -> i64) -> f64 {
    let local = day as i64 * 86_400 + start_hour as i64 * 3600;
    // The offset of the moment itself, found from a guess one step away.
    let guess = local - off(local);
    (local - off(guess)) as f64
}

/// Split the span `a..b` (Unix times) into its reading days.
pub fn split_days(a: f64, b: f64, start_hour: u32) -> Vec<(i32, f64)> {
    split_with(a, b, start_hour, &utc_offset)
}

fn split_with(a: f64, b: f64, start_hour: u32, off: &dyn Fn(i64) -> i64) -> Vec<(i32, f64)> {
    let mut out = Vec::new();
    let mut t = a;
    while t < b {
        let d = day_with(t, start_hour, off);
        let end = day_begins(d + 1, start_hour, off).max(t + 1.0).min(b);
        out.push((d, end - t));
        t = end;
    }
    out
}

/// "2026-11-23" for a day number.
pub fn date(day: i32) -> String {
    let (y, m, d) = civil(day as i64);
    format!("{y}-{m:02}-{d:02}")
}

/// "Mon 23 Nov 2026".
pub fn long_date(day: i32) -> String {
    const WD: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MON: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (y, m, d) = civil(day as i64);
    format!(
        "{} {d} {} {y}",
        WD[weekday(day) as usize],
        MON[m as usize - 1]
    )
}

/// 0 = Monday.
pub fn weekday(day: i32) -> u32 {
    (day as i64 + 3).rem_euclid(7) as u32
}

/// The reading day of a Unix time, by the saved day start.
pub fn day_of(t: u64, s: &TrackerSettings) -> i32 {
    local_day(t as f64, s.day_start_hour)
}

#[cfg(test)]
static TEST_CLOCK: Mutex<Option<f64>> = Mutex::new(None);

/// Tests: [`now`] returns this while set.
#[cfg(test)]
pub fn set_test_clock(t: Option<f64>) {
    *TEST_CLOCK.lock().unwrap() = t;
}

/// Tests: the stats in memory.
#[cfg(test)]
pub fn reset_for_test(s: ReadingStats) {
    let mut st = STORE.lock().unwrap_or_else(|e| e.into_inner());
    st.stats = s;
    st.revision += 1;
}

pub fn now() -> f64 {
    #[cfg(test)]
    if let Some(t) = *TEST_CLOCK.lock().unwrap() {
        return t;
    }
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(read_mins: f64, chars: u64) -> Day {
        Day {
            read_secs: read_mins * 60.0,
            read_chars: chars,
            ..Day::default()
        }
    }

    #[test]
    fn civil_round_trips() {
        for z in [-800_000i64, -1, 0, 1, 10_957, 20_415, 60_000] {
            let (y, m, d) = civil(z);
            assert_eq!(days_from_civil(y, m as i64, d as i64), z);
        }
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(weekday(0), 3); // a Thursday
        assert_eq!(date(days_from_civil(2026, 11, 23) as i32), "2026-11-23");
        assert_eq!(
            long_date(days_from_civil(2026, 11, 23) as i32),
            "Mon 23 Nov 2026"
        );
    }

    #[test]
    fn days_start_at_the_hour_set() {
        let utc = |_: i64| 0;
        let midnight = days_from_civil(2026, 11, 23) as f64 * 86_400.0;
        let d = days_from_civil(2026, 11, 23) as i32;
        // 03:59 is still the day before; 04:00 is the new day.
        assert_eq!(
            day_with(midnight + 3.0 * 3600.0 + 59.0 * 60.0, 4, &utc),
            d - 1
        );
        assert_eq!(day_with(midnight + 4.0 * 3600.0, 4, &utc), d);
        assert_eq!(day_with(midnight, 0, &utc), d);
        // UTC+8: 20:00 UTC is 04:00 the next morning there.
        let east = |_: i64| 8 * 3600;
        assert_eq!(day_with(midnight + 20.0 * 3600.0, 4, &east), d + 1);
    }

    #[test]
    fn a_span_over_the_day_start_is_split() {
        let utc = |_: i64| 0;
        let midnight = days_from_civil(2026, 11, 23) as f64 * 86_400.0;
        let d = days_from_civil(2026, 11, 23) as i32;
        let parts = split_with(midnight + 3.5 * 3600.0, midnight + 4.25 * 3600.0, 4, &utc);
        assert_eq!(parts, vec![(d - 1, 1800.0), (d, 900.0)]);
        assert_eq!(split_with(10.0, 10.0, 4, &utc), vec![]);
    }

    #[test]
    fn a_dst_change_moves_the_day_start() {
        // A zone at UTC+1 that moves to UTC+2 at 2026-03-29 01:00 UTC.
        let change = days_from_civil(2026, 3, 29) * 86_400 + 3600;
        let off = move |t: i64| if t >= change { 7200 } else { 3600 };
        let d = days_from_civil(2026, 3, 29) as i32;
        // The day begins at 04:00 local = 02:00 UTC (after the change).
        let begins = day_begins(d, 4, &off);
        assert_eq!(
            begins,
            (days_from_civil(2026, 3, 29) * 86_400 + 2 * 3600) as f64
        );
        assert_eq!(day_with(begins - 1.0, 4, &off), d - 1);
        assert_eq!(day_with(begins, 4, &off), d);
        // That day is 24 h long again by the next morning.
        let next = day_begins(d + 1, 4, &off);
        assert_eq!(next - begins, 86_400.0);
    }

    #[test]
    fn local_offset_is_sane() {
        let t = now() as i64;
        let o = utc_offset(t);
        assert!(o.abs() <= 14 * 3600, "{o}");
        assert_eq!(o % 900, 0, "{o}");
    }

    #[test]
    fn streaks_count_back_from_today() {
        let mut days = BTreeMap::new();
        for d in [1, 2, 3, 5, 6, 7, 8, 10] {
            days.insert(d, day(10.0, 100));
        }
        days.insert(4, day(0.5, 10)); // under a minute
        let ok = |_: i32, d: &Day| d.read_secs >= DAY_READ_SECS;
        assert_eq!(streaks(&days, 10, ok), (1, 4));
        // Today not read yet: yesterday's run still counts.
        assert_eq!(streaks(&days, 9, ok), (4, 4));
        // Two days missed: none.
        assert_eq!(streaks(&days, 12, ok), (0, 4));
    }

    #[test]
    fn goals_apply_from_their_day_on() {
        let mut s = ReadingStats::default();
        s.set_goal(10, 30, 0);
        s.set_goal(20, 0, 5000);
        s.set_goal(20, 0, 6000); // changed again the same day
        assert_eq!(s.goals.len(), 2);
        assert_eq!(s.goal_on(9), None);
        assert_eq!(s.goal_on(15).unwrap().minutes, 30);
        assert_eq!(s.goal_on(25).unwrap().chars, 6000);
        s.set_goal(30, 0, 0);
        assert_eq!(s.goal_on(31), None);
        assert_eq!(s.goal_on(25).unwrap().chars, 6000);
        let g = Goal {
            from_day: 0,
            minutes: 30,
            chars: 6000,
        };
        assert!(!g.met(&day(40.0, 5000)));
        assert!(g.met(&day(30.0, 6000)));
        assert_eq!(g.progress(&day(15.0, 6000)), 0.5);
    }

    #[test]
    fn measured_speed_uses_the_last_30_days() {
        let mut s = ReadingStats::default();
        let b = s.books.entry("a".into()).or_default();
        b.days.insert(100, day(20.0, 6000)); // 300/min
        b.days.insert(60, day(100.0, 100)); // too old
        assert_eq!(s.measured_speed(100), Some(300.0));
        assert_eq!(s.measured_speed(130), None);
        let b = s.books.entry("b".into()).or_default();
        b.days.insert(99, day(5.0, 0));
        b.days.get_mut(&99).unwrap().listen_secs = 3600.0;
        b.days.get_mut(&99).unwrap().listen_chars = 99_999;
        // Listening is left out; reading time without characters lowers it.
        assert_eq!(s.measured_speed(100), Some(6000.0 / 25.0));
    }

    fn sample() -> ReadingStats {
        let mut s = ReadingStats::default();
        s.settings.idle_mins = 7;
        s.set_goal(5, 20, 0);
        s.set_goal(9, 0, 3000);
        for (id, title) in [("lib-1", "书"), ("C:\\书\\b.epub", "另一本")] {
            let b = s.books.entry(id.into()).or_default();
            b.title = title.into();
            b.han_total = 123_456;
            b.first_read = Some(1_790_000_000);
            b.debt = 17;
            b.days.insert(20_000, day(12.5, 3456));
            b.days.insert(
                20_001,
                Day {
                    listen_secs: 61.25,
                    listen_chars: 9,
                    ..day(0.0, 0)
                },
            );
            b.sessions.push(Session {
                start: 1,
                end: 2,
                totals: day(1.0, 2),
            });
            b.sessions.push(Session {
                start: 3,
                end: 4,
                totals: day(2.0, 5),
            });
        }
        s.books.get_mut("lib-1").unwrap().finished = Some(1_790_100_000);
        s.places.insert(
            "lib-1".into(),
            LeftAt {
                spine: 12,
                offset: 3456,
                saved: 1_790_100_001,
            },
        );
        s
    }

    fn batch_all(s: &ReadingStats) -> db::Batch {
        db::Batch {
            books: s.books.clone().into_iter().collect(),
            removed: vec![],
            meta: Some((s.settings.clone(), s.goals.clone())),
            places: s.places.clone().into_iter().collect(),
        }
    }

    #[test]
    fn database_round_trips_and_rewrites_books_whole() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("stats.sqlite");
        let mut c = db::open(&p).unwrap();
        let mut s = sample();
        db::write(&mut c, &batch_all(&s)).unwrap();
        assert_eq!(db::read(&db::open(&p).unwrap()).unwrap(), s);
        // A discarded session: the book's days and sessions shrink.
        let b = s.books.get_mut("lib-1").unwrap();
        b.days.remove(&20_001);
        b.sessions.pop();
        let one = db::Batch {
            books: vec![("lib-1".into(), b.clone())],
            removed: vec![],
            meta: None,
            places: vec![],
        };
        db::write(&mut c, &one).unwrap();
        assert_eq!(db::read(&c).unwrap(), s);
        // A removed book is gone, the other kept.
        let gone = db::Batch {
            books: vec![],
            removed: vec!["lib-1".into()],
            meta: None,
            places: vec![],
        };
        db::write(&mut c, &gone).unwrap();
        s.books.remove("lib-1");
        assert_eq!(db::read(&c).unwrap(), s);
    }

    #[test]
    fn the_writer_thread_saves_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub").join("stats.sqlite");
        let mut s = sample();
        writer::queue(p.clone(), batch_all(&s));
        s.books.get_mut("lib-1").unwrap().debt = 0;
        writer::queue(
            p.clone(),
            db::Batch {
                books: vec![("lib-1".into(), s.books["lib-1"].clone())],
                removed: vec![],
                meta: None,
                places: vec![],
            },
        );
        writer::flush();
        assert_eq!(writer::take_error(), None);
        assert_eq!(db::read(&db::open(&p).unwrap()).unwrap(), s);
    }

    #[test]
    fn unreadable_or_newer_databases_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.sqlite");
        std::fs::write(
            &junk,
            b"not a database at all, just text that is long enough",
        )
        .unwrap();
        assert!(db::open(&junk).and_then(|c| db::read(&c)).is_err());
        assert_eq!(
            std::fs::read(&junk).unwrap(),
            b"not a database at all, just text that is long enough"
        );
        let newer = dir.path().join("newer.sqlite");
        {
            let c = rusqlite::Connection::open(&newer).unwrap();
            c.pragma_update(None, "user_version", 99).unwrap();
        }
        let e = db::open(&newer).err().unwrap();
        assert!(e.contains("newer hanstat"), "{e}");
    }

    #[test]
    fn file_round_trips_and_old_files_load() {
        let mut s = ReadingStats::default();
        s.set_goal(5, 20, 0);
        let b = s.books.entry("lib-1".into()).or_default();
        b.title = "书".into();
        b.days.insert(20_000, day(12.5, 3456));
        b.sessions.push(Session {
            start: 1,
            end: 2,
            totals: day(1.0, 2),
        });
        let json = serde_json::to_vec(&s).unwrap();
        let back: ReadingStats = serde_json::from_slice(&json).unwrap();
        assert_eq!(back, s);
        let old: ReadingStats =
            serde_json::from_str(r#"{"books":{"x":{"days":{"3":{"read_secs":5}}}}}"#).unwrap();
        assert_eq!(old.settings, TrackerSettings::default());
        assert_eq!(old.books["x"].days[&3].read_secs, 5.0);
    }
}
